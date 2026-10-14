//! Saved state: groups, settings, answers and sent marks, freshness verdicts.
//! One JSON file under `$XDG_STATE_HOME/herdr-planner/`; on first run the
//! Tern plugin's saved block state is imported (see `import_tern`).

// Reference logic: parts are used only by tests and by stages 2–4 (the page mirrors the list logic).
#![allow(dead_code)]

use crate::model::Groups;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

/// Settings (stage 3 edits them; stage 1 keeps and validates them).
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
    /// Saved values that are no longer valid fall back to the default.
    pub fn normalize(mut self) -> Self {
        let d = Settings::default();
        if !["haiku", "deepseek"].contains(&self.agent.as_str()) {
            self.agent = d.agent;
        }
        for v in [&mut self.autocheck, &mut self.autogroup] {
            if !["on", "off"].contains(&v.as_str()) {
                *v = "on".into();
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
    /// Freshness verdicts by slug: `{head, verdicts: {text: {head, v, why}}, kept: {...}}`.
    pub fresh: BTreeMap<String, Value>,
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
        if let Some(Value::Object(fresh)) = v.get("fresh") {
            s.fresh = fresh.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        }
        s.last_dir = v.get("last_dir").and_then(|x| x.as_str()).map(String::from);
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
            "qa":{"slug\ntask":[{"q":"a?","a":"b"},{"q":"pending"}],"legacy":[{"q":"x","error":"e"}]},
            "sent":{"slug\ntask":true},
            "fresh":{"apicache":{"head":"abc","kept":{},"verdicts":{}}},
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
        assert_eq!(s.fresh["apicache"]["head"], "abc");
        assert_eq!(s.last_dir.as_deref(), Some("/home/u/Projects"));
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
