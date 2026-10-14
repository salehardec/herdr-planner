//! First-run import of the Tern plugin's saved block state.
//!
//! Tern keeps every block's `dash.save` result inside its binary daemon state
//! (`~/.local/state/tern/daemon.state`), next to the block's kind string
//! `herdr-dashboard.dashboard`. The file is read only, never written: each
//! occurrence of the kind is followed (within a short gap) by the JSON object
//! the block saved; the last one that parses and carries the plugin's keys wins.

use crate::config::state_home;
use crate::store::State;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub const KIND: &[u8] = b"herdr-dashboard.dashboard";

pub fn default_path() -> PathBuf {
    state_home().join("tern/daemon.state")
}

/// The plugin's saved JSON, extracted from the daemon state's bytes.
pub fn extract(bytes: &[u8]) -> Option<Value> {
    let mut best = None;
    let mut from = 0;
    while let Some(at) = find(bytes, KIND, from) {
        from = at + KIND.len();
        let window_end = (from + 240).min(bytes.len());
        let mut start = from;
        while let Some(off) = bytes[start..window_end].iter().position(|&b| b == b'{') {
            let i = start + off;
            let text = String::from_utf8_lossy(&bytes[i..]);
            let mut de = serde_json::Deserializer::from_str(&text).into_iter::<Value>();
            if let Some(Ok(v)) = de.next() {
                if v.is_object()
                    && ["groups", "settings", "qa", "fresh"]
                        .iter()
                        .any(|k| v.get(k).is_some())
                {
                    best = Some(v);
                    break;
                }
            }
            start = i + 1;
        }
    }
    best
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from >= hay.len() {
        return None;
    }
    hay[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

/// The imported state, or none when the file is missing or holds no block state.
pub fn import(path: &Path) -> Option<State> {
    let bytes = std::fs::read(path).ok()?;
    let v = extract(&bytes)?;
    let mut s = State::from_plugin_json(&v);
    s.imported_from = Some(path.display().to_string());
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_the_last_full_state() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"\x00\x12\x19herdr-dashboard.dashboard\x1a\x13orion: Speakr\xe2\xa0\x80\x1a\xe1\x89\x01");
        bytes.extend_from_slice(br#"{"groups":{"configured":true,"list":["A"],"assign":{},"collapsed":[]},"settings":{"agent":"haiku"}}"#);
        bytes.extend_from_slice(
            b"\x00garbage\x00{\"plugin\":{\"kind\":\"herdr-dashboard.dashboard\",\"args\":[]}}",
        );
        bytes.extend_from_slice(b"herdr-dashboard.dashboard\x1a\x05{\"groups\":{\"configured\":true,\"list\":[\"B\"],\"assign\":{},\"collapsed\":[]}}trailing");
        let v = extract(&bytes).unwrap();
        assert_eq!(v["groups"]["list"][0], "B");
    }

    #[test]
    fn nothing_without_state() {
        assert!(extract(b"herdr-dashboard.dashboard then nothing").is_none());
        assert!(extract(b"").is_none());
    }
}
