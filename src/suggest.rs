//! Group suggestion: the cheap agent sorts projects without a group by area.
//! One call for every project with no entry yet (a new project gets its
//! guess once, when it appears), the answer fills only those that are still
//! without — the user may have picked meanwhile. A manual pick is never
//! touched; «Разложить заново» forgets only the guesses and asks again.
//! Ported from the Tern plugin (0.17.1).

use crate::agent::{self, Kind};
use crate::app::{now_ms, Shared};
use crate::jsonscan;
use crate::model::{group_name_clean, Project, MAX_GROUPS};
use crate::sources::tasks;
use serde_json::Value;
use std::collections::BTreeMap;

/// After a failed or unreadable answer the next try waits this long.
pub const RETRY_MS: u64 = 300_000;
/// The agent may name at most this many new groups per answer.
pub const MAX_NEW_GROUPS: usize = 3;

/// The starting groups and what each is about (the agent reads these).
pub const DEFAULT_HINTS: [(&str, &str); 5] = [
    ("Omarchy", "проекты вокруг Linux-окружения Omarchy: плагины, виджеты, темы, утилиты этой машины"),
    ("ЯНАО", "проекты для Ямало-Ненецкого автономного округа: ИТ-платформы, ГИС, городские и ведомственные сервисы"),
    ("ИИ-инфраструктура", "серверы моделей, каталоги и шаблоны API, развёртывание ИИ-сервисов"),
    ("Агенты и herdr", "herdr, Tern, агенты, их обвязка и оркестрация"),
    ("Личное", "личные заметки, голос, погода и прочие личные проекты"),
];

#[derive(Debug, Default)]
pub struct GroupRun {
    pub busy: bool,
    pub error: Option<String>,
    pub retry_after: Option<u64>,
    /// «Разложить заново» was asked while a call was running.
    pub again: bool,
}

pub const SYSTEM: &str = "Ты раскладываешь проекты по тематическим группам — по ОБЛАСТИ, а не по типу работы \
(не «боты», «скрипты», «плагины», «бэкенд»). Ты НЕ координатор и ничего не меняешь; файлы читать не нужно, \
всё нужное — в запросе. Ответь СТРОГО одним JSON-массивом, без текста до и после, без ``` и без пояснений: \
[{\"slug\":\"…\",\"group\":\"…\"}, …] — по одному объекту на каждый проект списка. \
slug — как в списке; group — ровно одно название из списка групп, написанное точно так же. \
Новую группу можно назвать только если проект ЯВНО не подходит ни к одной из групп списка \
(короткое название области до 3 слов; близким проектам давай одно и то же новое название). \
Не уверен — выбери ближайшую группу из списка.";

/// The request: the groups (with their hints) and every project's name and goal.
pub fn prompt(list: &[String], projects: &[&Project]) -> String {
    let mut lines = vec!["Группы:".to_string()];
    for name in list {
        let hint = DEFAULT_HINTS
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, h)| *h);
        lines.push(match hint {
            Some(h) => format!("- {name} — {h}"),
            None => format!("- {name}"),
        });
    }
    lines.push(String::new());
    lines.push("Проекты (slug | название | цель):".into());
    for p in projects {
        let goal = p
            .goal
            .as_deref()
            .unwrap_or("")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        lines.push(format!(
            "{} | {} | {}",
            p.slug,
            if p.name.is_empty() { &p.slug } else { &p.name },
            if goal.is_empty() {
                "(цель не задана)".to_string()
            } else {
                tasks::clip(&goal, 300)
            }
        ));
    }
    lines.join("\n")
}

fn index_in(list: &[String], name: &str) -> Option<usize> {
    let key = name.to_lowercase();
    list.iter().position(|g| g.to_lowercase() == key)
}

