//! A finished thread's report (`threads/t-NNNN.md`): the first line that says
//! what came of it.

use super::tasks::{clip, strip_md};

const HEADINGS: [&str; 4] = ["Report", "Отчёт", "Итог", "Результат"];

/// The text under the first `## Report` / `## Отчёт` / … heading, else
/// whatever precedes `## Next`, else the whole file.
fn body(md: &str) -> String {
    let md = format!("\n{md}\n## ");
    for h in HEADINGS {
        let start = format!("\n## {h}");
        if let Some(at) = md.find(&start) {
            let after = &md[at + start.len()..];
            if let Some(nl) = after.find('\n') {
                let after = &after[nl + 1..];
                let end = after.find("\n## ").unwrap_or(after.len());
                return after[..end].to_string();
            }
        }
    }
    let end = md.find("\n## Next").unwrap_or(md.len());
    md[..end].to_string()
}

/// First line of a thread's report that says what came of it: links,
/// headings, tables and bare labels like "Что сделано." (under 24 characters)
/// are skipped.
pub fn summary(md: &str) -> Option<String> {
    let mut fence = false;
    for line in body(md).lines() {
        if line.trim_start().starts_with("```") {
            fence = !fence;
            continue;
        }
        if fence {
            continue;
        }
        let s = strip_md(
            line.trim_start_matches(|c: char| c.is_whitespace() || matches!(c, '-' | '*' | '>')),
        );
        let bare_label = s.chars().count() < 24 && (s.ends_with('.') || s.ends_with(':'));
        let link_line = s.starts_with("http://")
            || s.starts_with("https://")
            || s.split_once(':').is_some_and(|(k, v)| {
                k.chars().all(|c| c.is_alphabetic() || c == ' ')
                    && (v.trim_start().starts_with("http://")
                        || v.trim_start().starts_with("https://"))
            });
        if s.is_empty()
            || s.starts_with('#')
            || s.starts_with('|')
            || s.starts_with("---")
            || link_line
            || bare_label
        {
            continue;
        }
        return Some(clip(&s, 1200));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_the_first_telling_line() {
        let md = "PR: https://github.com/o/r/pull/1\n\n## Report\n\n**Что сделано.**\n\nПлагин готов и закоммичен, PR слит.\n\n- пункт\n\n## Next\n\nСлить\n";
        assert_eq!(
            summary(md).as_deref(),
            Some("Плагин готов и закоммичен, PR слит.")
        );
    }

    #[test]
    fn free_form_report_stops_at_next() {
        let md = "# Заголовок\nСделал всё как просили, проверил тестами.\n## Next\nСлить PR\n";
        assert_eq!(
            summary(md).as_deref(),
            Some("Сделал всё как просили, проверил тестами.")
        );
        assert_eq!(summary("## Next\nСлить\n"), None);
        assert_eq!(summary("```\ncode only\n```\n"), None);
        assert_eq!(summary(""), None);
    }
}
