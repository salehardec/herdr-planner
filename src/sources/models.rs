//! Which agent and model made a thread.
//!
//! While a thread works it shows its profile's model (`herdr-projects profile
//! resolve NAME`, the thread's own `profile_args` on top). Once it is finished
//! it shows the models named in the `Co-Authored-By` trailers of its commits;
//! without trailers, its profile, quietly: "by fact" differs from "by profile".

use super::tasks::clip;
use super::threads::Thread;
use serde::Serialize;

const FAMILIES: [&str; 4] = ["sonnet", "opus", "haiku", "fable"];

/// Words that start the name of an AI co-author in a trailer.
const AI_NAMES: [&str; 12] = [
    "claude", "deepseek", "gpt", "gemini", "codex", "copilot", "qwen", "kimi", "glm", "grok",
    "mistral", "llama",
];

/// A short name and family for a model id (`claude-sonnet-5-5`,
/// `deepseek/deepseek-v4-pro`) or the name in a trailer (`Claude Sonnet 5.5`):
/// "Sonnet 5.5", "DS Pro". Unknown ones stay as they are.
pub fn label(raw: &str) -> Option<(String, String)> {
    let raw = raw.trim();
    let s = strip_groups(&raw.to_lowercase());
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if s.contains("deepseek") {
        let text = if s.contains("flash") {
            "DS Flash".to_string()
        } else if s.contains("pro") {
            "DS Pro".to_string()
        } else {
            match version_token(s) {
                Some(v) => format!("DS {}", v.to_uppercase()),
                None => "DeepSeek".to_string(),
            }
        };
        return Some((text, "deepseek".into()));
    }
    for fam in FAMILIES {
        if let Some(at) = s.find(fam) {
            let rest = format!("{} {}", &s[..at], &s[at + fam.len()..]);
            // Date stamps (20250929) are longer than a version part.
            let nums: Vec<&str> = digit_runs(&rest)
                .into_iter()
                .filter(|t| t.len() <= 2)
                .collect();
            let mut name = fam[..1].to_uppercase() + &fam[1..];
            match nums.as_slice() {
                [] => {}
                [a] => name = format!("{name} {a}"),
                [a, b, ..] => name = format!("{name} {a}.{b}"),
            }
            return Some((name, fam.to_string()));
        }
    }
    // `vendor/model` and a leading `claude-` are noise.
    let plain = match raw.split_once('/') {
        Some((head, tail)) if !head.contains(char::is_whitespace) => tail,
        _ => raw,
    };
    let lower = plain.to_lowercase();
    let plain = if lower.starts_with("claude-") || lower.starts_with("claude ") {
        plain.get(7..).unwrap_or(plain)
    } else {
        plain
    }
    .trim();
    let shown = if plain.is_empty() { raw } else { plain };
    Some((clip(shown, 18), "other".into()))
}

