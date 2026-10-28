//! Which version a device gets through the gate: the light page (`/m`) or the full planner (`/`).
//!
//! The server decides when a device first comes in (the browser's own words: `Sec-CH-UA-Mobile`, the `User-Agent`, and
//! whether the sign-in page found a touch screen), and the person can switch by hand afterwards: a cookie `hp_view`
//! (`lite` or `full`) that says only what they prefer. It gives no right: the full planner is reached only by a device
//! that was raised to it with the planner's password (`devices::Devices::raise_at`), and only while the gate's setting
//! «full version for computers» is on and a password is set. From this machine `/` is always the full planner and `/m`
//! opens as well (so the page can be built and checked); the rules here are for the tunnel.

use axum::http::{header, HeaderMap, HeaderValue};
use serde::Serialize;

/// The preference: not secret, not a right.
pub const COOKIE: &str = "hp_view";
/// A year, in seconds.
const COOKIE_AGE: u64 = 365 * 24 * 60 * 60;

/// What kind of machine the browser is on, as far as it says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Form {
    Phone,
    Tablet,
    #[default]
    Desktop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Lite,
    Full,
}

impl View {
    pub fn parse(word: &str) -> Option<View> {
        match word {
            "lite" => Some(View::Lite),
            "full" => Some(View::Full),
            _ => None,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            View::Lite => "lite",
            View::Full => "full",
        }
    }
}

/// Phone, tablet or computer from the words of the browser. `hint` is `Sec-CH-UA-Mobile` (`?1` or `?0`; Chromium sends
/// it on every request), `touch` what the sign-in page found out (a touch screen that is the main pointer): an iPad
/// with iPadOS 13+ says «Macintosh» and nothing else tells it from a Mac.
pub fn form_from(ua: &str, hint: Option<&str>, touch: bool) -> Form {
    if hint.map(str::trim) == Some("?1") {
        return Form::Phone;
    }
    if ua.contains("iPad") {
        return Form::Tablet;
    }
    if ua.contains("iPhone") || ua.contains("iPod") {
        return Form::Phone;
    }
    if ua.contains("Android") {
        // a tablet's Chrome leaves «Mobile» out
        return if ua.contains("Mobile") { Form::Phone } else { Form::Tablet };
    }
    if ua.contains("Mobi") || ua.contains("Windows Phone") || ua.contains("Opera Mini") {
        return Form::Phone;
    }
    // «Request desktop site» on an Android phone, and iPadOS pretending to be a Mac: a touch screen is the proof
    if touch && (ua.contains("Macintosh") || ua.contains("X11") || ua.contains("Linux")) {
        return Form::Tablet;
    }
    Form::Desktop
}

pub fn form_of(headers: &HeaderMap, touch: bool) -> Form {
    let ua = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).unwrap_or("");
    let hint = headers.get("sec-ch-ua-mobile").and_then(|v| v.to_str().ok());
    form_from(ua, hint, touch)
}

/// The cookie `hp_view`, if it says `lite` or `full`.
pub fn preference(headers: &HeaderMap) -> Option<View> {
    crate::auth::cookie_value(headers, COOKIE).as_deref().and_then(View::parse)
}

/// `Set-Cookie` for the preference.
pub fn cookie_header(view: View) -> String {
    format!("{COOKIE}={}; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age={COOKIE_AGE}", view.word())
}

/// Where a device goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// The light page.
    Lite,
    /// The planner.
    Full,
    /// The planner, but the device must first enter the planner's password to be raised to it.
    Password,
}

impl Choice {
    pub fn path(self) -> &'static str {
        match self {
            Choice::Lite => "/m",
            Choice::Full => "/",
            Choice::Password => "/login?next=/",
        }
    }
}

/// The version for a device in the tunnel: its own preference if it has one, else the kind of machine decides (a
/// computer wants the full version, the rest the light one). `raised`: the device has the full right already;
/// `can_full`: the setting is on and a password is set, so a device may get it. The full version that cannot be had
/// is the light one: the page says why.
pub fn choose(form: Form, pref: Option<View>, raised: bool, can_full: bool) -> Choice {
    let want = pref.unwrap_or(if form == Form::Desktop { View::Full } else { View::Lite });
    match want {
        View::Lite => Choice::Lite,
        View::Full if !can_full => Choice::Lite,
        View::Full if raised => Choice::Full,
        View::Full => Choice::Password,
    }
}

/// Why a device has no full version, or how it gets one: what the light page's settings and notice say.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FullState {
    /// This device has it.
    Yes,
    /// It may have it: it needs the planner's password.
    Login,
    /// The gate's setting is off.
    Closed,
    /// The setting is on but the planner has no password (the full version needs one).
    Nopassword,
}

pub fn full_state(allowed: bool, password_set: bool, raised: bool) -> FullState {
    match (allowed, password_set) {
        (false, _) => FullState::Closed,
        (true, false) => FullState::Nopassword,
        (true, true) if raised => FullState::Yes,
        (true, true) => FullState::Login,
    }
}

/// The answer to «switch to…»: the preference is remembered, and the person goes to the version.
pub fn switched(view: View) -> axum::response::Response {
    use axum::response::IntoResponse;
    let to = match view {
        View::Lite => "/m",
        View::Full => "/",
    };
    let mut res = axum::response::Redirect::to(to).into_response();
    if let Ok(v) = HeaderValue::from_str(&cookie_header(view)) {
        res.headers_mut().insert(header::SET_COOKIE, v);
    }
    res
}

#[derive(serde::Deserialize)]
pub struct ViewQuery {
    v: String,
}

/// `GET /g/view?v=lite|full`: the hand switch (the gate lets in a device that is in; from this machine it works as well).
pub async fn view(axum::extract::Query(q): axum::extract::Query<ViewQuery>) -> axum::response::Response {
    match View::parse(&q.v) {
        Some(v) => switched(v),
        None => crate::server::error(axum::http::StatusCode::BAD_REQUEST, "v is lite or full"),
    }
}

#[cfg(test)]
#[path = "version_tests.rs"]
mod tests;
