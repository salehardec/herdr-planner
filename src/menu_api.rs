//! `POST /api/menu/answer`: a choice in an agent's selection menu, sent as the keys that make it.
//!
//! The page shows the menu as buttons (see `menu.rs`); a button names a row, not keys. The screen is read again here,
//! so the keys are counted from where the cursor is now, and an answer to a menu that has changed since the page
//! saw it (the person answered in the terminal, or the program went on) is refused instead of landing in the next one.

use crate::app::Shared;
use crate::menu::{self, Answer, Step};
use crate::server::{agent_name, error, run_herdr, same_origin};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

#[derive(Deserialize)]
pub struct AnswerBody {
    /// `coordinator` or a task id.
    target: Option<String>,
    /// `reviewer` asks for `<agent>-r`.
    role: Option<String>,
    /// The `sig` of the menu the page showed.
    sig: String,
    answer: AnswerJson,
    /// What the human did, when the page knows the words for it: `allow`, `always` or `deny` (a permission
    /// prompt). The Coordinator is told; an unrecognised value is not news.
    note: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum AnswerJson {
    Pick { at: usize },
    Own { text: String },
    Multi { picked: Vec<usize>, own: Option<String> },
    Cancel,
}

impl From<AnswerJson> for Answer {
    fn from(a: AnswerJson) -> Answer {
        match a {
            AnswerJson::Pick { at } => Answer::Pick(at),
            AnswerJson::Own { text } => Answer::Own(text),
            AnswerJson::Multi { picked, own } => Answer::Multi { picked, own },
            AnswerJson::Cancel => Answer::Cancel,
        }
    }
}

/// How many keys one `herdr agent send-keys` carries (the board's own limit for the `keys` action).
const KEYS_PER_CALL: usize = 12;

pub async fn menu_answer(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<AnswerBody>,
) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    let target = body.target.as_deref().unwrap_or("coordinator");
    let name = match agent_name(target, body.role.as_deref()) {
        Ok(name) => name,
        Err(e) => return error(StatusCode::BAD_REQUEST, e),
    };
    if !app.cfg.herdr_socket.exists() {
        return error(StatusCode::BAD_GATEWAY, "herdr не запущен: ответить в меню отсюда нельзя");
    }
    // One answer at a time: two sets of keys must not interleave in a terminal.
    let _guard = app.action_lock.lock().await;
    let read = ["agent", "read", &name, "--source", "visible", "--lines", "80", "--format", "text"];
    let screen = match run_herdr(&read, Duration::from_secs(8)).await {
        Ok(out) => crate::server::strip_ansi(&out),
        Err(e) => return error(StatusCode::BAD_GATEWAY, e),
    };
    let Some(current) = menu::parse(&screen) else {
        return gone("в терминале уже нет меню: ответ дан или программа пошла дальше");
    };
    if current.sig != body.sig {
        return gone("меню изменилось: на экране уже другой вопрос");
    }
    let steps = match menu::plan(&current, &body.answer.into()) {
        Ok(s) => s,
        Err(e) => return error(StatusCode::BAD_REQUEST, e),
    };
    let mut pane: Option<String> = None;
    for (i, step) in steps.iter().enumerate() {
        if i > 0 {
            // the program redraws between a key and the next step
            tokio::time::sleep(Duration::from_millis(120)).await;
        }
        let done = match step {
            Step::Keys(keys) => {
                let mut res = Ok(());
                for chunk in keys.chunks(KEYS_PER_CALL) {
                    let mut args: Vec<&str> = vec!["agent", "send-keys", &name];
                    args.extend(chunk.iter().copied());
                    if let Err(e) = run_herdr(&args, Duration::from_secs(10)).await {
                        res = Err(e);
                        break;
                    }
                }
                res
            }
            Step::Text(text) => {
                if pane.is_none() {
                    match pane_of(&name).await {
                        Ok(id) => pane = Some(id),
                        Err(e) => return error(StatusCode::BAD_GATEWAY, e),
                    }
                }
                let id = pane.as_deref().unwrap_or_default();
                run_herdr(&["pane", "send-text", id, text], Duration::from_secs(10)).await.map(|_| ())
            }
        };
        if let Err(e) = done {
            return error(StatusCode::BAD_GATEWAY, e);
        }
    }
    // A permission answered from a task's card is a human's action on that task: the Coordinator learns of it.
    if target != "coordinator" {
        let what = match body.note.as_deref() {
            Some("allow") => Some("разрешено действие агента"),
            Some("always") => Some("разрешено действие агента, больше не спрашивать"),
            Some("deny") => Some("действие агента запрещено"),
            _ => None,
        };
        if let Some(what) = what {
            crate::app::planner_notice(&app, target, what).await;
            // The page reads the notice from the snapshot: read it again, so the chat shows it at once.
            crate::app::refresh(&app).await;
        }
    }
    Json(json!({"ok": true, "steps": steps.len()})).into_response()
}

fn gone(message: &str) -> Response {
    (StatusCode::CONFLICT, Json(json!({"ok": false, "gone": true, "error": message}))).into_response()
}

/// The pane an agent lives in: literal text goes to a pane, not to an agent name.
async fn pane_of(name: &str) -> Result<String, String> {
    let out = run_herdr(&["agent", "get", name], Duration::from_secs(8)).await?;
    let v: serde_json::Value = serde_json::from_str(out.trim()).map_err(|e| format!("herdr agent get: {e}"))?;
    v["result"]["agent"]["pane_id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "herdr не назвал панель агента".to_string())
}
