//! `herdr workspace list`: the focused workspace names the active project:
//! by its id (the board records each repo's space), by the task tag its label
//! starts with (`PLN-1 Title`), or by the project's name. herdr pads labels
//! with U+2800.

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
    workspace_id: String,
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

/// The focused workspace: its id and its label, or none when nothing is focused.
pub fn focused(stdout: &str) -> Option<(String, String)> {
    let reply: Reply = serde_json::from_str(stdout).ok()?;
    let ws = reply.result?.workspaces.into_iter().find(|w| w.focused)?;
    let label = clean_label(&ws.label);
    (!label.is_empty()).then_some((ws.workspace_id, label))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_focused_label() {
        let text = r#"{"id":"cli:workspace:list","result":{"type":"workspace_list","workspaces":[{"focused":false,"label":"A⠀"},{"focused":true,"label":"APICache⠀"}]}}"#;
        assert_eq!(focused(text), Some((String::new(), "APICache".to_string())));
        let with_id = r#"{"result":{"workspaces":[{"focused":true,"label":"PLN-1 Title⠀","workspace_id":"wDD"}]}}"#;
        assert_eq!(focused(with_id), Some(("wDD".to_string(), "PLN-1 Title".to_string())));
        assert_eq!(focused(r#"{"result":{"workspaces":[]}}"#), None);
        assert_eq!(focused("garbage"), None);
    }
}
