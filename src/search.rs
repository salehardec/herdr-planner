//! Search across the planner: the documents of the boards (galaxies, planets,
//! tasks, ...) and the Coordinator's conversation, in one ranked list.
//!
//! Text fields of a [`Hit`] are plain, except `snippet`, which is HTML: the text is
//! escaped and every matched word is wrapped in `<mark>`. The page can insert it as
//! HTML without further escaping.

use crate::db::{query_terms, ChatHit, Db, DocHit};
use serde::Serialize;
use std::collections::HashSet;

/// The path shown for a chat hit.
pub const CHAT_PATH: &str = "Чат Координатора";
/// The kind of a chat hit.
pub const CHAT_KIND: &str = "chat";
const SNIPPET_WIDTH: usize = 160;
const TITLE_CHARS: usize = 80;
const MAX_LIMIT: usize = 50;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Hit {
    pub kind: String,
    pub id: String,
    pub galaxy: String,
    pub path: String,
    pub title: String,
    /// HTML, see the module docs.
    pub snippet: String,
    pub status: String,
    pub at: u64,
    /// FTS5 bm25 (lower is better); 0 for matches found by `LIKE`.
    pub rank: f64,
}

/// The best `limit` hits (at most 50) for `q`, best first. Chat hits have kind
/// `chat`, and their id is the turn number.
pub fn query(db: &Db, q: &str, limit: usize) -> Vec<Hit> {
    let terms = query_terms(q);
    if terms.is_empty() {
        return Vec::new();
    }
    let limit = limit.clamp(1, MAX_LIMIT);
    let mut hits: Vec<Hit> = db
        .search_docs_match(q, limit)
        .into_iter()
        .map(|h| doc_hit(h, &terms))
        .collect();
    hits.extend(db.chat_search(q, limit).into_iter().map(chat_hit));
    hits.sort_by(|a, b| a.rank.total_cmp(&b.rank).then(b.at.cmp(&a.at)));
    let mut seen = HashSet::new();
    hits.retain(|h| seen.insert((h.kind.clone(), h.id.clone())));
    hits.truncate(limit);
    hits
}

fn doc_hit(h: DocHit, terms: &[String]) -> Hit {
    let d = h.doc;
    let snippet = snippet_for(&[&d.text, &d.title], terms);
    Hit {
        kind: d.kind,
        id: d.id,
        galaxy: d.galaxy,
        path: d.path,
        title: d.title,
        snippet,
        status: d.status,
        at: d.at,
        rank: h.rank,
    }
}

fn chat_hit(h: ChatHit) -> Hit {
    let first_line: String =
        h.q.lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(TITLE_CHARS)
            .collect();
    let snippet = if h.q_snip.contains("<mark>") {
        h.q_snip
    } else {
        h.a_snip
    };
    Hit {
        kind: CHAT_KIND.into(),
        id: h.n.to_string(),
        galaxy: String::new(),
        path: CHAT_PATH.into(),
        title: first_line,
        snippet,
        status: String::new(),
        at: h.at,
        rank: h.rank,
    }
}

/// The first field whose text contains a match; otherwise the first non-empty one.
fn snippet_for(fields: &[&str], terms: &[String]) -> String {
    let marked: Vec<String> = fields
        .iter()
        .map(|f| highlight(f, terms, SNIPPET_WIDTH))
        .collect();
    marked
        .iter()
        .find(|s| s.contains("<mark>"))
        .or_else(|| marked.iter().find(|s| !s.is_empty()))
        .cloned()
        .unwrap_or_default()
}

/// Case-insensitive, per character (Cyrillic included).
fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

fn push_escaped(out: &mut String, chars: &[char]) {
    for &c in chars {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            '\n' | '\r' | '\t' => out.push(' '),
            c => out.push(c),
        }
    }
}

