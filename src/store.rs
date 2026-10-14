//! Saved state: groups, settings, answers and sent marks, freshness verdicts.
//! One JSON file under `$XDG_STATE_HOME/herdr-planner/`; on first run the
//! Tern plugin's saved block state is imported (see `import_tern`).

use crate::model::Groups;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

/// One setting of the settings screen: the table below drives the screen, the
/// defaults and the validation, so a new setting is one more entry.
#[derive(Debug, Clone, Serialize)]
pub struct SettingDef {
    pub key: &'static str,
    pub label: &'static str,
    pub hint: &'static str,
    pub default: &'static str,
    pub options: &'static [(&'static str, &'static str)],
}

const ON_OFF: &[(&str, &str)] = &[("on", "вкл"), ("off", "выкл")];

pub static SETTING_DEFS: [SettingDef; 3] = [
    SettingDef {
        key: "agent",
        label: "Дешёвый агент",
        hint: "Отвечает на «Задать вопрос», проверяет актуальность задач и раскладывает проекты по группам. Только чтение файлов.",
        default: "haiku",
        options: &[("haiku", "Claude Haiku 5.5"), ("deepseek", "DeepSeek Flash (omp)")],
    },
    SettingDef {
        key: "autocheck",
        label: "Автопроверка актуальности",
        hint: "Сверяет задачи бэклога открытого проекта с main его репозитория: после новых коммитов, при открытии проекта, для новых задач. Ничего не удаляет.",
        default: "on",
        options: ON_OFF,
    },
    SettingDef {
        key: "autogroup",
        label: "Автопредложение групп",
        hint: "Дешёвый агент раскладывает проекты без группы по областям (по имени и цели), один раз на проект. Ручной выбор не трогает.",
        default: "on",
        options: ON_OFF,
    },
];

pub fn setting_def(key: &str) -> Option<&'static SettingDef> {
    SETTING_DEFS.iter().find(|d| d.key == key)
}

/// Settings, validated against `SETTING_DEFS`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// `haiku` or `deepseek`.
    pub agent: String,
    /// `on` / `off`.
    pub autocheck: String,
    pub autogroup: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            agent: "haiku".into(),
            autocheck: "on".into(),
            autogroup: "on".into(),
        }
    }
}