/// `s` without `(...)` and `[...]` groups.
fn strip_groups(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0u32;
    for c in s.chars() {
        match c {
            '(' | '[' => {
                depth += 1;
                out.push(' ');
            }
            ')' | ']' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

fn digit_runs(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in s.char_indices() {
        match (c.is_ascii_digit(), start) {
            (true, None) => start = Some(i),
            (false, Some(b)) => {
                out.push(&s[b..i]);
                start = None;
            }
            _ => {}
        }
    }
    if let Some(b) = start {
        out.push(&s[b..]);
    }
    out
}

/// `v4`, `v4.1`: the first version-looking token.
fn version_token(s: &str) -> Option<&str> {
    let at = s.char_indices().find_map(|(i, c)| {
        (c == 'v' && s[i + 1..].starts_with(|d: char| d.is_ascii_digit())).then_some(i)
    })?;
    let end = s[at + 1..]
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .map(|e| at + 1 + e)
        .unwrap_or(s.len());
    Some(&s[at..end])
}

/// A trailer value (`Claude Sonnet 5.5 <noreply@anthropic.com>`): label and
/// family, or none for a person.
pub fn trailer_label(value: &str) -> Option<(String, String)> {
    let (name, email) = match (value.find('<'), value.rfind('>')) {
        (Some(a), Some(b)) if b > a => (value[..a].trim(), value[a + 1..b].trim()),
        _ => (value.trim(), ""),
    };
    let (lname, lmail) = (name.to_lowercase(), email.to_lowercase());
    let ai = lmail.contains("@anthropic.com")
        || lmail.contains("@openai.com")
        || AI_NAMES.iter().any(|w| lname.starts_with(w));
    if !ai {
        return None;
    }
    label(name)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Badge {
    pub label: String,
    pub family: String,
    /// A guess from the profile rather than a fact from the commits.
    pub quiet: bool,
}

/// Unique models of a list of trailer values, in order of appearance.
pub fn from_trailers(values: &[String]) -> Vec<Badge> {
    let mut out: Vec<Badge> = Vec::new();
    for v in values {
        if let Some((label, family)) = trailer_label(v) {
            if !out.iter().any(|b| b.label == label) {
                out.push(Badge {
                    label,
                    family,
                    quiet: false,
                });
            }
        }
    }
    out
}

/// The value after `flag` (or `--flag=value`) in an argument list; the last one wins.
pub fn arg_value(args: &[String], flags: &[&str]) -> Option<String> {
    let mut found = None;
    for (i, a) in args.iter().enumerate() {
        for f in flags {
            if a == f {
                if let Some(next) = args.get(i + 1) {
                    found = Some(next.clone());
                }
            } else if let Some(v) = a.strip_prefix(&format!("{f}=")) {
                found = Some(v.to_string());
            }
        }
    }
    found
}

/// What `profile resolve NAME` says about a profile.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Resolved {
    pub agent: String,
    pub model: Option<String>,
    pub effort: Option<String>,
}

const MODEL_FLAGS: [&str; 2] = ["--model", "-m"];
const EFFORT_FLAGS: [&str; 3] = ["--effort", "--thinking", "--reasoning-effort"];

/// `profile resolve` JSON: `{agent, args, name}`.
pub fn parse_resolve(stdout: &str) -> Option<Resolved> {
    let v: serde_json::Value = serde_json::from_str(stdout).ok()?;
    let agent = v.get("agent")?.as_str()?.to_string();
    let args: Vec<String> = v
        .get("args")
        .and_then(|a| a.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    Some(Resolved {
        agent,
        model: arg_value(&args, &MODEL_FLAGS),
        effort: arg_value(&args, &EFFORT_FLAGS),
    })
}

/// The thread's launch setup as far as known.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Launch {
    pub name: String,
    pub agent: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub label: Option<String>,
    pub family: String,
    pub remote: bool,
    /// The profile was read here (not a remote machine's, not a removed one).
    pub resolved: bool,
}

/// A profile that does not resolve here (remote machine, removed profile) is
/// shown by its name. `profile_args` of the thread override the profile's.
pub fn launch(t: &Thread, resolved: Option<&Resolved>) -> Launch {
    let mut l = Launch {
        name: t.profile.clone(),
        agent: t.agent.clone(),
        remote: t.remote(),
        ..Default::default()
    };
    if let Some(r) = resolved.filter(|_| !l.remote) {
        l.agent = r.agent.clone();
        l.model = r.model.clone();
        l.effort = r.effort.clone();
        l.resolved = true;
    }
    if let Some(m) = arg_value(&t.profile_args, &MODEL_FLAGS) {
        l.model = Some(m);
    }
    if let Some(e) = arg_value(&t.profile_args, &EFFORT_FLAGS) {
        l.effort = Some(e);
    }
    match l.model.as_deref().and_then(label) {
        Some((text, family)) => {
            l.label = Some(text);
            l.family = family;
        }
        None => {
            let name = if !t.profile.is_empty() {
                &t.profile
            } else {
                &t.agent
            };
            if !name.is_empty() {
                l.label = Some(clip(name, 18));
            }
            l.family = "other".into();
        }
    }
    l
}

/// What to show for a thread: the badges, where they come from and how it was launched.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ThreadModels {
    pub badges: Vec<Badge>,
    /// `commits` (a fact) or `profile` (a guess).
    pub source: Option<&'static str>,
    /// Where the trailers were found.
    pub src: Option<String>,
    pub launch: Launch,
    /// The parts of the "Запуск" line: harness, profile, model, effort, machine.
    pub launch_parts: Vec<String>,
}

/// `trailers` are the `Co-Authored-By` values from the thread's git summary
/// (a live worktree), `repo_trailers` and `repo_src` from the repository.
pub fn of(
    t: &Thread,
    resolved: Option<&Resolved>,
    trailers: &[String],
    summary_src: Option<&str>,
    repo_trailers: &[String],
    repo_src: Option<&str>,
) -> ThreadModels {
    let l = launch(t, resolved);
    let mut parts: Vec<String> = Vec::new();
    for p in [
        Some(l.agent.clone()),
        (!l.name.is_empty()).then(|| format!("профиль {}", l.name)),
        l.model.clone(),
        l.effort.as_ref().map(|e| format!("effort {e}")),
        (l.remote && !t.machine.is_empty()).then(|| format!("@{}", t.machine)),
    ]
    .into_iter()
    .flatten()
    {
        if !p.is_empty() {
            parts.push(p);
        }
    }
    let mut out = ThreadModels {
        launch_parts: parts,
        ..Default::default()
    };
    if t.failed() {
        out.launch = l;
        return out;
    }
    if t.closed() {
        let mut found = from_trailers(trailers);
        let mut src = (!found.is_empty()).then(|| summary_src.unwrap_or("ветка").to_string());
        if found.is_empty() {
            found = from_trailers(repo_trailers);
            src = (!found.is_empty()).then(|| repo_src.unwrap_or("коммиты").to_string());
        }
        if !found.is_empty() {
            out.source = Some("commits");
            out.badges = found;
            out.src = src;
            out.launch = l;
            return out;
        }
        if let Some(text) = &l.label {
            out.source = Some("profile");
            out.badges = vec![Badge {
                label: text.clone(),
                family: l.family.clone(),
                quiet: true,
            }];
        }
        out.launch = l;
        return out;
    }
    if let Some(text) = &l.label {
        out.source = Some("profile");
        out.badges = vec![Badge {
            label: text.clone(),
            family: l.family.clone(),
            quiet: false,
        }];
    }
    out.launch = l;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lab(s: &str) -> (String, String) {
        label(s).unwrap()
    }

    #[test]
    fn short_names() {
        assert_eq!(
            lab("claude-sonnet-5-5"),
            ("Sonnet 5.5".into(), "sonnet".into())
        );
        assert_eq!(
            lab("Claude Opus 5.5 (1M context)"),
            ("Opus 5.5".into(), "opus".into())
        );
        assert_eq!(lab("claude-haiku-4-5-20251001").0, "Haiku 4.5");
        assert_eq!(lab("claude-fable-5-1").0, "Fable 5.1");
        assert_eq!(lab("Claude Sonnet 4").0, "Sonnet 4");
        assert_eq!(lab("claude-opus").0, "Opus");
        assert_eq!(
            lab("deepseek/deepseek-v4-flash"),
            ("DS Flash".into(), "deepseek".into())
        );
        assert_eq!(lab("deepseek/deepseek-v4-pro").0, "DS Pro");
        assert_eq!(lab("DeepSeek V4").0, "DS V4");
        assert_eq!(lab("DeepSeek").0, "DeepSeek");
        assert_eq!(lab("gpt-5"), ("gpt-5".into(), "other".into()));
        assert_eq!(
            lab("openai/gpt-5-codex-with-a-very-long-name")
                .0
                .chars()
                .count(),
            18
        );
        assert_eq!(lab("Claude").0, "Claude");
        assert_eq!(label("  "), None);
    }

    #[test]
    fn trailers_skip_people() {
        assert_eq!(
            trailer_label("Claude Sonnet 5.5 <noreply@anthropic.com>")
                .unwrap()
                .0,
            "Sonnet 5.5"
        );
        assert_eq!(trailer_label("Anna Petrova <anna@example.com>"), None);
        assert_eq!(trailer_label("Bot <bot@openai.com>").unwrap().1, "other");
        assert_eq!(trailer_label("claude-opus-5-5").unwrap().0, "Opus 5.5");
        let got = from_trailers(&[
            "Claude Sonnet 5.5 <noreply@anthropic.com>".into(),
            "Anna <a@b.c>".into(),
            "Claude Opus 5.5 <noreply@anthropic.com>".into(),
            "Claude Sonnet 5.5 <noreply@anthropic.com>".into(),
        ]);
        assert_eq!(
            got.iter().map(|b| b.label.as_str()).collect::<Vec<_>>(),
            ["Sonnet 5.5", "Opus 5.5"]
        );
    }

    #[test]
    fn args_and_resolve() {
        let args: Vec<String> = ["--model", "a", "--effort=high", "--model=b"]
            .map(String::from)
            .to_vec();
        assert_eq!(arg_value(&args, &MODEL_FLAGS).as_deref(), Some("b"));
        assert_eq!(arg_value(&args, &EFFORT_FLAGS).as_deref(), Some("high"));
        let r = parse_resolve(r#"{"agent":"claude","args":["--model","claude-sonnet-5-5","--effort","high"],"name":"claude-sonnet"}"#).unwrap();
        assert_eq!(r.model.as_deref(), Some("claude-sonnet-5-5"));
        assert_eq!(r.effort.as_deref(), Some("high"));
        assert_eq!(parse_resolve("nope"), None);
        assert_eq!(parse_resolve(r#"{"args":[]}"#), None);
    }

    fn thread(group: &str) -> Thread {
        Thread {
            id: "t-1".into(),
            group: group.into(),
            profile: "claude-sonnet".into(),
            agent: "claude".into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_working_thread_shows_its_profile() {
        let r = Resolved {
            agent: "claude".into(),
            model: Some("claude-sonnet-5-5".into()),
            effort: Some("high".into()),
        };
        let m = of(&thread("Working"), Some(&r), &[], None, &[], None);
        assert_eq!(m.source, Some("profile"));
        assert_eq!(m.badges[0].label, "Sonnet 5.5");
        assert!(!m.badges[0].quiet);
        assert_eq!(
            m.launch_parts,
            [
                "claude",
                "профиль claude-sonnet",
                "claude-sonnet-5-5",
                "effort high"
            ]
        );
        // The thread's own arguments win over the profile's.
        let mut t = thread("Working");
        t.profile_args = ["--model", "claude-opus-5-5"].map(String::from).to_vec();
        assert_eq!(
            of(&t, Some(&r), &[], None, &[], None).badges[0].label,
            "Opus 5.5"
        );
        // A profile that did not resolve is shown by name.
        let unknown = of(&thread("Working"), None, &[], None, &[], None);
        assert_eq!(unknown.badges[0].label, "claude-sonnet");
        assert_eq!(unknown.badges[0].family, "other");
    }

    #[test]
    fn a_finished_thread_shows_its_commits_then_its_profile() {
        let co = vec!["Claude Opus 5.5 <noreply@anthropic.com>".to_string()];
        let m = of(&thread("Resolved"), None, &co, None, &[], None);
        assert_eq!(m.source, Some("commits"));
        assert_eq!(m.badges[0].label, "Opus 5.5");
        assert_eq!(m.src.as_deref(), Some("ветка"));
        let repo = of(
            &thread("Resolved"),
            None,
            &[],
            None,
            &co,
            Some("слитый коммит"),
        );
        assert_eq!(repo.src.as_deref(), Some("слитый коммит"));
        let quiet = of(&thread("Resolved"), None, &[], None, &[], None);
        assert_eq!(quiet.source, Some("profile"));
        assert!(quiet.badges[0].quiet);
        // A remote thread's profile is not resolved here.
        let mut remote = thread("Working");
        remote.machine = "mac".into();
        let r = Resolved {
            agent: "claude".into(),
            model: Some("claude-haiku-5-5".into()),
            effort: None,
        };
        let m = of(&remote, Some(&r), &[], None, &[], None);
        assert_eq!(m.badges[0].label, "claude-sonnet");
        assert!(m.launch_parts.contains(&"@mac".to_string()));
        // A failed start has no model.
        let mut failed = thread("Working");
        failed.status = "failed".into();
        assert!(of(&failed, None, &[], None, &[], None).badges.is_empty());
    }
}
