//! Plan usage in the terms the top bar and its card show: which provider, which account, which limit windows, how much
//! of each is used, when each resets, and whatever else the provider's service reports. The model and the pace maths
//! follow herdr-gpui's `herdr-usage` crate (`model.rs`), so the same account reads the same numbers in both.

pub const MIN: u64 = 60_000;
pub const HOUR: u64 = 60 * MIN;
pub const DAY: u64 = 24 * HOUR;
pub const SESSION: u64 = 5 * HOUR;
pub const WEEK: u64 = 7 * DAY;
/// A calendar month varies; thirty days is close enough for a pace.
pub const MONTH: u64 = 30 * DAY;

/// Which limit a window measures. A named window is one the service names itself, such as a weekly limit scoped to one
/// model. The order is the order a card lists them in.
/// Daily and monthly windows come with the providers that have them (Kimi's monthly pool is a named one for now).
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Session,
    Daily,
    Weekly,
    Monthly,
    Named(String),
}

impl Kind {
    pub fn id(&self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Daily => "daily",
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
            Self::Named(_) => "named",
        }
    }

    /// The title the window carries to the consumers that know nothing of kinds (the crew screen, the old panel).
    pub fn title(&self) -> String {
        match self {
            Self::Session => "Session (5-hour)".into(),
            Self::Daily => "Daily".into(),
            Self::Weekly => "Weekly (7-day)".into(),
            Self::Monthly => "Monthly".into(),
            Self::Named(name) => name.clone(),
        }
    }

    /// How long the window usually runs, for judging its pace.
    #[allow(dead_code)]
    pub fn length(&self) -> Option<u64> {
        match self {
            Self::Session => Some(SESSION),
            Self::Daily => Some(DAY),
            Self::Weekly => Some(WEEK),
            Self::Monthly => Some(MONTH),
            Self::Named(_) => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub kind: Kind,
    /// Percent of the window used, clamped to 0..=100.
    pub used: f64,
    pub resets_at_ms: Option<u64>,
    /// How long the window lasts, when known, so its pace can be judged.
    pub length_ms: Option<u64>,
}

/// How a window's use compares with an even spend across it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pace {
    /// Percent an even spend would have used by now.
    pub expected: f64,
    /// When the window runs out at the rate used so far, as time from now, if that is before it resets.
    pub runs_out_ms: Option<u64>,
}

impl Window {
    pub fn new(kind: Kind, used: f64, resets_at_ms: Option<u64>, length_ms: Option<u64>) -> Self {
        let used = if used.is_finite() { used.clamp(0., 100.) } else { 0. };
        Self { kind, used, resets_at_ms, length_ms }
    }

    /// Nothing until a hundredth of the window has passed: an early estimate swings too far to be useful.
    #[allow(dead_code)]
    pub fn pace(&self, now: u64) -> Option<Pace> {
        pace(self.used, self.resets_at_ms?, self.length_ms?, now)
    }
}

pub fn pace(used: f64, resets_at_ms: u64, length_ms: u64, now: u64) -> Option<Pace> {
    if length_ms == 0 || resets_at_ms < now {
        return None;
    }
    let left = resets_at_ms - now;
    let elapsed = length_ms.checked_sub(left)?;
    if elapsed < length_ms / 100 {
        return None;
    }
    let expected = elapsed as f64 / length_ms as f64 * 100.;
    let runs_out_ms = (used > 0.)
        .then(|| {
            let rate = used / elapsed as f64;
            ((100. - used) / rate).round() as u64
        })
        .filter(|full| *full < left);
    Some(Pace { expected, runs_out_ms })
}

/// Money left, in the unit the service counts in.
#[derive(Clone, Debug, PartialEq)]
pub struct Balance {
    pub amount: f64,
    pub currency: String,
    /// What the amount is out of, when the service says.
    pub total: Option<f64>,
}

/// Anything else a service reports, in shapes the card knows how to draw.
#[derive(Clone, Debug, PartialEq)]
pub enum Section {
    /// A further limit that is not part of the headline windows (its pace is judged as the headline ones').
    Limit(Window),
    /// Label and value pairs under a heading.
    Facts { title: String, facts: Vec<(String, String)> },
    /// Shares of one whole, such as which surfaces spent a weekly limit.
    Shares { title: String, shares: Vec<(String, f64)> },
}