impl Settings {
    fn slot(&mut self, key: &str) -> Option<&mut String> {
        match key {
            "agent" => Some(&mut self.agent),
            "autocheck" => Some(&mut self.autocheck),
            "autogroup" => Some(&mut self.autogroup),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> &str {
        match key {
            "agent" => &self.agent,
            "autocheck" => &self.autocheck,
            "autogroup" => &self.autogroup,
            _ => "",
        }
    }

    /// Sets `key` to `value` when both are known; the old value stays otherwise.
    pub fn set(&mut self, key: &str, value: &str) -> bool {
        let valid = setting_def(key).is_some_and(|d| d.options.iter().any(|(v, _)| *v == value));
        match self.slot(key) {
            Some(slot) if valid => {
                *slot = value.to_string();
                true
            }
            _ => false,
        }
    }

    /// Saved values that are no longer valid fall back to the default.
    pub fn normalize(mut self) -> Self {
        for d in &SETTING_DEFS {
            let value = self.get(d.key).to_string();
            if !d.options.iter().any(|(v, _)| *v == value) {
                if let Some(slot) = self.slot(d.key) {
                    *slot = d.default.to_string();
                }
            }
        }
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct QaEntry {
    pub q: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub a: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Asked and not answered yet; never survives a restart.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub pending: bool,
}

/// What the freshness check said about one task at one commit of main.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Verdict {
    /// The commit of main the verdict is for.
    pub head: String,
    /// `actual`, `partial` or `done`.
    pub v: String,
    pub why: String,
    /// For `done`: the file in the repository that does the task.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proof: Option<String>,
}

/// One project's cached freshness verdicts, by task text.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FreshCache {
    /// The commit of main checked last.
    pub head: Option<String>,
    #[serde(deserialize_with = "lenient_map")]
    pub verdicts: BTreeMap<String, Verdict>,
    /// «Оставить»: task text → the commit of main the badge is hidden at.
    #[serde(deserialize_with = "lenient_map")]
    pub kept: BTreeMap<String, String>,
}

/// A map that tolerates what Lua makes of an empty table (`[]`) and drops the
/// entries that do not fit.
fn lenient_map<'de, D, V>(d: D) -> Result<BTreeMap<String, V>, D::Error>
where
    D: Deserializer<'de>,
    V: serde::de::DeserializeOwned,
{
    Ok(match Value::deserialize(d)? {
        Value::Object(m) => m
            .into_iter()
            .filter_map(|(k, x)| serde_json::from_value(x).ok().map(|x| (k, x)))
            .collect(),
        _ => BTreeMap::new(),
    })
}

/// The key of a task's answers and «sent» mark: the project, a line break,
/// the task text. A task text is one line, so keys from before 0.15.4 of the
/// plugin (text only) are told apart by the missing line break.
pub fn mem_key(slug: &str, text: &str) -> String {
    format!("{slug}\n{text}")
}

fn legacy_key(key: &str) -> bool {
    !key.contains('\n')
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub groups: Groups,
    pub settings: Settings,
    /// Questions and answers by `slug\ntext` (legacy keys: text only).
    pub qa: BTreeMap<String, Vec<QaEntry>>,
    /// «sent to the coordinator» marks by the same key.
    pub sent: BTreeMap<String, Value>,
    /// Freshness verdicts by slug (format as the plugin's `dash.save`).
    #[serde(deserialize_with = "lenient_map")]
    pub fresh: BTreeMap<String, FreshCache>,
    /// The folder the "new project" browser opened last.
    pub last_dir: Option<String>,
    /// Where the state came from on first run, for the record.
    pub imported_from: Option<String>,
}

/// Lua tables arrive as JSON objects or arrays: an empty `collapsed` or
/// `assign` is `{}` or `[]`, so each field is read on its own.
fn groups_from_plugin(g: &Value) -> Groups {
    let mut out = Groups {
        configured: g
            .get("configured")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        ..Default::default()
    };
    if let Some(list) = g.get("list").and_then(Value::as_array) {
        out.list = list
            .iter()
            .filter_map(Value::as_str)
            .map(String::from)
            .collect();
    }
    if let Some(assign) = g.get("assign").and_then(Value::as_object) {
        for (slug, e) in assign {
            let name = e.get("g").and_then(Value::as_str).unwrap_or("");
            let auto = e.get("auto").and_then(Value::as_bool).unwrap_or(false);
            out.assign.insert(
                slug.clone(),
                crate::model::Assign {
                    g: name.to_string(),
                    auto,
                },
            );
        }
    }
    if let Some(c) = g.get("collapsed").and_then(Value::as_array) {
        out.collapsed = c
            .iter()
            .filter_map(Value::as_str)
            .map(String::from)
            .collect();
    }
    out.on = g.get("on").and_then(Value::as_bool);
    out
}

impl State {
    /// A saved state (any shape) made consistent.
    pub fn normalize(mut self) -> Self {
        self.groups = Groups::normalize(Some(std::mem::take(&mut self.groups)));
        self.settings = self.settings.normalize();
        for cache in self.fresh.values_mut() {
            let ok = |v: &str| ["actual", "partial", "done"].contains(&v);
            cache.verdicts.retain(|_, e| ok(&e.v) && !e.head.is_empty());
        }
        // An answer still pending when the plugin stopped never arrives.
        for list in self.qa.values_mut() {
            list.retain(|e| e.a.is_some() || e.error.is_some());
        }
        self.qa.retain(|_, l| !l.is_empty());
        self
    }

    /// Reads the plugin's saved form (`dash.save`): the same keys, Lua-shaped.
    pub fn from_plugin_json(v: &Value) -> State {
        let mut s = State::default();
        if let Some(g) = v.get("groups") {
            s.groups = groups_from_plugin(g);
        }
        if let Some(x) = v.get("settings") {
            s.settings = serde_json::from_value(x.clone()).unwrap_or_default();
        }
        if let Some(Value::Object(qa)) = v.get("qa") {
            for (k, list) in qa {
                let entries: Vec<QaEntry> =
                    serde_json::from_value(list.clone()).unwrap_or_default();
                s.qa.insert(k.clone(), entries);
            }
        }
        if let Some(Value::Object(sent)) = v.get("sent") {
            s.sent = sent.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        }
        if let Some(fresh) = v.get("fresh") {
            s.fresh = lenient_map(fresh.clone()).unwrap_or_default();
        }
        s.last_dir = v.get("last_dir").and_then(|x| x.as_str()).map(String::from);
        s.normalize()
    }

    /// The answers of one task, newest first.
    pub fn qa_list(&mut self, slug: &str, text: &str) -> &mut Vec<QaEntry> {
        self.qa.entry(mem_key(slug, text)).or_default()
    }

    /// Only a TASKS.md that was read and has tasks counts: an empty read may
    /// be a file being rewritten.
    pub fn trusted(tasks: usize, read: bool) -> bool {
        read && tasks > 0
    }

    /// After a trusted read of `slug`'s TASKS.md (`texts`: the tasks, done
    /// ones too): keys of the old shape are copied to it, its keys for tasks
    /// that are gone are dropped. `read` is the set of projects read so far;
    /// once it covers `all`, the old keys go for good.
    pub fn settle(
        &mut self,
        slug: &str,
        texts: &HashSet<String>,
        read: &mut HashSet<String>,
        all: &[String],
    ) {
        let prefix = format!("{slug}\n");
        fn settle_map<V: Clone>(
            map: &mut BTreeMap<String, V>,
            slug: &str,
            prefix: &str,
            texts: &HashSet<String>,
        ) {
            let copies: Vec<(String, V)> = map
                .iter()
                .filter(|(k, _)| legacy_key(k) && texts.contains(*k))
                .filter(|(k, _)| !map.contains_key(&mem_key(slug, k)))
                .map(|(k, v)| (mem_key(slug, k), v.clone()))
                .collect();
            map.extend(copies);
            map.retain(|k, _| {
                legacy_key(k) || !k.starts_with(prefix) || texts.contains(&k[prefix.len()..])
            });
        }
        settle_map(&mut self.qa, slug, &prefix, texts);
        settle_map(&mut self.sent, slug, &prefix, texts);
        read.insert(slug.to_string());
        if !all.is_empty() && all.iter().all(|p| read.contains(p)) {
            self.qa.retain(|k, _| !legacy_key(k));
            self.sent.retain(|k, _| !legacy_key(k));
        }
    }

    pub fn load(path: &Path) -> Option<State> {
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str::<State>(&text)
            .ok()
            .map(State::normalize)
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_plugin_shape() {
        let v: Value = serde_json::from_str(r#"{
            "groups":{"configured":true,"list":["Omarchy","ЯНАО"],"assign":{"omarchy":{"auto":true,"g":"Omarchy"},"x":{"auto":false,"g":"Nope"}},"collapsed":["ЯНАО"],"on":true},
            "settings":{"agent":"deepseek","autocheck":"maybe","autogroup":"off"},
            "qa":{"slug\ntask":[{"q":"a?","a":"b"},{"q":"pending"}],"legacy":[{"q":"x","error":"e"}]},
            "sent":{"slug\ntask":true},
            "fresh":{"apicache":{"head":"abc","kept":[],"verdicts":{"Задача":{"head":"abc","v":"partial","why":"часть"},"Мусор":{"head":"abc","v":"???"}}}},
            "last_dir":"/home/u/Projects"
        }"#).unwrap();
        let s = State::from_plugin_json(&v);
        assert_eq!(s.groups.list, vec!["Omarchy", "ЯНАО"]);
        assert_eq!(s.groups.group_of("omarchy").0, "Omarchy");
        assert_eq!(s.groups.group_of("x").0, "Прочее");
        assert!(s.groups.is_collapsed("ЯНАО"));
        assert_eq!(s.groups.on, Some(true));
        let lua_empty: Value = serde_json::from_str(r#"{"groups":{"configured":true,"list":["A"],"assign":{"p":{"auto":true,"g":"A"}},"collapsed":{}}}"#).unwrap();
        let s2 = State::from_plugin_json(&lua_empty);
        assert_eq!(s2.groups.group_of("p"), ("A".to_string(), true));
        assert_eq!(s.settings.agent, "deepseek");
        assert_eq!(s.settings.autocheck, "on");
        assert_eq!(s.qa["slug\ntask"].len(), 1);
        assert_eq!(s.qa["legacy"][0].error.as_deref(), Some("e"));
        assert_eq!(s.sent.len(), 1);
        assert_eq!(s.fresh["apicache"].head.as_deref(), Some("abc"));
        assert_eq!(s.fresh["apicache"].verdicts["Задача"].v, "partial");
        assert!(!s.fresh["apicache"].verdicts.contains_key("Мусор"));
        assert!(s.fresh["apicache"].kept.is_empty());
        assert_eq!(s.last_dir.as_deref(), Some("/home/u/Projects"));
    }

    #[test]
    fn settle_keeps_current_tasks_and_migrates_old_keys() {
        let mut s = State::default();
        s.qa.insert(
            "a\nkeep".into(),
            vec![QaEntry {
                q: "q".into(),
                a: Some("x".into()),
                ..Default::default()
            }],
        );
        s.qa.insert("a\ngone".into(), vec![]);
        s.qa.insert("b\nother".into(), vec![]);
        s.qa.insert(
            "old".into(),
            vec![QaEntry {
                q: "o".into(),
                a: Some("y".into()),
                ..Default::default()
            }],
        );
        s.sent.insert("a\nkeep".into(), Value::Bool(true));
        s.sent.insert("a\ngone".into(), Value::Bool(true));
        let mut read = HashSet::new();
        let texts: HashSet<String> = ["keep".to_string(), "old".to_string()].into();
        s.settle("a", &texts, &mut read, &["a".into(), "b".into()]);
        assert!(s.qa.contains_key("a\nkeep"));
        assert!(
            !s.qa.contains_key("a\ngone"),
            "a task that is gone loses its answers"
        );
        assert!(
            s.qa.contains_key("b\nother"),
            "other projects are not touched"
        );
        assert!(
            s.qa.contains_key("a\nold"),
            "the old key is copied to the project that has the text"
        );
        assert!(
            s.qa.contains_key("old"),
            "…and stays until every project was read"
        );
        assert!(!s.sent.contains_key("a\ngone"));
        s.settle("b", &HashSet::new(), &mut read, &["a".into(), "b".into()]);
        assert!(
            !s.qa.contains_key("old"),
            "every project read: the old key goes"
        );
        assert!(!s.qa.contains_key("b\nother"));
        assert!(State::trusted(2, true) && !State::trusted(0, true) && !State::trusted(2, false));
    }

    #[test]
    fn pending_answers_do_not_survive() {
        let mut s = State::default();
        s.qa_list("a", "t").push(QaEntry {
            q: "wait".into(),
            pending: true,
            ..Default::default()
        });
        s.qa_list("a", "t").push(QaEntry {
            q: "done".into(),
            a: Some("ok".into()),
            ..Default::default()
        });
        let json = serde_json::to_string(&s).unwrap();
        let back: State = serde_json::from_str(&json).unwrap();
        let back = back.normalize();
        assert_eq!(back.qa["a\nt"].len(), 1);
        assert_eq!(back.qa["a\nt"][0].q, "done");
        assert!(!serde_json::to_string(&back.qa["a\nt"][0])
            .unwrap()
            .contains("pending"));
    }

    #[test]
    fn round_trips_through_file() {
        let dir = std::env::temp_dir().join(format!("herdr-planner-test-{}", std::process::id()));
        let path = dir.join("state.json");
        let mut s = State::default().normalize();
        s.groups.assign("a", "Omarchy", false);
        s.save(&path).unwrap();
        let back = State::load(&path).unwrap();
        assert_eq!(back, s);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
