//! The page's calls for the leftovers of finished tasks (PLN-53): the list, «Остановить» and the switch of the
//! automatic cleaning. See `strays.rs` for the rule.

use crate::app::{self, Shared};
use crate::config::now_ms;
use crate::server::{error, same_origin};
use crate::strays::{self, Target};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

const MAX_TARGETS: usize = 200;
const JOURNAL_ROWS: usize = 30;

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    fresh: Option<String>,
}

fn state_json(app: &Shared, scan: &strays::Scan) -> serde_json::Value {
    let total_rss: u64 = scan.strays.iter().map(|s| s.rss).sum();
    json!({
        "ok": scan.ok,
        "error": scan.error,
        "board_ok": scan.board_ok,
        "supported": strays::supported(),
        "at": scan.at,
        "now": now_ms(),
        "auto": strays::auto_on(app),
        "grace_ms": strays::grace_ms(),
        "term_wait_ms": strays::term_wait_ms(),
        "total_rss": total_rss,
        "strays": scan.strays,
        "journal": app.db.log_page(strays::JOURNAL_KEY, strays::KIND, "", "", 0, JOURNAL_ROWS),
    })
}

/// `GET /api/strays`: the last look at this machine (a new one with `?fresh=1`, which is what «Проверить» asks for).
pub async fn strays_get(State(app): State<Shared>, Query(q): Query<ListQuery>) -> Response {
    let fresh = q.fresh.as_deref().is_some_and(|v| v == "1" || v == "true");
    let scan = strays::latest(&app, fresh).await;
    Json(state_json(&app, &scan)).into_response()
}

#[derive(Deserialize)]
pub struct StopBody {
    #[serde(default)]
    targets: Vec<Target>,
    confirm: Option<bool>,
}

/// `POST /api/strays/stop`: stops the leftovers the page showed (by pid and start time). Each is found again first and
/// refused when it is not the same process or no longer a leftover; the answer says what happened to each.
pub async fn strays_stop(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<StopBody>) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    if body.confirm != Some(true) {
        return error(StatusCode::BAD_REQUEST, "остановка требует подтверждения");
    }
    if body.targets.is_empty() {
        return error(StatusCode::BAD_REQUEST, "не выбрано ни одного процесса");
    }
    if body.targets.len() > MAX_TARGETS {
        return error(StatusCode::UNPROCESSABLE_ENTITY, format!("за раз — не больше {MAX_TARGETS} процессов"));
    }
    let board = {
        let snap = app.snapshot.read().await;
        strays::board_of(&snap)
    };
    let report = strays::stop(strays::sys().as_ref(), &board, &body.targets, strays::term_wait_ms()).await;
    // A press that found nothing to stop (everything gone or refused) is not an action worth a row.
    if report.stopped() > 0 || report.count("failed") > 0 {
        app.db.log_add(strays::JOURNAL_KEY, &strays::stopped_entry(now_ms(), crate::runners_log::HUMAN, &report));
    }
    if report.stopped() > 0 {
        app::journal_note(&app, "strays", "runner", &format!("остановлено процессов завершённых задач: {}", report.stopped())).await;
    }
    let scan = strays::scan(&app).await;
    let mut out = state_json(&app, &scan);
    out["report"] = json!(report);
    out["stopped"] = json!(report.stopped());
    Json(out).into_response()
}

#[derive(Deserialize)]
pub struct AutoBody {
    enabled: bool,
    confirm: Option<bool>,
}

/// `POST /api/strays/auto`: the switch of the automatic cleaning (off by default; switching on is the consent).
pub async fn strays_auto(State(app): State<Shared>, headers: HeaderMap, Json(body): Json<AutoBody>) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    if body.confirm != Some(true) {
        return error(StatusCode::BAD_REQUEST, "изменение требует подтверждения");
    }
    strays::set_auto(&app, body.enabled);
    app.db.log_add(strays::JOURNAL_KEY, &strays::switch_entry(now_ms(), body.enabled));
    app::journal_note(
        &app,
        "strays",
        "runner",
        &format!("автоуборка процессов завершённых задач: {}", if body.enabled { "включена" } else { "выключена" }),
    )
    .await;
    let scan = strays::latest(&app, false).await;
    Json(state_json(&app, &scan)).into_response()
}
