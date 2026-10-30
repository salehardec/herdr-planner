//! Selection menus in an agent's terminal: what the screen asks, what the choices are and which keys give a choice.
//!
//! A program that waits for a choice draws a list and a hint line: Claude Code's `AskUserQuestion`
//! (`❯ 1. date-fns` with the description under it, a tab strip for several questions, `[ ]` boxes for a
//! multiple choice), Codex's approvals (`› 1. Yes, proceed`), omp's `ask` (a box with glyph rows). The page shows
//! such a screen as a card with buttons; this module reads the plain text of the screen (what `herdr agent read`
//! gives) and turns a choice into the keys (`down`, `enter`, …) and the text that make it. Nothing here touches
//! herdr: a screen that is not a menu gives `None`, and the page keeps showing the screen and the line for an answer.

use serde::Serialize;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// One choice the page offers as a button.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MenuOption {
    /// The row in the list (counted from the first one): where the cursor has to go for this choice.
    pub at: usize,
    pub label: String,
    /// The line under the label, if the program draws one.
    pub desc: String,
    /// A multiple choice: the box is ticked now.
    pub checked: bool,
    /// «Type something» / «Other»: the choice that takes the person's own words.
    pub own: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Menu {
    /// `claude`, `codex`, `omp` or `other`: how the program takes its own answer is the only difference.
    pub program: &'static str,
    /// Changes with the question and the choices, never with the cursor, the ticks or the typed text.
    pub sig: String,
    pub question: String,
    /// What the screen says around the question (the command to run, the answers so far): shown on demand.
    pub context: String,
    pub options: Vec<MenuOption>,
    pub multi: bool,
    /// The row the program's cursor is on now.
    pub cursor: usize,
    /// The row of the «Submit» line a multiple choice has (Claude Code): `Enter` on it sends the ticks.
    pub submit_at: Option<usize>,
    /// «Other» opens an editor of its own (omp): `Enter` on the row, then the text, then `Enter`.
    pub own_editor: bool,
    /// Which question of several this is (0 when the screen does not say).
    pub step: usize,
    pub steps: usize,
    /// The tab strip of a several-questions form, the question labels.
    pub tabs: Vec<String>,
}

/// What the person chose. `at` is a row of `Menu::options`.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    Pick(usize),
    Own(String),
    Multi { picked: Vec<usize>, own: Option<String> },
    Cancel,
}

/// What to send to the pane, in this order.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Keys(Vec<&'static str>),
    Text(String),
}

#[derive(Debug, Clone, PartialEq)]
enum Mark {
    None,
    Box(bool),
    Radio,
}

#[derive(Debug, Clone)]
struct Row {
    ptr: bool,
    num: Option<u32>,
    mark: Mark,
    label: String,
}

enum Kind {
    Blank,
    /// A rule or a frame line (`────`, `├──┤`, `╭─ Ask ─╮`).
    Rule { top: bool },
    Row(Row),
    Text(String),
}

const POINTERS: &[char] = &['❯', '›', '>', '▶', '▸', '→', '➤', '❱', '\u{f054}'];
const BOXES: &[char] = &['│', '┃', '║'];

fn frame_char(c: char) -> bool {
    "─━═╭╮╰╯├┤┬┴┼│┃║┏┓┗┛".contains(c)
}

/// The text of a line inside a frame (omp draws its menu in a box); `None` for a line that is not framed.
fn unframe(line: &str) -> Option<&str> {
    let t = line.trim_start();
    let c = t.chars().next()?;
    if !BOXES.contains(&c) {
        return None;
    }
    let inner = &t[c.len_utf8()..];
    let inner = inner.trim_end();
    Some(inner.strip_suffix(BOXES).unwrap_or(inner))
}

fn parse_mark(s: &str) -> (Mark, &str) {
    for (m, checked) in [("[ ]", false), ("[✔]", true), ("[x]", true), ("[X]", true), ("[✓]", true), ("[*]", true)] {
        if let Some(rest) = s.strip_prefix(m) {
            return (Mark::Box(checked), rest.trim_start());
        }
    }
    let mut it = s.chars();
    if let Some(c) = it.next() {
        let rest = it.as_str();
        let found = match c {
            '☐' | '◻' | '□' | '\u{f096}' => Some(Mark::Box(false)),
            '☑' | '☒' | '■' | '◼' | '\u{f14a}' => Some(Mark::Box(true)),
            '○' | '◯' | '◉' | '●' | '\u{f10c}' | '\u{f192}' | '\u{f111}' => Some(Mark::Radio),
            _ => None,
        };
        if let Some(m) = found {
            if rest.starts_with(' ') {
                return (m, rest.trim_start());
            }
        }
    }
    (Mark::None, s)
}