/// Who is signed in, as far as the sign-in and the service say.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Account {
    pub email: Option<String>,
    pub plan: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub provider: &'static str,
    pub account: Account,
    /// Session, then weekly, then model windows.
    pub windows: Vec<Window>,
    pub balances: Vec<Balance>,
    pub sections: Vec<Section>,
}

impl Report {
    pub fn new(provider: &'static str, account: Account, mut windows: Vec<Window>) -> Self {
        windows.sort_by(|a, b| a.kind.cmp(&b.kind));
        Self { provider, account, windows, balances: Vec::new(), sections: Vec::new() }
    }

    pub fn has_numbers(&self) -> bool {
        !self.windows.is_empty() || !self.balances.is_empty()
    }
}

/// `pro` to `Pro`, `max` to `Max`: plan identifiers read as names.
pub fn title_case(word: &str) -> String {
    let mut chars = word.trim().chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// `1250000` as `1,250,000`.
pub fn group(value: i64) -> String {
    let digits = value.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if value < 0 {
        out.push('-');
    }
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000_000;

    #[test]
    fn pace_of_the_session_in_the_screenshot() {
        // 7 % used, 1 h 33 m to the reset of a 5-hour window: 3 h 27 m have passed, 69 % of it, so 62 % are in reserve.
        let w = Window::new(Kind::Session, 7., Some(NOW + HOUR + 33 * MIN), Some(SESSION));
        let p = w.pace(NOW).unwrap();
        assert!((p.expected - 69.).abs() < 0.01, "{p:?}");
        assert_eq!((p.expected - w.used).round(), 62.);
        assert_eq!(p.runs_out_ms, None, "at this rate the window outlives its reset");
    }

    #[test]
    fn a_window_that_runs_out_says_when() {
        // 60 % used after one of five hours: the rest goes in 40 min, long before the 4 h that are left.
        let w = Window::new(Kind::Session, 60., Some(NOW + 4 * HOUR), Some(SESSION));
        let p = w.pace(NOW).unwrap();
        assert!((p.expected - 20.).abs() < 0.01);
        assert_eq!(p.runs_out_ms, Some(40 * MIN));
    }

    #[test]
    fn no_pace_until_a_hundredth_of_the_window_has_passed_or_without_a_length_or_a_reset() {
        let fresh = Window::new(Kind::Weekly, 1., Some(NOW + WEEK - MIN), Some(WEEK));
        assert_eq!(fresh.pace(NOW), None);
        assert_eq!(Window::new(Kind::Weekly, 1., Some(NOW + DAY), None).pace(NOW), None);
        assert_eq!(Window::new(Kind::Weekly, 1., None, Some(WEEK)).pace(NOW), None);
        // a reset already in the past says nothing about the allowance now
        assert_eq!(Window::new(Kind::Weekly, 1., Some(NOW - 1), Some(WEEK)).pace(NOW), None);
        // a reset further off than the window is long is nonsense, not a negative elapsed time
        assert_eq!(Window::new(Kind::Session, 1., Some(NOW + 2 * SESSION), Some(SESSION)).pace(NOW), None);
    }

    #[test]
    fn used_is_clamped_and_unused_never_runs_out() {
        assert_eq!(Window::new(Kind::Daily, 140., None, None).used, 100.);
        assert_eq!(Window::new(Kind::Daily, -5., None, None).used, 0.);
        assert_eq!(Window::new(Kind::Daily, f64::NAN, None, None).used, 0.);
        let idle = Window::new(Kind::Weekly, 0., Some(NOW + 3 * DAY), Some(WEEK));
        assert_eq!(idle.pace(NOW).unwrap().runs_out_ms, None);
    }

    #[test]
    fn windows_are_listed_session_then_weekly_then_named() {
        let r = Report::new(
            "x",
            Account::default(),
            vec![
                Window::new(Kind::Named("Fable".into()), 1., None, None),
                Window::new(Kind::Weekly, 1., None, None),
                Window::new(Kind::Session, 1., None, None),
            ],
        );
        let kinds: Vec<_> = r.windows.iter().map(|w| w.kind.id()).collect();
        assert_eq!(kinds, ["session", "weekly", "named"]);
    }

    #[test]
    fn grouping_and_titles() {
        assert_eq!(group(1250000), "1,250,000");
        assert_eq!(group(-12), "-12");
        assert_eq!(title_case("max"), "Max");
        assert_eq!(title_case(" "), "");
    }
}
