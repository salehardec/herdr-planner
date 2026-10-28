//! The lint of memory entries: what the panel marks for the human to look at again. Nothing here changes or removes an
//! entry; a mark is a suggestion («обновить / удалить»), and the human decides.
//!
//! * **overdue** — the header's `review: YYYY-MM-DD` is today or earlier (a date that is not a date is marked too:
//!   `bad_review`);
//! * **oneoff** — probably a one-off: a task or pull request number, a dated fix, «починил …», a snapshot of the state
//!   («ждёт слияния», «в данный момент»). `SUGGEST_PROMPT` asks the model not to write such things, but the human's own
//!   notes and an agent's files get them anyway. A `review:` date in the future silences it: the human has said when
//!   to look again;
//! * **old** — a decision or a pitfall that came from the chat and is older than `OLD_DAYS`, with no `review:` date;
//! * **duplicate** — another entry with the same title, or nearly the same words in title and description.

use crate::memory::Entry;
use serde::Serialize;

/// A decision or pitfall from the chat is asked about again after this many days.
pub const OLD_DAYS: i64 = 60;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Lint {
    /// `overdue`, `bad_review`, `oneoff`, `old`, `duplicate`.
    pub kind: &'static str,
    /// What was seen, for the human: the date, the words that gave it away.
    pub detail: String,
    /// The other entry of a `duplicate`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub with: Option<String>,
}

fn lint(kind: &'static str, detail: impl Into<String>) -> Lint {
    Lint { kind, detail: detail.into(), with: None }
}

/// `YYYY-MM-DD` that is a calendar day (the month and day ranges are checked, the length of the month is not).
pub fn valid_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
        && matches!(s[5..7].parse::<u32>(), Ok(1..=12))
        && matches!(s[8..10].parse::<u32>(), Ok(1..=31))
}

/// Days since 1970-01-01 of a valid `YYYY-MM-DD` (the same civil calendar as `memory::date_of`, the other way round).
pub fn days_of(date: &str) -> Option<i64> {
    if !valid_date(date) {
        return None;
    }
    let (y, m, d): (i64, i64, i64) = (date[..4].parse().ok()?, date[5..7].parse().ok()?, date[8..10].parse().ok()?);
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

/// `PLN-31`, `ALP-4`: two to six capitals, a dash, digits.
fn has_task_id(text: &str) -> Option<String> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')).find_map(|w| {
        let (head, num) = w.split_once('-')?;
        let ok = (2..=6).contains(&head.len()) && head.chars().all(|c| c.is_ascii_uppercase()) && !num.is_empty() && num.chars().all(|c| c.is_ascii_digit());
        ok.then(|| w.to_string())
    })
}

/// «задача 12», «задачей №12», «PR #45», «pull request 45»: a word and a number after it.
fn has_numbered(low: &str, words: &[&str]) -> Option<String> {
    for w in words {
        let mut from = 0;
        while let Some(i) = low[from..].find(w) {
            let at = from + i + w.len();
            let rest: String = low[at..].chars().skip_while(|c| c.is_alphabetic() || *c == ' ' || *c == '№' || *c == '#').take_while(|c| c.is_ascii_digit()).collect();
            let gap = low[at..].chars().take_while(|c| c.is_alphabetic() || *c == ' ' || *c == '№' || *c == '#').count();
            if !rest.is_empty() && gap <= 6 {
                return Some(format!("{w} {rest}"));
            }
            from = at;
        }
    }
    None
}

/// A date in the text (`2026-10-22`), except at the start of the line it is only a mention: any in the body counts.
fn has_date(text: &str) -> Option<String> {
    let b = text.as_bytes();
    (0..b.len().saturating_sub(9)).find_map(|i| {
        let s = text.get(i..i + 10)?;
        let before = i == 0 || !b[i - 1].is_ascii_digit();
        let after = b.get(i + 10).map_or(true, |c| !c.is_ascii_digit());
        (before && after && valid_date(s) && s.starts_with("20")).then(|| s.to_string())
    })
}

