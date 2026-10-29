//! Which agent a provider really runs through on the board: read from the crew's settings (`coordinator.agent`, `worker.agent`,
//! `reviewer.agent`, `reviewer.fallback`, `plan.agent`, `watcher.agent`, …). A value is `program/model`, or `program/provider/model` for the
//! programs that carry several providers inside (pi, omp, opencode). The provider's key or sign-in is then looked for first in that
//! agent's own files, so the limits shown are those of the account the board really spends.

/// One use of a provider: the agent (program) it runs through, and the role that says so (`worker.agent`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub provider: &'static str,
    pub agent: &'static str,
    pub role: String,
}

/// The programs a provider's account can be found through.
const AGENTS: [&str; 6] = ["claude", "codex", "kimi", "pi", "omp", "opencode"];

/// The roles in the order they count: the main work first, fallbacks last.
const ROLE_ORDER: [&str; 5] = ["worker.agent", "coordinator.agent", "reviewer.agent", "plan.agent", "watcher.agent"];

fn agent_of(program: &str) -> Option<&'static str> {
    AGENTS.iter().copied().find(|a| *a == program)
}

/// The provider a model name belongs to, as pi and omp name it (`deepseek`, `kimi-code`, `openai-codex`, `anthropic`).
pub fn provider_named(name: &str) -> Option<&'static str> {
    match name.trim().to_ascii_lowercase().as_str() {
        "deepseek" => Some("deepseek"),
        "kimi" | "kimi-code" | "kimi-coding" | "kimi-for-coding" => Some("kimi"),
        "openai" | "openai-codex" | "codex" => Some("codex"),
        "anthropic" | "claude" => Some("claude"),
        _ => None,
    }
}

/// The values one setting holds. The board gives a `*.fallback` as a JSON list (`["pi/openai/gpt-6", "pi/kimi-coding/k3"]`), and its text form
/// may be a comma list too; an `*.agent` is one value.
pub fn values_of(value: &str) -> Vec<String> {
    let v = value.trim();
    if v.starts_with('[') {
        if let Ok(list) = serde_json::from_str::<Vec<serde_json::Value>>(v) {
            return list.iter().filter_map(|x| x.as_str()).map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect();
        }
    }
    v.split([',', '\n']).map(|x| x.trim().trim_matches(['"', '\'', '[', ']']).trim().to_string()).filter(|x| !x.is_empty()).collect()
}

/// The agent and the provider one setting value names: `omp/deepseek/deepseek-v4-flash` is (omp, deepseek), `claude/claude-sonnet-5-5` is
/// (claude, claude), a bare `codex` is (codex, codex). Nothing for a program or a provider the planner does not read.
pub fn route_of(value: &str) -> Option<(&'static str, &'static str)> {
    let mut parts = value.trim().split('/');
    let agent = agent_of(parts.next()?.trim())?;
    match agent {
        "claude" => Some((agent, "claude")),
        "codex" => Some((agent, "codex")),
        "kimi" => Some((agent, "kimi")),
        _ => provider_named(parts.next()?).map(|p| (agent, p)),
    }
}

/// Every provider use among the crew's settings, the main roles first, in the order the board lists the rest.
pub fn routes_of(settings: &[(String, String)]) -> Vec<Route> {
    let rank = |name: &str| -> usize {
        match ROLE_ORDER.iter().position(|r| *r == name) {
            Some(i) => i,
            None if name.ends_with(".agent") => ROLE_ORDER.len(),
            None => ROLE_ORDER.len() + 1,
        }
    };
    let mut named: Vec<(usize, usize, &str, &str)> = settings
        .iter()
        .enumerate()
        .filter(|(_, (name, value))| (name.ends_with(".agent") || name.ends_with(".fallback")) && !value.trim().is_empty())
        .map(|(i, (name, value))| (rank(name), i, name.as_str(), value.as_str()))
        .collect();
    named.sort();
    let mut out: Vec<Route> = Vec::new();
    for (_, _, name, value) in named {
        for one in values_of(value) {
            if let Some((agent, provider)) = route_of(&one) {
                if !out.iter().any(|r| r.provider == provider && r.agent == agent) {
                    out.push(Route { provider, agent, role: name.trim_end_matches(".agent").to_string() });
                }
            }
        }
    }
    out
}

