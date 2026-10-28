//! Server-sent events: the page learns of changes without polling.
//!
//! One broadcast channel. `state` carries the whole snapshot (serialised once
//! per refresh, only when somebody listens); `chat` says the Coordinator's
//! transcript grew; `toast` is a line for the page to show.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, watch};

#[derive(Debug, Clone)]
pub struct Msg {
    pub event: &'static str,
    pub data: String,
}

#[derive(Clone)]
pub struct Bus {
    tx: broadcast::Sender<Arc<Msg>>,
    /// Counts everything that was sent, and the changes nobody was told of (`pulse`): what a long-poll waits on.
    /// Unlike `tx` it costs nothing to keep a watcher, so the snapshot is not serialised for it.
    beat: Arc<watch::Sender<u64>>,
    /// How many times the Coordinator's transcript grew.
    chat: Arc<AtomicU64>,
}

impl Bus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(64);
        Bus { tx, beat: Arc::new(watch::channel(0).0), chat: Arc::new(AtomicU64::new(0)) }
    }

    /// Something changed, whether or not an event stream is open to hear it.
    pub fn pulse(&self) {
        self.beat.send_modify(|n| *n = n.wrapping_add(1));
    }

    /// A receiver that wakes at the next `pulse` or `send` (and not for the ones before it).
    pub fn watch(&self) -> watch::Receiver<u64> {
        self.beat.subscribe()
    }

    /// How many times the transcript of the Coordinator grew since the planner started.
    pub fn chat_rev(&self) -> u64 {
        self.chat.load(Ordering::Relaxed)
    }

    pub fn subscribers(&self) -> usize {
        self.tx.receiver_count()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Msg>> {
        self.tx.subscribe()
    }

    pub fn send(&self, event: &'static str, data: String) {
        if event == "chat" {
            self.chat.fetch_add(1, Ordering::Relaxed);
        }
        // Nobody listening is not an error.
        let _ = self.tx.send(Arc::new(Msg { event, data }));
        self.pulse();
    }
}

impl Default for Bus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn subscribers_get_what_is_sent() {
        let bus = Bus::new();
        assert_eq!(bus.subscribers(), 0);
        bus.send("state", "{}".into());
        let mut rx = bus.subscribe();
        assert_eq!(bus.subscribers(), 1);
        bus.send("chat", "1".into());
        let m = rx.recv().await.unwrap();
        assert_eq!(m.event, "chat");
        assert_eq!(m.data, "1");
    }

    #[tokio::test]
    async fn a_watcher_wakes_for_what_is_sent_after_it_began_and_for_a_bare_pulse() {
        let bus = Bus::new();
        bus.send("state", "{}".into());
        let mut rx = bus.watch();
        let none = tokio::time::timeout(std::time::Duration::from_millis(30), rx.changed()).await;
        assert!(none.is_err(), "what came before is not news");
        bus.pulse();
        assert!(rx.changed().await.is_ok());
        bus.send("toast", "{}".into());
        assert!(rx.changed().await.is_ok());
        assert_eq!(bus.subscribers(), 0, "a watcher is not an event stream");
    }

    #[test]
    fn the_transcript_counts_its_own_growth() {
        let bus = Bus::new();
        assert_eq!(bus.chat_rev(), 0);
        bus.send("state", "{}".into());
        assert_eq!(bus.chat_rev(), 0);
        bus.send("chat", "1".into());
        bus.send("chat", "1".into());
        assert_eq!(bus.chat_rev(), 2);
    }
}