/// HTML of a window of `text` of about `width` characters around its first match of
/// any of `terms`, with every match in `<mark>`. Without a match, the text's start.
/// The text is escaped; `…` marks cut ends.
pub fn highlight(text: &str, terms: &[String], width: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    let total = chars.len();
    let folded: Vec<char> = chars.iter().map(|&c| fold(c)).collect();
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for term in terms {
        let needle: Vec<char> = term.chars().map(fold).collect();
        if needle.is_empty() || needle.len() > total {
            continue;
        }
        for i in 0..=total - needle.len() {
            if folded[i..i + needle.len()] == needle[..] {
                spans.push((i, i + needle.len()));
            }
        }
    }
    spans.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (s, e) in spans {
        match merged.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => merged.push((s, e)),
        }
    }
    let (start, end) = match merged.first() {
        None => (0, width.min(total)),
        Some(&(s, _)) => {
            let start = s.saturating_sub(width / 3);
            (start, (start + width).min(total))
        }
    };
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    let mut pos = start;
    for &(s, e) in &merged {
        if e <= start || s >= end {
            continue;
        }
        let (s, e) = (s.max(start), e.min(end));
        push_escaped(&mut out, &chars[pos..s]);
        out.push_str("<mark>");
        push_escaped(&mut out, &chars[s..e]);
        out.push_str("</mark>");
        pos = e;
    }
    push_escaped(&mut out, &chars[pos..end]);
    if end < total {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms(s: &str) -> Vec<String> {
        query_terms(s)
    }

    #[test]
    fn marks_every_match_case_insensitively_and_escapes() {
        let h = highlight("Удалил <b>Задачи</b> и задачу & всё", &terms("задач"), 200);
        assert_eq!(
            h,
            "Удалил &lt;b&gt;<mark>Задач</mark>и&lt;/b&gt; и <mark>задач</mark>у &amp; всё"
        );
    }

    #[test]
    fn cuts_a_window_around_the_first_match() {
        let long = format!("{}нужное слово{}", "а".repeat(300), "б".repeat(300));
        let h = highlight(&long, &terms("нужное"), 40);
        assert!(h.starts_with('…') && h.ends_with('…'), "{h}");
        assert!(h.contains("<mark>нужное</mark>"), "{h}");
        assert!(h.chars().count() < 60, "{h}");
    }

    #[test]
    fn without_a_match_gives_the_start() {
        let h = highlight("короткий текст", &terms("нет"), 5);
        assert_eq!(h, "корот…");
        assert_eq!(highlight("", &terms("x"), 5), "");
    }

    #[test]
    fn newlines_become_spaces() {
        assert_eq!(highlight("a\nb", &[], 10), "a b");
    }

    #[test]
    fn query_ranks_documents_and_chat_together() {
        let db = Db::memory().unwrap();
        db.search_docs_replace(
            "",
            &[crate::db::SearchDoc {
                key: "APC-7".into(),
                kind: "planet".into(),
                id: "APC-7".into(),
                galaxy: String::new(),
                path: "alpha › APC".into(),
                title: "Каталог API".into(),
                text: "описание: переписать каталог".into(),
                status: "идёт".into(),
                at: 5,
            }],
        );
        db.chat_turns_replace_from(
            0,
            &[crate::db::ChatTurnRow {
                n: 3,
                at: 9,
                from: "human".into(),
                q: "что с каталогом?".into(),
                a: "Каталог переписан.".into(),
            }],
        );
        let hits = query(&db, "каталог", 10);
        assert_eq!(hits.len(), 2);
        let planet = hits.iter().find(|h| h.kind == "planet").unwrap();
        assert_eq!(
            (planet.id.as_str(), planet.status.as_str()),
            ("APC-7", "идёт")
        );
        assert!(
            planet.snippet.contains("<mark>каталог</mark>"),
            "{}",
            planet.snippet
        );
        let chat = hits.iter().find(|h| h.kind == CHAT_KIND).unwrap();
        assert_eq!(chat.id, "3");
        assert_eq!(chat.path, CHAT_PATH);
        assert_eq!(chat.title, "что с каталогом?");
        assert!(chat.snippet.contains("<mark>"), "{}", chat.snippet);
    }

    #[test]
    fn query_is_limited_and_safe() {
        let db = Db::memory().unwrap();
        let docs: Vec<crate::db::SearchDoc> = (0..70)
            .map(|i| crate::db::SearchDoc {
                key: format!("k{i}"),
                kind: "planet".into(),
                id: format!("k{i}"),
                title: format!("задача {i:02}"),
                at: i,
                ..Default::default()
            })
            .collect();
        db.search_docs_replace("", &docs);
        assert_eq!(query(&db, "задача", 500).len(), MAX_LIMIT);
        assert_eq!(query(&db, "задача", 3).len(), 3);
        assert_eq!(
            query(&db, "задача", 3)[0].id,
            "k69",
            "same rank: newest first"
        );
        assert!(query(&db, "   ", 10).is_empty());
        assert!(query(&db, "\"", 10).is_empty());
        assert!(query(&db, "NEAR(", 10).is_empty());
        assert_eq!(query(&db, "OR", 10).len(), 0);
    }
}
