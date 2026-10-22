//! A marked block inside a board setting: the planner's own paragraph, replaced in place.
//!
//! Two things use it: the Galaxy's memory in `coordinator.instructions` (PLN-10) and the shipyards'
//! docker block in `worker.instructions` and `reviewer.instructions`. The human's own text stays as it
//! is; only the text between the markers is touched.

/// The markers of one block.
#[derive(Debug, Clone, Copy)]
pub struct Markers {
    pub begin: &'static str,
    pub end: &'static str,
}

fn block_range(text: &str, m: Markers) -> Option<(usize, usize)> {
    let a = text.find(m.begin)?;
    let b = text[a..].find(m.end)? + a + m.end.len();
    // the line break after the end marker goes with the block
    let b = if text[b..].starts_with('\n') { b + 1 } else { b };
    Some((a, b))
}

/// `text` with the block put in (replacing the one there) or, for `None`, taken out.
pub fn apply(text: &str, m: Markers, block: Option<&str>) -> String {
    match (block_range(text, m), block) {
        (Some((a, b)), Some(block)) => format!("{}{}{}", &text[..a], block, &text[b..]),
        (Some((a, b)), None) => {
            let head = text[..a].trim_end_matches('\n');
            let tail = &text[b..];
            if head.is_empty() {
                tail.to_string()
            } else if tail.is_empty() {
                format!("{head}\n")
            } else {
                format!("{head}\n\n{tail}")
            }
        }
        (None, Some(block)) => {
            let head = text.trim_end_matches('\n');
            if head.is_empty() {
                block.to_string()
            } else {
                format!("{head}\n\n{block}")
            }
        }
        (None, None) => text.to_string(),
    }
}

pub fn has_block(text: &str, m: Markers, block: &str) -> bool {
    block_range(text, m).is_some_and(|(a, b)| text[a..b].trim_end() == block.trim_end())
}

pub fn has_any_block(text: &str, m: Markers) -> bool {
    block_range(text, m).is_some()
}

/// The setting's value with the block added at the end (or the old block replaced).
pub fn with_block(current: &str, m: Markers, block: &str) -> String {
    apply(current.trim_end(), m, Some(block)).trim_end().to_string()
}

/// The value `herdr-projects set` is given to switch the block on or off. `Ok(None)`: unset.
/// A value that starts with «-» and a word would be read as an option by the board, so it is refused.
pub fn connect_value(
    current: &str,
    m: Markers,
    block: &str,
    on: bool,
) -> Result<Option<String>, String> {
    let value = if on {
        Some(with_block(current, m, block))
    } else {
        without_block(current, m)
    };
    if let Some(v) = &value {
        let mut chars = v.chars();
        if chars.next() == Some('-') && chars.next().is_some_and(|c| !c.is_whitespace()) {
            return Err("инструкции начинаются с «-» и слова после него: доска могла бы принять их за параметр. Начните первую строку иначе (например, с «Мои правила:»).".into());
        }
    }
    Ok(value)
}

/// The text without the block; `None` when nothing else is left (the setting is then unset).
pub fn without_block(current: &str, m: Markers) -> Option<String> {
    let text = apply(current, m, None).trim_end().to_string();
    (!text.trim().is_empty()).then_some(text)
}

/// Whether a paragraph outside the block mentions `needle` (the old hand-written docker rule).
pub fn has_paragraph(text: &str, m: Markers, needle: &str) -> bool {
    let (head, tail) = match block_range(text, m) {
        Some((a, b)) => (&text[..a], &text[b..]),
        None => (text, ""),
    };
    head.split("\n\n")
        .chain(tail.split("\n\n"))
        .any(|p| p.contains(needle))
}

/// Drops every paragraph outside the block that mentions `needle`; the rest of the text stays as it is.
pub fn drop_paragraphs(text: &str, m: Markers, needle: &str) -> String {
    let keep = |part: &str| -> String {
        part.split("\n\n")
            .filter(|p| !p.contains(needle))
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    let Some((a, b)) = block_range(text, m) else {
        return keep(text);
    };
    let h = keep(&text[..a]);
    let t = keep(&text[b..]);
    let block = &text[a..b];
    let mut out = h.trim_end().to_string();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(block.trim_end_matches('\n'));
    let t = t.trim_start_matches('\n');
    if !t.is_empty() {
        out.push_str("\n\n");
        out.push_str(t.trim_end());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const M: Markers = Markers { begin: "<!-- b -->", end: "<!-- /b -->" };

    #[test]
    fn a_block_is_added_replaced_and_removed_without_touching_the_human() {
        assert_eq!(apply("Мои правила.", M, Some("<!-- b -->\nX\n<!-- /b -->\n")), "Мои правила.\n\n<!-- b -->\nX\n<!-- /b -->\n");
        let on = with_block("Мои правила.", M, "<!-- b -->\nX\n<!-- /b -->\n");
        let again = with_block(&on, M, "<!-- b -->\nY\n<!-- /b -->\n");
        assert_eq!(again.matches("<!-- b -->").count(), 1);
        assert!(again.contains("Y") && !again.contains("X"));
        assert_eq!(without_block(&on, M).as_deref(), Some("Мои правила."));
        assert_eq!(without_block("<!-- b -->\nX\n<!-- /b -->\n", M), None);
    }

    #[test]
    fn an_unfinished_block_is_repaired() {
        let broken = "Мои правила.\n<!-- b -->\nобрыв\n";
        let fixed = with_block(broken, M, "<!-- b -->\nX\n<!-- /b -->\n");
        assert_eq!(fixed.matches("<!-- /b -->").count(), 1);
        assert!(fixed.starts_with("Мои правила."));
    }

    #[test]
    fn old_paragraphs_are_seen_and_dropped_only_outside_the_block() {
        let text = "Правило первое.\n\nНе запускай контейнеры тут: DOCKER_HOST=ssh://x и всё.\n\nПравило третье.\n\n<!-- b -->\nDOCKER_HOST=in block\n<!-- /b -->\n";
        assert!(has_paragraph(text, M, "DOCKER_HOST="));
        let out = drop_paragraphs(text, M, "DOCKER_HOST=");
        assert!(!out.contains("тут"));
        assert!(out.contains("Правило первое.") && out.contains("Правило третье."));
        assert!(out.contains("DOCKER_HOST=in block"));
    }

    #[test]
    fn a_value_that_looks_like_an_option_is_refused() {
        assert!(connect_value("-x", M, "block", true).is_err());
        assert!(connect_value("- список", M, "block", true).is_ok());
    }
}