/// `{slug → raw group string}` from one decoded answer: an array of rows, a
/// `slug → group` object, a `group → [slugs]` object, or any of them wrapped
/// in one more object. Nothing else is believed.
pub fn group_pairs(data: &Value, projects: &[&Project]) -> BTreeMap<String, String> {
    let mut lookup: BTreeMap<String, String> = BTreeMap::new();
    for p in projects {
        lookup.insert(p.slug.to_lowercase(), p.slug.clone());
    }
    for p in projects {
        if !p.name.is_empty() {
            lookup
                .entry(p.name.to_lowercase())
                .or_insert_with(|| p.slug.clone());
        }
    }
    let mut found: BTreeMap<String, String> = BTreeMap::new();
    let slug_of = |k: &Value| -> Option<String> {
        k.as_str()
            .and_then(|s| lookup.get(&s.trim().to_lowercase()).cloned())
    };
    fn put(found: &mut BTreeMap<String, String>, slug: Option<String>, g: Option<&Value>) {
        if let Some(slug) = slug {
            found
                .entry(slug)
                .or_insert_with(|| g.and_then(Value::as_str).unwrap_or("").to_string());
        }
    }
    fn walk(
        d: &Value,
        depth: u32,
        in_group: Option<&str>,
        found: &mut BTreeMap<String, String>,
        slug_of: &dyn Fn(&Value) -> Option<String>,
    ) {
        if depth > 3 {
            return;
        }
        let group_value = in_group.map(|g| Value::String(g.to_string()));
        match d {
            Value::Array(rows) => {
                for row in rows {
                    match row {
                        Value::String(_) => {
                            if group_value.is_some() {
                                put(found, slug_of(row), group_value.as_ref());
                            }
                        }
                        Value::Object(o) => {
                            let members = ["projects", "slugs", "items"]
                                .iter()
                                .find_map(|k| o.get(*k));
                            let gname = ["group", "name", "title"].iter().find_map(|k| o.get(*k));
                            if let (Some(m @ Value::Array(_)), Some(Value::String(g))) =
                                (members, gname)
                            {
                                walk(m, depth + 1, Some(g), found, slug_of);
                                continue;
                            }
                            let key = ["slug", "project", "id", "name"]
                                .iter()
                                .find_map(|k| o.get(*k));
                            match key.and_then(slug_of) {
                                Some(slug) => {
                                    let g = ["group", "g", "category", "area"]
                                        .iter()
                                        .find_map(|k| o.get(*k))
                                        .or(group_value.as_ref());
                                    put(found, Some(slug), g);
                                }
                                None => walk(row, depth + 1, in_group, found, slug_of),
                            }
                        }
                        Value::Array(_) => walk(row, depth + 1, in_group, found, slug_of),
                        _ => {}
                    }
                }
            }
            Value::Object(o) => {
                for (k, v) in o {
                    let key = Value::String(k.clone());
                    match v {
                        Value::String(_) if slug_of(&key).is_some() => {
                            put(found, slug_of(&key), Some(v));
                        }
                        Value::Array(a) => {
                            let named =
                                a.first().is_some_and(Value::is_string) && slug_of(&key).is_none();
                            walk(
                                v,
                                depth + 1,
                                if named { Some(k) } else { in_group },
                                found,
                                slug_of,
                            );
                        }
                        Value::Object(_) => walk(v, depth + 1, in_group, found, slug_of),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    walk(data, 0, None, &mut found, &slug_of);
    found
}

const NO_GROUP_WORDS: [&str; 6] = ["-", "none", "null", "без группы", "нет", "другое"];

/// What the agent said as `{slug → group name or ""}` over `asked`, and the
/// new group names it proposed (at most `MAX_NEW_GROUPS`).
pub fn parse_groups(
    text: &str,
    asked: &[&Project],
    list: &[String],
) -> Result<(BTreeMap<String, String>, Vec<String>), String> {
    let found_all = jsonscan::candidates(text);
    if found_all.is_empty() {
        return Err("агент ответил не JSON".into());
    }
    let found = found_all
        .iter()
        .map(|d| group_pairs(d, asked))
        .find(|f| !f.is_empty())
        .ok_or("в ответе агента нет групп")?;
    let mut known: Vec<String> = list.to_vec();
    let (mut out, mut fresh) = (BTreeMap::new(), Vec::new());
    for p in asked {
        let mut name = String::new();
        if let Some(raw) = found.get(&p.slug).filter(|r| !r.is_empty()) {
            if !NO_GROUP_WORDS.contains(&raw.trim().to_lowercase().as_str()) {
                if let Some(clean) = group_name_clean(raw) {
                    if let Some(at) = index_in(&known, &clean) {
                        name = known[at].clone();
                    } else if fresh.len() < MAX_NEW_GROUPS && known.len() < MAX_GROUPS {
                        known.push(clean.clone());
                        fresh.push(clean.clone());
                        name = clean;
                    }
                }
            }
        }
        out.insert(p.slug.clone(), name);
    }
    Ok((out, fresh))
}

// ---------------------------------------------------------------------------
// Running

/// After every list read: new projects get their guess once. With the setting
/// off nothing runs.
pub fn tick(app: &Shared) {
    if app.state.lock().unwrap().settings.autogroup != "on" {
        return;
    }
    start(app);
}

/// «Разложить заново»: forgets the agent's guesses (manual picks stay) and
/// guesses again, whatever the setting says.
pub fn resort(app: &Shared) {
    app.state.lock().unwrap().groups.forget_auto();
    app.save_state();
    {
        let mut run = app.group_run.lock().unwrap();
        run.error = None;
        run.retry_after = None;
        if run.busy {
            run.again = true;
            return;
        }
    }
    app.emit_snapshot();
    start(app);
}

fn start(app: &Shared) {
    let projects = app.snapshot.read().unwrap().projects.clone();
    let (list, asked, kind) = {
        let st = app.state.lock().unwrap();
        let asked: Vec<Project> = st
            .groups
            .unassigned(&projects)
            .into_iter()
            .cloned()
            .collect();
        (
            st.groups.list.clone(),
            asked,
            Kind::from_setting(&st.settings.agent),
        )
    };
    if list.is_empty() || asked.is_empty() {
        return;
    }
    {
        let mut run = app.group_run.lock().unwrap();
        if run.busy || run.retry_after.is_some_and(|t| now_ms() < t) {
            return;
        }
        run.busy = true;
    }
    app.emit_snapshot();
    let app = app.clone();
    tokio::spawn(async move {
        let refs: Vec<&Project> = asked.iter().collect();
        let req = agent::Request {
            system: SYSTEM.into(),
            prompt: prompt(&list, &refs),
            cwd: app.config.root.display().to_string(),
            dirs: vec![],
        };
        let result = match agent::run(kind, &req).await {
            Ok(text) => parse_groups(&text, &refs, &list),
            Err(e) => Err(e),
        };
        let again = {
            let mut run = app.group_run.lock().unwrap();
            run.busy = false;
            match &result {
                Ok(_) => {
                    run.error = None;
                    run.retry_after = None;
                }
                Err(e) => {
                    run.error = Some(e.clone());
                    run.retry_after = Some(now_ms() + RETRY_MS);
                }
            }
            std::mem::take(&mut run.again)
        };
        if let Ok((assigned, fresh)) = result {
            apply(&app, &asked, &assigned, &fresh);
            app.save_state();
        }
        app.emit_snapshot();
        if again {
            start(&app);
        }
    });
}

/// Fills only the projects that are still without an entry.
fn apply(app: &Shared, asked: &[Project], assigned: &BTreeMap<String, String>, fresh: &[String]) {
    let mut st = app.state.lock().unwrap();
    for p in asked {
        if st.groups.assign.contains_key(&p.slug) {
            continue;
        }
        let mut g = assigned.get(&p.slug).cloned().unwrap_or_default();
        if !g.is_empty() && st.groups.index(&g).is_none() {
            if fresh.contains(&g) && st.groups.list.len() < MAX_GROUPS {
                st.groups.list.push(g.clone());
            } else {
                g.clear();
            }
        }
        st.groups.assign(&p.slug, &g, true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(slug: &str, name: &str, goal: &str) -> Project {
        Project {
            slug: slug.into(),
            name: name.into(),
            goal: (!goal.is_empty()).then(|| goal.to_string()),
            ..Default::default()
        }
    }

    fn list() -> Vec<String> {
        vec!["Omarchy".into(), "Личное".into()]
    }

    fn parse(
        text: &str,
        asked: &[Project],
    ) -> Result<(BTreeMap<String, String>, Vec<String>), String> {
        let refs: Vec<&Project> = asked.iter().collect();
        parse_groups(text, &refs, &list())
    }

    #[test]
    fn the_prompt_names_groups_with_hints_and_projects() {
        let a = p("bar", "Бар", "Виджет бара   Omarchy");
        let b = p("x", "", "");
        let text = prompt(&["Omarchy".into(), "Своя".into()], &[&a, &b]);
        assert!(text.starts_with("Группы:\n- Omarchy — проекты вокруг Linux-окружения Omarchy"));
        assert!(
            text.contains("\n- Своя\n"),
            "a group without a hint is just its name"
        );
        assert!(text.contains("bar | Бар | Виджет бара Omarchy"));
        assert!(text.contains("x | x | (цель не задана)"));
    }

    #[test]
    fn reads_rows_objects_and_wrappers() {
        let asked = [p("a", "A", ""), p("b", "Bee", "")];
        for text in [
            r#"[{"slug":"a","group":"Omarchy"},{"slug":"b","group":"Личное"}]"#,
            r#"Вот: {"a":"Omarchy","b":"Личное"} всё"#,
            r#"```json
{"Omarchy":["a"],"Личное":["b"]}
```"#,
            r#"{"result":[{"project":"a","g":"Omarchy"},{"name":"Bee","area":"Личное"}]}"#,
            r#"{"groups":[{"group":"Omarchy","projects":["a"]},{"name":"Личное","slugs":["B"]}]}"#,
        ] {
            let (got, fresh) = parse(text, &asked).unwrap_or_else(|e| panic!("{text}: {e}"));
            assert_eq!(got["a"], "Omarchy", "{text}");
            assert_eq!(got["b"], "Личное", "{text}");
            assert!(fresh.is_empty(), "{text}");
        }
    }

    #[test]
    fn unknown_projects_and_words_are_not_believed() {
        let asked = [p("a", "A", ""), p("b", "B", "")];
        let (got, fresh) = parse(r#"[{"slug":"a","group":"none"},{"slug":"zzz","group":"Omarchy"},{"slug":"b","group":"Прочее"}]"#, &asked).unwrap();
        assert_eq!(got["a"], "", "«none» means no group");
        assert_eq!(got["b"], "", "«Прочее» is reserved");
        assert!(!got.contains_key("zzz") && fresh.is_empty());
        assert_eq!(
            parse("текст без json", &asked).unwrap_err(),
            "агент ответил не JSON"
        );
        assert_eq!(
            parse(r#"[{"slug":"nobody","group":"x"}]"#, &asked).unwrap_err(),
            "в ответе агента нет групп"
        );
        assert_eq!(
            parse("[]", &asked).unwrap_err(),
            "в ответе агента нет групп"
        );
    }

    #[test]
    fn new_groups_are_few_and_named_once() {
        let asked: Vec<Project> = ["a", "b", "c", "d", "e"]
            .iter()
            .map(|s| p(s, s, ""))
            .collect();
        let (got, fresh) = parse(
            r#"[{"slug":"a","group":"омарчи"},{"slug":"b","group":"Музыка"},{"slug":"c","group":"музыка"},{"slug":"d","group":"Кино"},{"slug":"e","group":"Книги"}]"#,
            &asked,
        )
        .unwrap();
        assert_eq!(
            got["a"], "омарчи",
            "a near miss in spelling is a new name, not a guess"
        );
        assert_eq!(
            fresh,
            ["омарчи", "Музыка", "Кино"],
            "at most three new groups"
        );
        assert_eq!(
            got["c"], "Музыка",
            "the same new name, any case, is one group"
        );
        assert_eq!(got["e"], "", "the fourth new name is dropped");
    }
}