const FIXED: [&str; 9] = ["починил", "починено", "починен", "исправил", "исправлено", "поправил", "пофиксил", "fixed", "заработал"];
const SNAPSHOT: [&str; 10] = [
    "ждёт слияния", "ждет слияния", "ожидает слияния", "в данный момент", "на данный момент", "на сегодня", "удаляется задачей", "удаляются задачей", "стоят в очереди", "пока не слит",
];

/// Why an entry reads like a one-off, in words (empty when it does not).
fn oneoff_reasons(e: &Entry) -> Vec<String> {
    let text = format!("{}\n{}\n{}", e.title, e.description, e.body);
    let low = text.to_lowercase();
    let mut out = Vec::new();
    if let Some(id) = has_task_id(&text) {
        out.push(format!("номер задачи {id}"));
    } else if let Some(n) = has_numbered(&low, &["задач", "pull request", "pr "]) {
        out.push(format!("номер задачи или PR ({n})"));
    }
    if let Some(d) = has_date(&format!("{}\n{}", e.title, e.body)).or_else(|| has_date(&e.description)) {
        out.push(format!("дата в тексте {d}"));
    }
    let title = e.title.to_lowercase();
    let id = e.id.to_lowercase();
    if let Some(w) = FIXED.iter().find(|w| title.starts_with(**w) || id.starts_with(&translit_stem(w))) {
        out.push(format!("разовая починка («{w}»)"));
    }
    if let Some(w) = SNAPSHOT.iter().find(|w| low.contains(**w)) {
        out.push(format!("снимок состояния («{w}»)"));
    }
    out
}

/// The file-name stem for a Russian word (`pochinil`): what `memory::slugify` would make of it.
fn translit_stem(word: &str) -> String {
    crate::memory::slugify(word)
}

/// The words of a title and description, long enough to mean something, cut to five letters so endings do not matter.
fn words_of(e: &Entry) -> Vec<String> {
    let text = format!("{} {}", e.title, e.description).to_lowercase();
    let mut w: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 3)
        .map(|w| w.chars().take(5).collect())
        .collect();
    w.sort();
    w.dedup();
    w
}

fn same_title(a: &Entry, b: &Entry) -> bool {
    let n = |e: &Entry| e.title.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ");
    let (x, y) = (n(a), n(b));
    !x.is_empty() && x == y
}

fn similar(a: &[String], b: &[String]) -> bool {
    let both = a.iter().filter(|w| b.contains(w)).count();
    let all = a.len() + b.len() - both;
    both >= 3 && all > 0 && both * 100 / all >= 70
}

/// More entries than this are not compared pairwise (a folder of that size is not a memory a person reads).
const DUP_LIMIT: usize = 600;