/// A list row: an optional cursor glyph, an optional number, an optional box or radio mark and the label.
fn parse_row(text: &str, framed: bool) -> Option<Row> {
    let mut s = text.trim_start();
    let mut ptr = false;
    if let Some(c) = s.chars().next() {
        if POINTERS.contains(&c) && s[c.len_utf8()..].starts_with(' ') {
            ptr = true;
            s = s[c.len_utf8()..].trim_start();
        }
    }
    let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
    let mut num = None;
    if !digits.is_empty() && digits.len() <= 2 {
        let after = &s[digits.len()..];
        if let Some(rest) = after.strip_prefix('.').or_else(|| after.strip_prefix(')')) {
            if rest.starts_with(' ') {
                num = digits.parse().ok();
                s = rest.trim_start();
            }
        }
    }
    let (mark, rest) = parse_mark(s);
    let label = rest.trim().to_string();
    if label.is_empty() {
        return None;
    }
    // A cursor glyph and nothing else is a prompt line (`❯ some text`), not a choice; it is one in a frame
    // (omp's «Submit») and on a Claude Code multiple choice's «Submit» line.
    let bare = num.is_none() && mark == Mark::None;
    if bare && !(ptr && (framed || label.eq_ignore_ascii_case("submit"))) {
        return None;
    }
    if num.is_none() && !ptr && mark == Mark::None {
        return None;
    }
    Some(Row { ptr, num, mark, label })
}

fn classify(line: &str) -> Kind {
    let line = line.trim_end();
    let t = line.trim();
    if t.is_empty() {
        return Kind::Blank;
    }
    let first = t.chars().next().unwrap_or(' ');
    // Claude Code frames a command with dashed lines (`╌╌╌`): they only part the paragraphs, they end nothing
    if t.chars().count() >= 3 && t.chars().all(|c| "╌╍┄┅┈┉".contains(c) || c == ' ') {
        return Kind::Blank;
    }
    let only_frame = t.chars().all(|c| frame_char(c) || c == ' ');
    // an empty line inside a box (`│      │`) is a blank line, not a rule
    if only_frame && t.chars().all(|c| BOXES.contains(&c) || c == ' ') {
        return Kind::Blank;
    }
    if only_frame || (matches!(first, '╭' | '╰' | '├') && t.chars().filter(|c| *c == '─').count() >= 3) {
        return Kind::Rule { top: matches!(first, '╭' | '╰') };
    }
    let (inner, framed) = match unframe(line) {
        Some(i) => (i, true),
        None => (line, false),
    };
    if inner.trim().is_empty() {
        return Kind::Blank;
    }
    match parse_row(inner, framed) {
        Some(r) => Kind::Row(r),
        None => Kind::Text(inner.trim().to_string()),
    }
}

/// The hint line under a menu: «Enter to select · ↑/↓ to navigate · Esc to cancel» in the program's words.
fn is_hint(text: &str) -> bool {
    let l = text.to_lowercase();
    l.contains("to navigate")
        || l.contains("press enter")
        || l.contains("↑/↓")
        || l.contains("↑↓")
        || l.contains("to toggle")
        || l.contains("toggle ·")
        || (l.contains("esc") && (l.contains("enter") || l.contains("select") || l.contains("cancel")))
}

/// A tab strip of a several-questions form: `←  ☐ Lib  ☐ Features  ✔ Submit  →` or `lib    feat    Submit`.
fn tab_strip(text: &str) -> Option<Vec<(String, bool)>> {
    let t = text.trim().trim_start_matches('←').trim_end_matches('→').trim();
    let tokens: Vec<&str> = t.split("  ").map(str::trim).filter(|x| !x.is_empty()).collect();
    if tokens.len() < 2 {
        return None;
    }
    let strip = |s: &str| -> (String, bool) {
        let mut it = s.chars();
        match it.next() {
            Some(c @ ('☐' | '☒' | '☑' | '✔' | '✓')) => (it.as_str().trim().to_string(), c != '☐'),
            _ => (s.trim().to_string(), false),
        }
    };
    let (last, _) = strip(tokens[tokens.len() - 1]);
    if !last.eq_ignore_ascii_case("submit") {
        return None;
    }
    Some(tokens[..tokens.len() - 1].iter().map(|s| strip(s)).collect())
}

fn is_own(label: &str) -> bool {
    let l = label.to_lowercase();
    l.starts_with("type something")
        || l.starts_with("other")
        || l.contains("type your own")
        || l.starts_with("something else")
}

