//! Server-sent events: the page learns of changes without polling.
//!
//! One broadcast channel. `state` carries the whole snapshot (serialised once
//! per refresh, only when somebody listens); `chat` says the Coordinator's
//! transcript grew; `toast` is a line for the page to show.

use std::sync::Arc;
use tokio::sync::broadcast;

#[derive(Debug, Clone)]
pub struct Msg {
    pub event: &'static str,
    pub data: String,
}

#[derive(Clone)]
pub struct Bus {
    tx: broadcast::Sender<Arc<Msg>>,
}

impl Bus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(64);
        Bus { tx }
    }

    pub fn subscribers(&self) -> usize {
        self.tx.receiver_count()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Msg>> {
        self.tx.subscribe()
    }

    pub fn send(&self, event: &'static str, data: String) {
        // Nobody listening is not an error.
        let _ = self.tx.send(Arc::new(Msg { event, data }));
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
}
