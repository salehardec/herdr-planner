//! `herdr-projects assignable <slug>`: the profiles a task can be handed to.

use serde::Serialize;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Assignable {
    /// `name` for this machine, `name@machine` for another.
    pub profiles: Vec<String>,
}

/// "Profiles here: a, b" (this machine) and "@m1: c, d" lines (other
/// machines, as c@m1); "(none…)" and "profiles unknown" lines add nothing.
pub fn parse_assignable(stdout: &str) -> Assignable {
    let mut out = Assignable::default();
    let mut add = |name: String| {
        if !out.profiles.contains(&name) {
            out.profiles.push(name);
        }
    };
    for line in stdout.lines() {
        if let Some(names) = line.strip_prefix("Profiles here:") {
            let names = names.trim();
            if !names.starts_with("(none") {
                for n in names.split([',', ' ']).filter(|n| !n.is_empty()) {
                    add(n.to_string());
                }
            }
        } else if let Some(rest) = line.strip_prefix('@') {
            let Some((machine, rest)) = rest.split_once(':') else {
                continue;
            };
            let machine = machine.trim();
            if machine.is_empty() || machine.contains(char::is_whitespace) {
                continue;
            }
            if rest.contains("profiles unknown") {
                continue;
            }
            // A trailing "(note)" is not a profile.
            let rest = match rest.rfind('(') {
                Some(i) if rest.trim_end().ends_with(')') => &rest[..i],
                _ => rest,
            };
            for n in rest.split([',', ' ']).filter(|n| !n.is_empty()) {
                add(format!("{n}@{machine}"));
            }
        }
    }
    out
}

/// A profile name the card may pass on: `name` or `name@machine`.
pub fn valid_profile(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 80
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '-' | '@'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_both_kinds_of_lines() {
        let a = parse_assignable(
            "me: the user\nProfiles here: claude-fable, claude-sonnet, deepseek-pro\n@mac: pi, claude-sonnet (3 profiles)\n@old: (profiles unknown)\nMachines: none\n",
        );
        assert_eq!(
            a.profiles,
            [
                "claude-fable",
                "claude-sonnet",
                "deepseek-pro",
                "pi@mac",
                "claude-sonnet@mac"
            ]
        );
        assert!(parse_assignable("Profiles here: (none configured)\n")
            .profiles
            .is_empty());
        assert!(parse_assignable("").profiles.is_empty());
    }

    #[test]
    fn profile_names_are_checked() {
        assert!(valid_profile("claude-sonnet"));
        assert!(valid_profile("pi@mac"));
        assert!(!valid_profile("-x"));
        assert!(!valid_profile("a b"));
        assert!(!valid_profile(""));
    }
}
