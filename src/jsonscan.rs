//! JSON an agent wrapped in prose or code fences: the freshness verdicts and
//! the group suggestion both read their answers through here.

use serde_json::Value;

/// Index of the bracket closing the one at `i`, strings and escapes respected;
/// none when it never closes.
fn closing_bracket(text: &str, i: usize) -> Option<usize> {
    let (mut depth, mut in_str, mut esc) = (0i32, false, false);
    for (k, &c) in text.as_bytes().iter().enumerate().skip(i) {
        if in_str {
            if esc {
                esc = false;
            } else if c == b'\\' {
                esc = true;
            } else if c == b'"' {
                in_str = false;
            }
        } else if c == b'"' {
            in_str = true;
        } else if c == b'[' || c == b'{' {
            depth += 1;
        } else if c == b']' || c == b'}' {
            depth -= 1;
            if depth == 0 {
                return Some(k);
            }
        }
    }
    None
}

/// `text` without its code fences (```` ``` ```` and the language word after it).
fn strip_fences(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("```") {
        out.push_str(&rest[..at]);
        rest = rest[at + 3..].trim_start_matches(|c: char| c.is_alphanumeric() || c == '_');
    }
    out.push_str(rest);
    out
}

/// Every JSON array or object in `text`, left to right, the whole text first.
/// Prose and fences around them are ignored; at most twelve are returned.
pub fn candidates(text: &str) -> Vec<Value> {
    let text = strip_fences(text);
    let mut out: Vec<Value> = Vec::new();
    let is_container = |v: &Value| v.is_array() || v.is_object();
    if let Ok(v) = serde_json::from_str::<Value>(text.trim()) {
        if is_container(&v) {
            out.push(v);
        }
    }
    let mut pos = 0;
    while out.len() < 12 {
        let Some(off) = text[pos..].find(['[', '{']) else {
            break;
        };
        let i = pos + off;
        let parsed = closing_bracket(&text, i).and_then(|j| {
            serde_json::from_str::<Value>(&text[i..=j])
                .ok()
                .map(|v| (j, v))
        });
        match parsed {
            Some((j, v)) if is_container(&v) => {
                out.push(v);
                pos = j + 1;
            }
            _ => pos = i + 1,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_json_in_prose_and_fences() {
        let c = candidates("Вот ответ:\n```json\n[{\"n\":1,\"v\":\"done\"}]\n```\nГотово.");
        assert!(c.iter().any(|v| v.is_array() && v[0]["n"] == 1));
        let whole = candidates("[1,2]");
        assert_eq!(whole[0], serde_json::json!([1, 2]));
        assert!(candidates("нет json").is_empty());
    }

    #[test]
    fn strings_may_hold_brackets() {
        let c = candidates(r#"текст {"why":"закрывает ] скобку \" и { ещё"} конец"#);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0]["why"], "закрывает ] скобку \" и { ещё");
    }

    #[test]
    fn broken_json_is_skipped_and_the_next_is_found() {
        let c = candidates("[1, 2 {\"a\": 1} хвост");
        assert!(c.iter().any(|v| v["a"] == 1));
    }
}
