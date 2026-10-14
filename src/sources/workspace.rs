//! `herdr workspace list`: the focused workspace's label names the active
//! project (label = project name, else slug). herdr pads labels with U+2800.

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Reply {
    result: Option<Result_>,
}

#[derive(Debug, Deserialize)]
struct Result_ {
    #[serde(default)]
    workspaces: Vec<Workspace>,
}

#[derive(Debug, Deserialize)]
struct Workspace {
    #[serde(default)]
    label: String,
    #[serde(default)]
    focused: bool,
}

/// A label without herdr's padding and surrounding whitespace.
pub fn clean_label(label: &str) -> String {
    label
        .replace(['\u{2800}', '\u{a0}'], " ")
        .trim()
        .to_string()
}

/// The focused workspace's label, or none when nothing is focused.
pub fn focused_label(stdout: &str) -> Option<String> {
    let reply: Reply = serde_json::from_str(stdout).ok()?;
    let ws = reply.result?.workspaces.into_iter().find(|w| w.focused)?;
    let label = clean_label(&ws.label);
    (!label.is_empty()).then_some(label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_focused_label() {
        let text = r#"{"id":"cli:workspace:list","result":{"type":"workspace_list","workspaces":[{"focused":false,"label":"A⠀"},{"focused":true,"label":"APICache⠀"}]}}"#;
        assert_eq!(focused_label(text).as_deref(), Some("APICache"));
        assert_eq!(focused_label(r#"{"result":{"workspaces":[]}}"#), None);
        assert_eq!(focused_label("garbage"), None);
    }
}
