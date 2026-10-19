//! Saved state: the groups of projects and the settings. One JSON file under
//! `$XDG_STATE_HOME/herdr-planner/`; on first run the Tern plugin's saved
//! block state is imported (see `import_tern`).
//!
//! The file also held what belonged to the old TASKS.md path (answers, sent
//! marks, freshness verdicts, the new-project folder). The planner no longer
//! reads those, but it does not throw them away: every key it does not know is
//! carried through a save as it was.

use crate::model::Groups;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
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

pub static SETTING_DEFS: [SettingDef; 2] = [
    SettingDef {
        key: "agent",
        label: "Дешёвый агент",
        hint: "Раскладывает проекты по группам. Только чтение файлов.",
        default: "haiku",
        options: &[("haiku", "Claude Haiku 5.5"), ("deepseek", "DeepSeek Flash (omp)")],
    },
    SettingDef {
        key: "autogroup",
        label: "Автопредложение групп",
        hint: "Дешёвый агент раскладывает проекты без группы по областям (по имени, пути и названиям задач), один раз на проект. Ручной выбор не трогает.",
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
    pub autogroup: String,
    /// Settings of earlier versions, kept as they were.
    #[serde(flatten)]
    pub other: BTreeMap<String, Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            agent: "haiku".into(),
            autogroup: "on".into(),
            other: BTreeMap::new(),
        }
    }
}

impl Settings {
    fn slot(&mut self, key: &str) -> Option<&mut String> {
        match key {
            "agent" => Some(&mut self.agent),
            "autogroup" => Some(&mut self.autogroup),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> &str {
        match key {
            "agent" => &self.agent,
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
pub struct State {
    pub groups: Groups,
    pub settings: Settings,
    /// Where the state came from on first run, for the record.
    pub imported_from: Option<String>,
    /// Keys of earlier versions (`qa`, `sent`, `fresh`, `last_dir`), carried through as they were.
    #[serde(flatten)]
    pub legacy: BTreeMap<String, Value>,
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
        self
    }

    /// Reads the plugin's saved form (`dash.save`): the groups and the settings
    /// (the rest of it belonged to TASKS.md and is not taken over).
    pub fn from_plugin_json(v: &Value) -> State {
        let mut s = State::default();
        if let Some(g) = v.get("groups") {
            s.groups = groups_from_plugin(g);
        }
        if let Some(x) = v.get("settings") {
            s.settings = serde_json::from_value(x.clone()).unwrap_or_default();
        }
        s.normalize()
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
            "qa":{"slug\ntask":[{"q":"a?","a":"b"}]},
            "last_dir":"/home/u/Projects"
        }"#).unwrap();
        let s = State::from_plugin_json(&v);
        assert_eq!(s.groups.list, vec!["Omarchy", "ЯНАО"]);
        assert_eq!(s.groups.group_of("omarchy").0, "Omarchy");
        assert_eq!(s.groups.group_of("x").0, "Прочее");
        assert!(s.groups.is_collapsed("ЯНАО"));
        assert_eq!(s.groups.on, Some(true));
        assert_eq!((s.settings.agent.as_str(), s.settings.autogroup.as_str()), ("deepseek", "off"));
        let lua_empty: Value = serde_json::from_str(r#"{"groups":{"configured":true,"list":["A"],"assign":{"p":{"auto":true,"g":"A"}},"collapsed":{}}}"#).unwrap();
        let s2 = State::from_plugin_json(&lua_empty);
        assert_eq!(s2.groups.group_of("p"), ("A".to_string(), true));
        assert_eq!(s2.settings, Settings::default());
    }

    #[test]
    fn settings_validate_and_fall_back() {
        let mut s = Settings::default();
        assert!(s.set("agent", "deepseek"));
        assert!(!s.set("agent", "gpt"));
        assert!(!s.set("autocheck", "on"), "a setting that is gone");
        assert_eq!(s.agent, "deepseek");
        s.autogroup = "maybe".into();
        let s = s.normalize();
        assert_eq!(s.autogroup, "on");
        assert_eq!(s.get("nope"), "");
    }

    /// The state of an earlier version keeps its groups and its settings, and what
    /// belonged to TASKS.md stays in the file as it was.
    #[test]
    fn an_old_state_file_keeps_everything_through_a_save() {
        let dir = std::env::temp_dir().join(format!("herdr-planner-state-{}", std::process::id()));
        let file = dir.join("state.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&file, r#"{
            "groups":{"configured":true,"list":["Omarchy"],"assign":{"a":{"g":"Omarchy","auto":false}},"collapsed":[],"on":true},
            "settings":{"agent":"deepseek","autocheck":"off","autogroup":"on","review_stale":"7"},
            "qa":{"a\nзадача":[{"q":"вопрос","a":"ответ"}]},
            "sent":{"a\nзадача":true},
            "fresh":{"a":{"head":"abc"}},
            "last_dir":"/home/u/Projects",
            "imported_from":"/x/daemon.state"
        }"#).unwrap();
        let st = State::load(&file).unwrap();
        assert_eq!(st.groups.group_of("a"), ("Omarchy".to_string(), false));
        assert!(st.groups.grouping_on());
        assert_eq!(st.settings.agent, "deepseek");
        assert_eq!(st.imported_from.as_deref(), Some("/x/daemon.state"));
        st.save(&file).unwrap();
        let back: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(back["qa"]["a\nзадача"][0]["a"], "ответ");
        assert_eq!(back["sent"]["a\nзадача"], true);
        assert_eq!(back["fresh"]["a"]["head"], "abc");
        assert_eq!(back["last_dir"], "/home/u/Projects");
        assert_eq!(back["settings"]["review_stale"], "7");
        assert_eq!(back["settings"]["agent"], "deepseek");
        assert_eq!(State::load(&file).unwrap(), st);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn round_trips_through_file() {
        let dir = std::env::temp_dir().join(format!("herdr-planner-store-{}", std::process::id()));
        let file = dir.join("state.json");
        let mut st = State::default().normalize();
        st.groups.assign("apicache", "Omarchy", true);
        st.settings.set("autogroup", "off");
        st.save(&file).unwrap();
        assert_eq!(State::load(&file).unwrap(), st);
        assert!(State::load(&dir.join("missing.json")).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