fn question_end(s: &str) -> bool {
    s.ends_with(['?', ':', '？', '：'])
}

/// Reads a terminal screen (plain text). `None` when it is not a selection menu.
pub fn parse(screen: &str) -> Option<Menu> {
    let all: Vec<&str> = screen.lines().collect();
    // Only the lower part of the screen matters: the menu is at the bottom, the history above it is not read.
    let base = all.len().saturating_sub(60);
    let lines: Vec<Kind> = all[base..].iter().map(|l| classify(l)).collect();
    // The row under the cursor: the lowest one.
    let p = lines.iter().rposition(|k| matches!(k, Kind::Row(r) if r.ptr))?;
    // The group: rows, their descriptions and the rules between them, from the first row above to the last one below.
    // A blank line ends it unless another row follows (Codex leaves a blank line between its choices).
    let numbered_cursor = matches!(&lines[p], Kind::Row(r) if r.num.is_some());
    let like = |k: &Kind| matches!(k, Kind::Row(r) if r.num.is_some() == numbered_cursor);
    let mut start = p;
    let mut j = p;
    while j > 0 {
        j -= 1;
        match &lines[j] {
            Kind::Rule { top: true } => break,
            Kind::Row(_) => start = j,
            Kind::Blank => {
                // the nearest line above the blank: a row, or a description of one
                let mut k = j;
                while k > 0 && matches!(lines[k - 1], Kind::Blank) {
                    k -= 1;
                }
                while k > 0 && matches!(lines[k - 1], Kind::Text(_)) {
                    k -= 1;
                }
                if k == 0 || !like(&lines[k - 1]) {
                    break;
                }
            }
            _ => {}
        }
    }
    let mut end = p;
    let mut j = p + 1;
    while j < lines.len() {
        match &lines[j] {
            Kind::Rule { top: true } => break,
            Kind::Row(_) => end = j,
            Kind::Blank => {
                let mut k = j;
                while k < lines.len() && matches!(lines[k], Kind::Blank) {
                    k += 1;
                }
                if k >= lines.len() || !like(&lines[k]) {
                    break;
                }
            }
            _ => {}
        }
        j += 1;
    }
    // Numbering starts again at 1 with a new menu: an older numbered list above is not part of this one.
    for j in (start..p).rev() {
        if matches!(&lines[j], Kind::Row(r) if r.num == Some(1)) {
            start = j;
            break;
        }
    }
    // A description line follows its row; a hint line right after the last row is the hint, not a description.
    let hint_in = |from: usize| -> bool {
        (from..lines.len().min(from + 14)).any(|j| match &lines[j] {
            Kind::Text(t) => is_hint(t),
            _ => false,
        })
    };
    // The tail of the group: lines after the last row up to a blank are its description unless they are the hint.
    let mut tail_end = end;
    for (j, k) in lines.iter().enumerate().skip(end + 1) {
        match k {
            Kind::Text(t) if !is_hint(t) => tail_end = j,
            _ => break,
        }
    }
    let mut rows: Vec<(usize, Row, String)> = Vec::new();
    let mut j = start;
    while j <= tail_end {
        if let Kind::Row(r) = &lines[j] {
            let mut desc: Vec<String> = Vec::new();
            let mut k = j + 1;
            while k <= tail_end {
                match &lines[k] {
                    Kind::Text(t) => desc.push(t.clone()),
                    Kind::Rule { .. } => {}
                    _ => break,
                }
                k += 1;
            }
            rows.push((j, r.clone(), desc.join(" ")));
        }
        j += 1;
    }
    if rows.is_empty() {
        return None;
    }
    let numbered = rows.iter().filter(|(_, r, _)| r.num.is_some()).count();
    let consecutive = numbered >= 2
        && rows
            .iter()
            .filter_map(|(_, r, _)| r.num)
            .zip(1..)
            .all(|(n, i)| n == i);
    if !(hint_in(end + 1) || consecutive) {
        return None;
    }
    let boxed = all[base..][start].trim_start().starts_with(BOXES);
    let hint_text = (end + 1..lines.len().min(end + 15))
        .filter_map(|j| match &lines[j] {
            Kind::Text(t) if is_hint(t) => Some(t.to_lowercase()),
            _ => None,
        })
        .next()
        .unwrap_or_default();

    let multi = rows.iter().any(|(_, r, _)| matches!(r.mark, Mark::Box(_))) || hint_text.contains("toggle");
    // A multiple choice of Claude Code has a «Submit» line after the choices: a row for the cursor, not a choice.
    // The cursor on it makes it a row of its own; off it the line reads as the description of the row above.
    let mut built: Vec<(Row, String)> = rows.into_iter().map(|(_, r, d)| (r, d)).collect();
    if multi {
        if let Some(i) = built.iter().position(|(_, d)| d.eq_ignore_ascii_case("submit")) {
            built[i].1.clear();
            built.insert(i + 1, (Row { ptr: false, num: None, mark: Mark::None, label: "Submit".into() }, String::new()));
        }
    }
    let submit_at = if multi {
        built.iter().position(|(r, _)| r.num.is_none() && r.mark == Mark::None && r.label.eq_ignore_ascii_case("submit"))
    } else {
        None
    };
    let cursor = built.iter().position(|(r, _)| r.ptr).unwrap_or(0);
    let own_editor = boxed;
    // Claude Code's own-words row sits right above its «Chat about this» row (and above «Submit»); once words are
    // typed into it the row no longer says «Type something», but it is still that row.
    let own_by_place = built
        .iter()
        .position(|(r, _)| r.label.eq_ignore_ascii_case("chat about this"))
        .and_then(|chat| {
            let above = chat.checked_sub(1)?;
            let above = if Some(above) == submit_at { above.checked_sub(1)? } else { above };
            Some(above)
        });
    let options: Vec<MenuOption> = built
        .iter()
        .enumerate()
        .filter(|(i, (r, _))| Some(*i) != submit_at && !r.label.eq_ignore_ascii_case("chat about this"))
        .map(|(i, (r, d))| MenuOption {
            at: i,
            label: r.label.clone(),
            desc: d.clone(),
            checked: matches!(r.mark, Mark::Box(true)),
            own: is_own(&r.label) || own_by_place == Some(i),
        })
        .collect();
    if options.is_empty() && submit_at.is_none() {
        return None;
    }

    // The question and what stands around it, above the first row.
    let mut at = start;
    let mut paragraphs: Vec<Vec<String>> = Vec::new();
    let mut tabs: Vec<(String, bool)> = Vec::new();
    'up: while at > 0 && paragraphs.len() < 4 {
        // skip blank lines and rules to the next text
        loop {
            if at == 0 {
                break 'up;
            }
            at -= 1;
            match &lines[at] {
                Kind::Blank => continue,
                Kind::Rule { top } => {
                    if *top || !paragraphs.is_empty() {
                        break 'up;
                    }
                    continue;
                }
                Kind::Text(_) | Kind::Row(_) => break,
            }
        }
        let mut para: Vec<String> = Vec::new();
        loop {
            let text = match &lines[at] {
                Kind::Text(t) => t.clone(),
                Kind::Row(r) => r.label.clone(),
                _ => break,
            };
            if let Some(t) = tab_strip(&text) {
                tabs = t;
                if !para.is_empty() {
                    para.reverse();
                    paragraphs.push(para);
                }
                break 'up;
            }
            para.push(text);
            if at == 0 {
                break;
            }
            at -= 1;
        }
        para.reverse();
        if !para.is_empty() {
            paragraphs.push(para);
        }
        if at == 0 && !matches!(lines[0], Kind::Text(_) | Kind::Row(_)) {
            break;
        }
    }
    // paragraphs: nearest first
    let pick = paragraphs
        .iter()
        .position(|p| p.last().is_some_and(|l| question_end(l)))
        // Claude Code's trust dialog asks inside a paragraph («…or one you trust? (Like your own code…). If not…»)
        // and ends with a link line; the paragraph that asks is still the question.
        .or_else(|| paragraphs.iter().position(|p| p.iter().any(|l| l.contains('?'))))
        .unwrap_or(0);
    let question = paragraphs.get(pick).map(|p| p.join(" ")).unwrap_or_default();
    let mut ctx: Vec<String> = Vec::new();
    for (i, p) in paragraphs.iter().enumerate().rev() {
        if i != pick {
            ctx.extend(p.iter().cloned());
            ctx.push(String::new());
        }
    }
    while ctx.last().is_some_and(String::is_empty) {
        ctx.pop();
    }
    let skip = ctx.len().saturating_sub(14);
    let context = ctx[skip..].join("\n");
    let context: String = context.chars().take(1200).collect();

    let program = if boxed {
        "omp"
    } else if !tabs.is_empty() || hint_text.contains("tab/arrow") || hint_text.contains("chat about") {
        "claude"
    } else if hint_text.contains("press enter") {
        "codex"
    } else {
        "other"
    };
    let steps = tabs.len();
    let answered = tabs.iter().filter(|(_, d)| *d).count();
    let step = if steps > 0 && program == "claude" { (answered + 1).min(steps) } else { 0 };

    let mut h = DefaultHasher::new();
    (program, &question, tabs.iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>(), multi, step).hash(&mut h);
    for o in options.iter().filter(|o| !o.own) {
        o.label.hash(&mut h);
    }
    Some(Menu {
        program,
        sig: format!("{:016x}", h.finish()),
        question,
        context,
        options,
        multi,
        cursor,
        submit_at,
        own_editor,
        step,
        steps: if step > 0 { steps } else { 0 },
        tabs: if step > 0 { tabs.into_iter().map(|(l, _)| l).collect() } else { Vec::new() },
    })
}