/// The agents to look through for `provider`, best first, each with the role that names it (empty for one the crew does not use): the ones the
/// crew runs it through, then the program that is the provider's own, then the other agents that can hold its account.
pub fn agents_for(routes: &[Route], provider: &str) -> Vec<(&'static str, String)> {
    let mut out: Vec<(&'static str, String)> = routes.iter().filter(|r| r.provider == provider).map(|r| (r.agent, r.role.clone())).collect();
    let own = match provider {
        "claude" => Some("claude"),
        "codex" => Some("codex"),
        "kimi" => Some("kimi"),
        _ => None,
    };
    for a in own.into_iter().chain(["pi", "omp", "opencode"]) {
        if !out.iter().any(|(x, _)| *x == a) {
            out.push((a, String::new()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    #[test]
    fn a_setting_value_names_the_agent_and_the_provider() {
        assert_eq!(route_of("omp/deepseek/deepseek-v4-flash"), Some(("omp", "deepseek")));
        assert_eq!(route_of("pi/deepseek/deepseek-v4-pro"), Some(("pi", "deepseek")));
        assert_eq!(route_of("pi/kimi-coding/k2"), Some(("pi", "kimi")));
        assert_eq!(route_of("omp/openai-codex/gpt-5"), Some(("omp", "codex")));
        assert_eq!(route_of("opencode/anthropic/claude-x"), Some(("opencode", "claude")));
        assert_eq!(route_of("claude/claude-sonnet-5-5"), Some(("claude", "claude")));
        assert_eq!(route_of("codex"), Some(("codex", "codex")));
        assert_eq!(route_of("kimi/kimi-k2"), Some(("kimi", "kimi")));
        assert_eq!(route_of("gemini/gemini-pro"), None, "a program the planner does not read");
        assert_eq!(route_of("omp/openrouter/x"), None, "a provider it does not read");
        assert_eq!(route_of("omp"), None);
        assert_eq!(route_of(""), None);
    }

    #[test]
    fn the_crews_roles_give_the_routes_the_main_roles_first() {
        let settings = s(&[
            ("reviewer.fallback", "omp/deepseek/deepseek-v4-flash"),
            ("coordinator.agent", "claude/claude-opus-5-5"),
            ("watcher.agent", "pi/deepseek/deepseek-v4-flash"),
            ("worker.agent", "omp/deepseek/deepseek-v4-pro"),
            ("worker.effort", "high"),
            ("plan.agent", "claude/claude-haiku-5-5"),
            ("worker.fallback", "gemini/x"),
        ]);
        let routes = routes_of(&settings);
        let shown: Vec<_> = routes.iter().map(|r| format!("{}:{}:{}", r.provider, r.agent, r.role)).collect();
        assert_eq!(shown, ["deepseek:omp:worker", "claude:claude:coordinator", "deepseek:pi:watcher"], "the same pair twice is one; fallbacks come last");
        let agents = agents_for(&routes, "deepseek");
        assert_eq!(agents.iter().map(|(a, _)| *a).collect::<Vec<_>>(), ["omp", "pi", "opencode"]);
        assert_eq!(agents[0].1, "worker");
        assert_eq!(agents[2].1, "", "not used by the crew: still a place to look");
    }

    #[test]
    fn a_provider_the_crew_does_not_use_is_looked_for_in_its_own_program_then_the_others() {
        let agents = agents_for(&[], "claude");
        assert_eq!(agents.iter().map(|(a, _)| *a).collect::<Vec<_>>(), ["claude", "pi", "omp", "opencode"]);
        let agents = agents_for(&[], "deepseek");
        assert_eq!(agents.iter().map(|(a, _)| *a).collect::<Vec<_>>(), ["pi", "omp", "opencode"]);
        // a fallback role counts when nothing else names the provider
        let routes = routes_of(&s(&[("reviewer.fallback", "pi/deepseek/x")]));
        assert_eq!(agents_for(&routes, "deepseek")[0], ("pi", "reviewer.fallback".to_string()));
    }

    #[test]
    fn a_fallback_the_board_gives_as_a_list_is_read_value_by_value() {
        // the lines of a real board: the work through claude, the fallbacks as JSON lists through pi
        let settings = s(&[
            ("worker.agent", "claude/claude-sonnet-5-5"),
            ("worker.fallback", r#"["pi/openai/gpt-6.1-sol","pi/kimi-coding/k3.1-pro","pi/deepseek/deepseek-flash"]"#),
            ("reviewer.fallback", r#"["omp/deepseek/deepseek-v4-flash"]"#),
        ]);
        let routes = routes_of(&settings);
        let shown: Vec<_> = routes.iter().map(|r| format!("{}:{}:{}", r.provider, r.agent, r.role)).collect();
        assert_eq!(shown, ["claude:claude:worker", "codex:pi:worker.fallback", "kimi:pi:worker.fallback", "deepseek:pi:worker.fallback", "deepseek:omp:reviewer.fallback"]);
        assert_eq!(agents_for(&routes, "codex")[0], ("pi", "worker.fallback".to_string()), "pi is the program Codex runs through, before the CLI");
        assert_eq!(agents_for(&routes, "deepseek").iter().map(|(a, _)| *a).collect::<Vec<_>>(), ["pi", "omp", "opencode"]);
    }

    #[test]
    fn the_other_forms_of_a_list_are_read_too() {
        assert_eq!(values_of(r#"["a/b", " c/d/e "]"#), ["a/b", "c/d/e"]);
        assert_eq!(values_of("pi/openai/x, pi/kimi-coding/y,pi/deepseek/z"), ["pi/openai/x", "pi/kimi-coding/y", "pi/deepseek/z"]);
        assert_eq!(values_of("claude/claude-sonnet-5-5"), ["claude/claude-sonnet-5-5"]);
        assert_eq!(values_of("[\"a\", broken"), ["a", "broken"]);
        assert!(values_of("  ").is_empty() && values_of("[]").is_empty());
        let r = routes_of(&s(&[("worker.fallback", "pi/openai/x, pi/kimi-coding/y")]));
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn the_boards_own_show_json_gives_the_routes() {
        let out = r#"{"ok":true,"settings":[
          {"name":"worker.agent","kind":"work","value":"claude/claude-sonnet-5-5","from":"you","place":"you","own":true},
          {"name":"worker.fallback","kind":"work","value":["pi/openai/gpt-6.1-sol","pi/kimi-coding/k3.1-pro","pi/deepseek/deepseek-flash"],"from":"you","place":"you","own":true},
          {"name":"reviewer.fallback","kind":"work","value":["omp/deepseek/deepseek-v4-flash"],"from":"you","place":"you","own":false}]}"#;
        let settings = crate::station::parse_settings(out).unwrap();
        let routes = routes_of(&settings.into_iter().map(|x| (x.name, x.value)).collect::<Vec<_>>());
        assert_eq!(routes.len(), 5);
        assert_eq!(agents_for(&routes, "kimi")[0].0, "pi");
    }
}
