//! The colours of the current Omarchy theme, handed to the page.
//!
//! Source: `~/.local/state/omarchy/current/theme/colors.toml`; the name is in
//! `~/.local/state/omarchy/current/theme.name`. A missing or broken file falls
//! back to Americano Night, the palette the design was drawn with.

use crate::config::state_home;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize)]
pub struct Theme {
    pub name: String,
    pub mode: String,
    pub colors: BTreeMap<String, String>,
    /// Where the colours came from; empty for the fallback.
    pub source: String,
    /// Why the fallback was used, if it was.
    pub error: String,
}

fn fallback() -> BTreeMap<String, String> {
    let pairs = [
        ("bg", "#1a120c"),
        ("bg-dark", "#140e09"),
        ("bg-darker", "#0d0906"),
        ("bg-light", "#2a201a"),
        ("fg", "#ecded6"),
        ("fg-bright", "#fcf5f1"),
        ("muted", "#806f64"),
        ("accent", "#dfa360"),
        ("sel", "#513b24"),
        ("red", "#e48682"),
        ("yellow", "#f6bb6f"),
        ("green", "#98bf7a"),
        ("cyan", "#5bcccb"),
        ("blue", "#77b0e9"),
        ("magenta", "#da8fbf"),
        ("border-a", "#dfa360"),
        ("border-b", "#7ea9d5"),
    ];
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

pub fn theme_dir() -> PathBuf {
    std::env::var_os("OMARCHY_THEME_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| state_home().join("omarchy/current/theme"))
}

pub fn name_file(theme_dir: &std::path::Path) -> PathBuf {
    theme_dir
        .parent()
        .map(|p| p.join("theme.name"))
        .unwrap_or_else(|| theme_dir.join("theme.name"))
}

pub fn load() -> Theme {
    let dir = theme_dir();
    let colors_file = dir.join("colors.toml");
    let name = std::fs::read_to_string(name_file(&dir))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Omarchy".to_string());
    let text = match std::fs::read_to_string(&colors_file) {
        Ok(t) => t,
        Err(e) => {
            return Theme {
                name,
                mode: "dark".into(),
                colors: fallback(),
                source: String::new(),
                error: format!("{}: {e}", colors_file.display()),
            }
        }
    };
    match parse(&text) {
        Ok((mode, colors)) => Theme {
            name,
            mode,
            colors,
            source: colors_file.display().to_string(),
            error: String::new(),
        },
        Err(e) => Theme {
            name,
            mode: "dark".into(),
            colors: fallback(),
            source: colors_file.display().to_string(),
            error: e,
        },
    }
}

fn get<'a>(v: &'a toml::Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(toml::Value::as_str)
}

/// The first two colours of a Hyprland gradient (`rgba(dfa360ee) rgba(7ea9d5ee) 45deg`).
fn gradient_colors(value: &str) -> (Option<String>, Option<String>) {
    let mut out: Vec<String> = Vec::new();
    let mut rest = value;
    while let Some(start) = rest.find("rgba(") {
        rest = &rest[start + 5..];
        let Some(end) = rest.find(')') else { break };
        let raw = rest[..end].trim();
        let hex = raw
            .chars()
            .take_while(|c| c.is_ascii_hexdigit())
            .take(6)
            .collect::<String>();
        if hex.len() == 6 {
            out.push(format!("#{}", hex.to_ascii_lowercase()));
        }
        rest = &rest[end + 1..];
        if out.len() == 2 {
            break;
        }
    }
    if out.is_empty() {
        // A plain `#rrggbb #rrggbb 45deg` gradient.
        for token in value.split_whitespace().take(2) {
            if token.starts_with('#') && token.len() == 7 {
                out.push(token.to_ascii_lowercase());
            }
        }
    }
    (
        out.first().cloned(),
        out.get(1).cloned().or_else(|| out.first().cloned()),
    )
}

/// Parses `colors.toml` into the design's CSS layer-1 names.
pub fn parse(text: &str) -> Result<(String, BTreeMap<String, String>), String> {
    let v: toml::Value = toml::from_str(text).map_err(|e| e.to_string())?;
    let mut c = fallback();
    let map = [
        ("bg", "background"),
        ("bg-dark", "dark_background"),
        ("bg-darker", "darker_background"),
        ("bg-light", "lighter_background"),
        ("fg", "foreground"),
        ("fg-bright", "bright_foreground"),
        ("muted", "muted"),
        ("accent", "accent"),
        ("sel", "selection"),
        ("red", "red"),
        ("yellow", "yellow"),
        ("green", "green"),
        ("cyan", "cyan"),
        ("blue", "blue"),
        ("magenta", "magenta"),
    ];
    for (to, from) in map {
        if let Some(value) = get(&v, from) {
            if value.starts_with('#') && (value.len() == 7 || value.len() == 4) {
                c.insert(to.to_string(), value.to_ascii_lowercase());
            }
        }
    }
    if let Some(border) = get(&v, "hyprland_active_border") {
        let (a, b) = gradient_colors(border);
        if let Some(a) = a {
            c.insert("border-a".into(), a);
        }
        if let Some(b) = b {
            c.insert("border-b".into(), b);
        }
    } else {
        let accent = c.get("accent").cloned().unwrap_or_else(|| "#dfa360".into());
        c.insert("border-a".into(), accent.clone());
        c.insert("border-b".into(), accent);
    }
    let mode = get(&v, "mode").unwrap_or("dark");
    Ok((
        if mode == "light" { "light" } else { "dark" }.to_string(),
        c,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r##"
mode = "light"
accent = "#9C610B"
background = "#f9f1e8"
dark_background = "#f1e7dc"
darker_background = "#e4d9ce"
lighter_background = "#eaded1"
foreground = "#372b1f"
bright_foreground = "#1d140a"
muted = "#84786a"
selection = "#e5d1b7"
red = "#a54746"
yellow = "#996306"
green = "#52792f"
cyan = "#007a7a"
blue = "#29669f"
magenta = "#954b7c"
hyprland_active_border = "rgba(9c610bee) rgba(4875a3ee) 45deg"
"##;

    #[test]
    fn reads_every_design_colour() {
        let (mode, c) = parse(SAMPLE).unwrap();
        assert_eq!(mode, "light");
        assert_eq!(c["bg"], "#f9f1e8");
        assert_eq!(c["bg-dark"], "#f1e7dc");
        assert_eq!(c["bg-darker"], "#e4d9ce");
        assert_eq!(c["bg-light"], "#eaded1");
        assert_eq!(c["fg"], "#372b1f");
        assert_eq!(c["fg-bright"], "#1d140a");
        assert_eq!(c["accent"], "#9c610b");
        assert_eq!(c["border-a"], "#9c610b");
        assert_eq!(c["border-b"], "#4875a3");
    }

    #[test]
    fn missing_keys_fall_back_and_missing_border_uses_accent() {
        let (_, c) = parse("mode = \"dark\"\naccent = \"#112233\"\n").unwrap();
        assert_eq!(c["accent"], "#112233");
        assert_eq!(c["border-a"], "#112233");
        assert_eq!(c["border-b"], "#112233");
        assert_eq!(c["bg"], "#1a120c");
    }

    #[test]
    fn broken_toml_is_an_error() {
        assert!(parse("accent = [").is_err());
    }
}