/// Moves the cursor from `from` to `to` with the arrow keys (the page never presses a digit: the arrows work in every program).
fn goto(steps: &mut Vec<Step>, from: &mut usize, to: usize) {
    let key = if to > *from { "down" } else { "up" };
    let n = to.abs_diff(*from);
    if n > 0 {
        steps.push(Step::Keys(vec![key; n]));
    }
    *from = to;
}

/// The keys and the text that give `answer` in `menu`, in the order they are sent.
pub fn plan(menu: &Menu, answer: &Answer) -> Result<Vec<Step>, String> {
    let mut steps: Vec<Step> = Vec::new();
    let mut cur = menu.cursor;
    let find = |at: usize| menu.options.iter().find(|o| o.at == at);
    let own_row = menu.options.iter().find(|o| o.own);
    // `confirm`: press Enter after the words. Claude Code's multiple choice ticks the row as the words are typed,
    // and Enter would clear the tick again.
    let type_own = |steps: &mut Vec<Step>, cur: &mut usize, text: &str, confirm: bool| -> Result<(), String> {
        let row = own_row.ok_or("в этом меню нет строки для своего ответа")?;
        let text = text.trim();
        if text.is_empty() || text.chars().count() > 2000 || text.chars().any(char::is_control) {
            return Err("свой ответ пуст, слишком длинный или с управляющими знаками".into());
        }
        goto(steps, cur, row.at);
        if menu.own_editor {
            steps.push(Step::Keys(vec!["enter"]));
        }
        steps.push(Step::Text(text.to_string()));
        if confirm || menu.own_editor {
            steps.push(Step::Keys(vec!["enter"]));
        }
        Ok(())
    };
    match answer {
        Answer::Cancel => steps.push(Step::Keys(vec!["esc"])),
        Answer::Pick(at) => {
            if menu.multi {
                return Err("множественный выбор отправляется вместе".into());
            }
            let option = find(*at).ok_or("такой строки нет в меню")?;
            if option.own {
                return Err("для своего ответа нужен текст".into());
            }
            goto(&mut steps, &mut cur, option.at);
            steps.push(Step::Keys(vec!["enter"]));
        }
        Answer::Own(text) => {
            if menu.multi {
                return Err("множественный выбор отправляется вместе".into());
            }
            type_own(&mut steps, &mut cur, text, true)?;
        }
        Answer::Multi { picked, own } => {
            if !menu.multi {
                return Err("это не множественный выбор".into());
            }
            for o in menu.options.iter().filter(|o| !o.own) {
                if o.checked != picked.contains(&o.at) {
                    goto(&mut steps, &mut cur, o.at);
                    steps.push(Step::Keys(vec!["space"]));
                }
            }
            if let Some(bad) = picked.iter().find(|a| find(**a).is_none_or(|o| o.own)) {
                return Err(format!("строки {bad} нет среди вариантов"));
            }
            if let Some(text) = own {
                type_own(&mut steps, &mut cur, text, false)?;
            }
            match menu.submit_at {
                Some(at) => {
                    goto(&mut steps, &mut cur, at);
                    steps.push(Step::Keys(vec!["enter"]));
                }
                // omp: Enter on any row confirms the question; its editor of own words has confirmed it already
                None if own.is_some() => {}
                None => steps.push(Step::Keys(vec!["enter"])),
            }
        }
    }
    Ok(steps)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAUDE_SINGLE: &str = include_str!("menu_samples/claude-single.txt");
    const CLAUDE_OTHER: &str = include_str!("menu_samples/claude-other-focused.txt");
    const CLAUDE_MULTI: &str = include_str!("menu_samples/claude-multi.txt");
    const CLAUDE_MULTI_SUBMIT: &str = include_str!("menu_samples/claude-multi-submit.txt");
    const CLAUDE_REVIEW: &str = include_str!("menu_samples/claude-review.txt");
    const OMP_SINGLE: &str = include_str!("menu_samples/omp-single.txt");
    const OMP_MULTI: &str = include_str!("menu_samples/omp-multi.txt");
    const OMP_REVIEW: &str = include_str!("menu_samples/omp-review.txt");
    const CODEX_LOGIN: &str = include_str!("menu_samples/codex-login.txt");
    const CODEX_APPROVAL: &str = include_str!("menu_samples/codex-approval.txt");
    const CLAUDE_PERMISSION: &str = include_str!("menu_samples/claude-permission.txt");
    const CLAUDE_CREATE_FILE: &str = include_str!("menu_samples/claude-create-file.txt");
    const CLAUDE_TRUST: &str = include_str!("menu_samples/claude-trust-folder.txt");
    const CLAUDE_TRUST_BOXED: &str = include_str!("menu_samples/claude-trust-folder-boxed.txt");
    const CODEX_TRUST: &str = include_str!("menu_samples/codex-trust-folder.txt");

    fn labels(m: &Menu) -> Vec<&str> {
        m.options.iter().map(|o| o.label.as_str()).collect()
    }

    #[test]
    fn claude_single_choice_with_descriptions_and_several_questions() {
        let m = parse(CLAUDE_SINGLE).expect("a menu");
        assert_eq!(m.program, "claude");
        assert_eq!(m.question, "Which library?");
        assert_eq!(labels(&m), ["date-fns", "dayjs", "Type something."]);
        assert_eq!(m.options[0].desc, "Modular");
        assert_eq!(m.options[1].desc, "Small");
        assert!(m.options[2].own && !m.options[0].own);
        assert!(!m.multi);
        assert_eq!(m.cursor, 0);
        // «Chat about this» is a row of the program, not a choice for the page
        assert!(!labels(&m).contains(&"Chat about this"));
        assert_eq!((m.step, m.steps), (1, 2));
        assert_eq!(m.tabs, ["Lib", "Features"]);
        assert!(!m.own_editor);
    }

    #[test]
    fn the_cursor_and_the_typed_text_do_not_change_the_menu() {
        let a = parse(CLAUDE_SINGLE).unwrap();
        let b = parse(CLAUDE_OTHER).unwrap();
        assert_eq!(b.cursor, 2, "the cursor is on «Type something.»");
        assert_eq!(a.sig, b.sig);
        let typing = CLAUDE_OTHER.replace("Type something.", "my own answer");
        let t = parse(&typing).unwrap();
        assert_eq!(t.sig, a.sig);
        assert!(t.options[2].own, "the typed row is still the own-words row");
        assert_eq!(t.options[2].label, "my own answer");
    }

    #[test]
    fn claude_multiple_choice_has_boxes_and_a_submit_row() {
        let m = parse(CLAUDE_MULTI).expect("a menu");
        assert!(m.multi);
        assert_eq!(m.question, "Which features?");
        assert_eq!(labels(&m), ["A", "B", "C", "Type something"]);
        assert_eq!(m.options[0].desc, "First");
        assert!(m.options.iter().all(|o| !o.checked));
        assert_eq!(m.submit_at, Some(4), "the Submit line is the row after the four choices");
        assert_eq!((m.step, m.steps), (2, 2));
        let ticked = parse(CLAUDE_MULTI_SUBMIT).expect("a menu with the cursor on Submit");
        assert_eq!(ticked.cursor, 4);
        assert_eq!(ticked.submit_at, Some(4));
        assert_eq!(
            ticked.options.iter().map(|o| o.checked).collect::<Vec<_>>(),
            [true, false, true, false]
        );
        assert_eq!(ticked.sig, m.sig, "ticks are not a new question");
    }

    #[test]
    fn claude_review_page_is_a_choice() {
        let m = parse(CLAUDE_REVIEW).expect("a menu");
        assert_eq!(m.question, "Ready to submit your answers?");
        assert_eq!(labels(&m), ["Submit answers", "Cancel"]);
        assert!(m.context.contains("Review your answers"), "{}", m.context);
        assert!(m.context.contains("my own answer"));
    }

    #[test]
    fn claude_permission_prompt() {
        let m = parse(CLAUDE_PERMISSION).expect("a menu");
        assert_eq!(m.question, "Do you want to proceed?");
        assert_eq!(m.options.len(), 4);
        assert_eq!(m.options[3].label, "No");
        assert!(m.context.contains("touch made-by-test.txt"), "{}", m.context);
        assert!(m.context.contains("Bash command") && !m.context.contains('╌'), "{}", m.context);
        assert!(m.question.len() < 60);
    }

    #[test]
    fn claude_file_creation_prompt() {
        let m = parse(CLAUDE_CREATE_FILE).expect("a menu");
        assert_eq!(m.question, "Do you want to create closing-note2.md?");
        assert_eq!(
            labels(&m),
            [
                "Yes",
                "Yes, and switch to accept edits (shift+tab) for this session",
                "No"
            ]
        );
        assert!(m.context.contains("closing-note2.md"), "{}", m.context);
        assert_eq!(m.cursor, 0);
    }

    #[test]
    fn omp_ask_in_a_box() {
        let m = parse(OMP_SINGLE).expect("a menu");
        assert_eq!(m.program, "omp");
        assert_eq!(m.question, "Which library?");
        assert_eq!(labels(&m), ["date-fns", "dayjs", "Other (type your own)"]);
        assert!(m.options[2].own);
        assert!(m.own_editor);
        assert_eq!(m.cursor, 0);
        let multi = parse(OMP_MULTI).expect("a menu");
        assert!(multi.multi);
        assert_eq!(labels(&multi), ["A", "B", "C", "Other (type your own)"]);
        assert_eq!(multi.submit_at, None);
        assert_eq!(multi.cursor, 2);
        assert_eq!(multi.options.iter().map(|o| o.checked).collect::<Vec<_>>(), [true, false, true, false]);
        let review = parse(OMP_REVIEW).expect("the review page is one row");
        assert_eq!(labels(&review), ["Submit"]);
    }

    /// PLN-101: the first start in a new working copy asks whether to trust the folder. Whatever the program draws,
    /// the page must get a menu whose first row is «yes» and whose second is «no», and the keys that answer it.
    #[test]
    fn the_trust_folder_dialogs_are_menus_with_a_yes_and_a_no() {
        for (screen, yes, no) in [
            (CLAUDE_TRUST, "Yes, I trust this folder", "No, exit"),
            (CLAUDE_TRUST_BOXED, "Yes, proceed", "No, exit"),
            (CODEX_TRUST, "Yes, continue", "No, quit"),
        ] {
            let m = parse(screen).unwrap_or_else(|| panic!("a menu in:\n{screen}"));
            assert_eq!(labels(&m), [yes, no], "{screen}");
            assert!(!m.multi && m.cursor == 0);
            assert_eq!(plan(&m, &Answer::Pick(0)).unwrap(), [Step::Keys(vec!["enter"])]);
            assert_eq!(plan(&m, &Answer::Pick(1)).unwrap(), [Step::Keys(vec!["down"]), Step::Keys(vec!["enter"])]);
        }
        assert!(parse(CLAUDE_TRUST).unwrap().question.starts_with("Quick safety check: Is this a project you created"));
        assert_eq!(parse(CLAUDE_TRUST_BOXED).unwrap().question, "Do you trust the files in this folder?");
        assert_eq!(
            parse(CODEX_TRUST).unwrap().question,
            "Do you trust the contents of this directory? Working with untrusted contents comes with higher risk of prompt injection."
        );
    }

    #[test]
    fn codex_menus() {
        let m = parse(CODEX_LOGIN).expect("a menu");
        assert_eq!(m.program, "codex");
        assert_eq!(m.options.len(), 3);
        assert_eq!(m.options[1].label, "Sign in with Device Code");
        assert_eq!(m.options[1].desc, "Sign in from another device with a one-time code");
        let a = parse(CODEX_APPROVAL).expect("a menu");
        assert_eq!(a.question, "Would you like to run the following command?");
        assert!(a.context.contains("rm -rf build"), "{}", a.context);
        assert_eq!(a.options.len(), 3);
    }

    #[test]
    fn a_screen_that_is_not_a_menu_is_none() {
        assert!(parse("").is_none());
        assert!(parse("just some output\n\n❯ \n").is_none());
        // a numbered list in the agent's answer is not a menu
        assert!(parse("Options:\n  1. first\n  2. second\n\n❯ my prompt text\n").is_none());
        // the plain prompt line with text is not a choice
        assert!(parse("Done.\n❯ please continue\n  ⏵⏵ accept edits on · esc to interrupt\n").is_none());
    }

    #[test]
    fn a_pointer_in_the_history_above_is_not_the_menu() {
        let screen = format!("❯ 1. an old option\n{}", "output line\n".repeat(80));
        assert!(parse(&screen).is_none());
    }

    #[test]
    fn single_choice_keys_move_by_arrows_from_the_cursor_and_press_enter() {
        let m = parse(CLAUDE_SINGLE).unwrap();
        assert_eq!(plan(&m, &Answer::Pick(0)).unwrap(), [Step::Keys(vec!["enter"])]);
        assert_eq!(plan(&m, &Answer::Pick(1)).unwrap(), [Step::Keys(vec!["down"]), Step::Keys(vec!["enter"])]);
        // from the cursor, not from the top
        let at_other = parse(CLAUDE_OTHER).unwrap();
        assert_eq!(
            plan(&at_other, &Answer::Pick(0)).unwrap(),
            [Step::Keys(vec!["up", "up"]), Step::Keys(vec!["enter"])]
        );
        assert!(plan(&m, &Answer::Pick(2)).is_err(), "own answer needs text");
        assert!(plan(&m, &Answer::Pick(9)).is_err());
        assert_eq!(plan(&m, &Answer::Cancel).unwrap(), [Step::Keys(vec!["esc"])]);
    }

    #[test]
    fn own_answer_types_into_the_row_or_into_the_editor() {
        let claude = parse(CLAUDE_SINGLE).unwrap();
        assert_eq!(
            plan(&claude, &Answer::Own(" мой ответ ".into())).unwrap(),
            [
                Step::Keys(vec!["down", "down"]),
                Step::Text("мой ответ".into()),
                Step::Keys(vec!["enter"])
            ]
        );
        let omp = parse(OMP_SINGLE).unwrap();
        assert_eq!(
            plan(&omp, &Answer::Own("x".into())).unwrap(),
            [
                Step::Keys(vec!["down", "down"]),
                Step::Keys(vec!["enter"]),
                Step::Text("x".into()),
                Step::Keys(vec!["enter"])
            ]
        );
        assert!(plan(&claude, &Answer::Own("a\nb".into())).is_err());
        assert!(plan(&claude, &Answer::Own("  ".into())).is_err());
    }

    #[test]
    fn multiple_choice_toggles_the_difference_then_submits() {
        let m = parse(CLAUDE_MULTI).unwrap();
        let steps = plan(&m, &Answer::Multi { picked: vec![0, 2], own: None }).unwrap();
        assert_eq!(
            steps,
            [
                Step::Keys(vec!["space"]),
                Step::Keys(vec!["down", "down"]),
                Step::Keys(vec!["space"]),
                Step::Keys(vec!["down", "down"]),
                Step::Keys(vec!["enter"]),
            ]
        );
        // already ticked boxes are left alone, and unwanted ones are cleared
        let ticked = parse(CLAUDE_MULTI_SUBMIT).unwrap();
        let steps = plan(&ticked, &Answer::Multi { picked: vec![0, 2], own: None }).unwrap();
        assert_eq!(steps, [Step::Keys(vec!["enter"])], "the cursor is already on Submit");
        let steps = plan(&ticked, &Answer::Multi { picked: vec![1], own: None }).unwrap();
        assert!(steps.len() > 2);
        assert_eq!(steps.last(), Some(&Step::Keys(vec!["enter"])));
        let omp = parse(OMP_MULTI).unwrap();
        let steps = plan(&omp, &Answer::Multi { picked: vec![0, 2], own: None }).unwrap();
        assert_eq!(steps, [Step::Keys(vec!["enter"])], "omp: Enter confirms the question");
        let steps = plan(&omp, &Answer::Multi { picked: vec![0], own: None }).unwrap();
        assert_eq!(steps, [Step::Keys(vec!["space"]), Step::Keys(vec!["enter"])], "the cursor is on C: untick it");
        let steps = plan(&omp, &Answer::Multi { picked: vec![0, 2], own: Some("D".into()) }).unwrap();
        assert_eq!(
            steps,
            [
                Step::Keys(vec!["down"]),
                Step::Keys(vec!["enter"]),
                Step::Text("D".into()),
                Step::Keys(vec!["enter"]),
            ],
            "the editor's Enter ends the question"
        );
    }

    #[test]
    fn multiple_choice_with_own_words() {
        let m = parse(CLAUDE_MULTI).unwrap();
        let steps = plan(&m, &Answer::Multi { picked: vec![1], own: Some("D".into()) }).unwrap();
        assert!(steps.contains(&Step::Text("D".into())));
        // Claude ticks the own row as the words go in: no Enter after them, straight on to Submit
        let at = steps.iter().position(|s| *s == Step::Text("D".into())).unwrap();
        assert_eq!(steps[at + 1], Step::Keys(vec!["down"]));
        assert_eq!(steps.last(), Some(&Step::Keys(vec!["enter"])));
        assert!(plan(&m, &Answer::Multi { picked: vec![3], own: None }).is_err(), "the own row is not a plain box");
        assert!(plan(&m, &Answer::Pick(0)).is_err(), "a multiple choice is sent whole");
    }
}