/// The marks for a list of entries, `today` being `YYYY-MM-DD`. Only entries that have something to say are in the
/// answer, in the order of the list. Entries `skip` names (being deleted) are not looked at and not compared.
pub fn lint_all(entries: &[Entry], today: &str, skip: impl Fn(&Entry) -> bool) -> Vec<(String, Vec<Lint>)> {
    let live: Vec<&Entry> = entries.iter().filter(|e| !skip(e)).collect();
    let today_days = days_of(today);
    let words: Vec<Vec<String>> = if live.len() <= DUP_LIMIT { live.iter().map(|e| words_of(e)).collect() } else { Vec::new() };
    let mut out = Vec::new();
    for (i, e) in live.iter().enumerate() {
        let mut marks: Vec<Lint> = Vec::new();
        let review = e.review.trim();
        let mut future_review = false;
        if !review.is_empty() {
            match (days_of(review), today_days) {
                (None, _) => marks.push(lint("bad_review", format!("review: {review} — не дата ГГГГ-ММ-ДД"))),
                (Some(r), Some(t)) if r <= t => marks.push(lint("overdue", format!("пересмотреть до {review}"))),
                (Some(_), _) => future_review = true,
            }
        }
        if !future_review {
            let why = oneoff_reasons(e);
            if !why.is_empty() {
                marks.push(lint("oneoff", why.join("; ")));
            }
        }
        if review.is_empty() && matches!(e.kind.as_str(), "decision" | "pitfall") && e.source.trim() == "chat" {
            if let (Some(a), Some(t)) = (days_of(&e.added), today_days) {
                if t - a > OLD_DAYS {
                    marks.push(lint("old", format!("записано {} ({} дн. назад), срока пересмотра нет", e.added, t - a)));
                }
            }
        }
        if !words.is_empty() {
            for (j, o) in live.iter().enumerate() {
                if i != j && (same_title(e, o) || similar(&words[i], &words[j])) {
                    marks.push(Lint { kind: "duplicate", detail: format!("похожа на «{}»", o.title), with: Some(o.id.clone()) });
                    break;
                }
            }
        }
        if !marks.is_empty() {
            out.push((e.id.clone(), marks));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, title: &str, desc: &str, kind: &str, body: &str) -> Entry {
        Entry { id: id.into(), title: title.into(), description: desc.into(), kind: kind.into(), body: body.into(), added: "2026-10-20".into(), source: "chat".into(), ..Entry::default() }
    }

    fn marks(es: &[Entry], id: &str) -> Vec<&'static str> {
        lint_all(es, "2026-10-28", |_| false).into_iter().find(|(i, _)| i == id).map(|(_, m)| m.into_iter().map(|l| l.kind).collect()).unwrap_or_default()
    }

    #[test]
    fn days_are_counted_across_months_and_leap_years() {
        assert_eq!(days_of("1970-01-01"), Some(0));
        assert_eq!(days_of("2026-10-28").unwrap() - days_of("2026-10-22").unwrap(), 6);
        assert_eq!(days_of("2024-03-01").unwrap() - days_of("2024-02-28").unwrap(), 2);
        assert_eq!(days_of("2027-01-01").unwrap() - days_of("2026-01-01").unwrap(), 365);
        // the same calendar as `memory::date_of`
        let d = days_of("2026-10-28").unwrap();
        assert_eq!(crate::memory::date_of(d as u64 * 86_400_000), "2026-10-28");
        assert_eq!(days_of("2026-13-01"), None);
        assert_eq!(days_of("вчера"), None);
    }

    #[test]
    fn a_review_date_that_has_come_is_overdue_and_one_in_the_future_is_not() {
        let mut a = entry("a", "Порты базы", "Локальный Postgres слушает 5433", "pitfall", "Не 5432.");
        a.review = "2026-10-28".into();
        assert_eq!(marks(&[a.clone()], "a"), vec!["overdue"]);
        a.review = "2026-10-01".into();
        assert_eq!(marks(&[a.clone()], "a"), vec!["overdue"]);
        a.review = "2027-01-01".into();
        assert!(marks(&[a.clone()], "a").is_empty());
        a.review = "скоро".into();
        assert_eq!(marks(&[a], "a"), vec!["bad_review"]);
    }

    #[test]
    fn what_reads_like_a_one_off_is_marked_with_the_reason() {
        let es = [
            entry("no-far-galaxies", "Без далёких галактик", "Решение о других машинах", "decision", "Далёкие галактики не нужны (удаляются задачей PLN-31). Другие машины — раннеры."),
            entry("board-plan-bug", "Застревание терминала", "Терминал застревает", "pitfall", "Из-за ble.sh. Исправлено 2026-10-22: `bleopt accept_line_threshold=-1`."),
            entry("pochinil-doska-snova-otvechaet-board-list", "Починил: доска снова отвечает", "board list работает", "pitfall", "Доска снова отвечает."),
            entry("wait", "Слияние", "PR открыт", "decision", "Правка ждёт слияния, в данный момент в очереди."),
            entry("ports", "Порты базы", "Postgres слушает 5433, а не 5432", "pitfall", "Так настроен локальный сервер; клиенты берут порт из окружения."),
        ];
        let by = |id: &str| lint_all(&es, "2026-10-28", |_| false).into_iter().find(|(i, _)| i == id).map(|(_, m)| m).unwrap_or_default();
        assert!(by("no-far-galaxies")[0].detail.contains("PLN-31"));
        assert!(by("board-plan-bug").iter().any(|l| l.kind == "oneoff" && l.detail.contains("2026-10-22")));
        assert!(by("pochinil-doska-snova-otvechaet-board-list").iter().any(|l| l.kind == "oneoff" && l.detail.contains("починка")));
        assert!(by("wait").iter().any(|l| l.kind == "oneoff" && l.detail.contains("снимок состояния")));
        assert!(by("ports").is_empty(), "{:?}", by("ports"));
    }

    #[test]
    fn task_and_pr_numbers_are_found_in_words_too() {
        for body in ["Это делает задача 12.", "Сделано задачей №7.", "см. PR #45", "pull request 45 уже слит"] {
            let e = entry("x", "Что-то", "Что-то важное", "decision", body);
            assert_eq!(marks(&[e], "x"), vec!["oneoff"], "{body}");
        }
        // a word that only starts like one is not a number
        let e = entry("y", "Задачи доски", "Как устроены задачи", "convention", "Каждая задача имеет свою ветку и свою рабочую копию.");
        assert!(marks(&[e], "y").is_empty());
    }

    #[test]
    fn a_future_review_date_silences_the_one_off_mark() {
        let mut e = entry("a", "Без далёких галактик", "Решение", "decision", "Удаляются задачей PLN-31.");
        assert_eq!(marks(&[e.clone()], "a"), vec!["oneoff"]);
        e.review = "2027-01-01".into();
        assert!(marks(&[e.clone()], "a").is_empty());
        e.review = "2026-10-01".into();
        assert_eq!(marks(&[e], "a"), vec!["overdue", "oneoff"]);
    }

    #[test]
    fn an_old_decision_from_the_chat_without_a_review_date_is_asked_about() {
        let mut e = entry("a", "Дешёвая модель", "Какую брать", "decision", "Берём flash.");
        e.added = "2026-08-01".into();
        assert_eq!(marks(&[e.clone()], "a"), vec!["old"]);
        e.added = "2026-10-01".into();
        assert!(marks(&[e.clone()], "a").is_empty());
        e.added = "2026-08-01".into();
        e.kind = "rule".into();
        assert!(marks(&[e.clone()], "a").is_empty(), "a rule does not age by the calendar");
        e.kind = "decision".into();
        e.source = "task herdr-planner/14".into();
        assert!(marks(&[e.clone()], "a").is_empty());
        e.source = "chat".into();
        e.review = "2027-01-01".into();
        assert!(marks(&[e], "a").is_empty());
    }

    #[test]
    fn the_same_title_or_nearly_the_same_words_are_duplicates_of_each_other() {
        let es = [
            entry("a", "GitLab — без PR", "В репо на GitLab слияние идёт без pull request", "rule", "…"),
            entry("b", "gitlab без pr", "Другое описание", "rule", "…"),
            entry("c", "Порты базы данных", "Локальный Postgres слушает порт 5433", "pitfall", "…"),
            entry("d", "Порты базы данных локально", "Локальный Postgres слушает порт 5433", "pitfall", "…"),
            entry("e", "Модель для ревью", "Ревью делает дешёвая модель", "rule", "…"),
        ];
        let all = lint_all(&es, "2026-10-28", |_| false);
        let dup = |id: &str| all.iter().find(|(i, _)| i == id).and_then(|(_, m)| m.iter().find(|l| l.kind == "duplicate")).and_then(|l| l.with.clone());
        assert_eq!(dup("a").as_deref(), Some("b"));
        assert_eq!(dup("b").as_deref(), Some("a"));
        assert_eq!(dup("c").as_deref(), Some("d"));
        assert_eq!(dup("d").as_deref(), Some("c"));
        assert_eq!(dup("e"), None);
    }

    #[test]
    fn entries_being_deleted_are_neither_marked_nor_compared() {
        let mut a = entry("a", "Порты базы", "Postgres слушает 5433", "pitfall", "…");
        a.review = "2026-01-01".into();
        let b = entry("b", "Порты базы", "Postgres слушает 5433", "pitfall", "…");
        let all = lint_all(&[a, b], "2026-10-28", |e| e.id == "a");
        assert!(all.is_empty(), "{all:?}");
    }
}
