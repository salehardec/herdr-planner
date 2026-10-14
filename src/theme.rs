//! The herdr-gpui theme, as CSS variables for the page.
//!
//! herdr-gpui bundles no theme files: its built-ins (Default, Nord, Dracula,
//! Catppuccin Mocha/Latte) are small palettes in its source, and this module
//! carries the same values. The selected theme is read from the app's config
//! (`config-gpui.local.toml` over `config-gpui.toml` in `~/.config/herdr/`):
//! `theme` is a built-in name, `Follow Herdr` (the daemon's `[theme]` in
//! `config.toml`, with herdr's own UI palettes), a Ghostty theme name found
//! in the theme directories, a `~/` or absolute path to a Ghostty theme file,
//! or `light:NAME,dark:NAME`, which follows the system appearance (the page
//! then picks a side by `prefers-color-scheme`). Fonts come from `[ui]` and
//! `[terminal]` of the same files.

use crate::config::{config_home, home};
use serde::Serialize;
use std::env;
use std::path::{Path, PathBuf};

pub const FOLLOW_HERDR: &str = "Follow Herdr";

/// One resolved palette, packed 24-bit RGB.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    pub background: u32,
    pub foreground: u32,
    pub cursor: u32,
    pub surface: u32,
    pub active: u32,
    pub muted: u32,
    pub sidebar: Option<u32>,
    pub palette: [u32; 16],
}

/// `percent` of `over` blended onto `base`, per channel.
pub fn mix(base: u32, over: u32, percent: u32) -> u32 {
    let channel = |shift: u32| {
        let b = (base >> shift) & 255;
        let o = (over >> shift) & 255;
        (b * (100 - percent) + o * percent) / 100
    };
    (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

pub fn luminance(color: u32) -> f32 {
    let ch = |shift: u32| ((color >> shift) & 255) as f32 / 255.;
    0.2126 * ch(16) + 0.7152 * ch(8) + 0.0722 * ch(0)
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            background: 0x101419,
            foreground: 0xd8dee9,
            cursor: 0xd8dee9,
            surface: 0x1c1c22,
            active: 0x2b2933,
            muted: 0x827e91,
            sidebar: None,
            palette: [
                0x000000, 0x800000, 0x008000, 0x808000, 0x000080, 0x800080, 0x008080, 0xc0c0c0,
                0x808080, 0xff0000, 0x00ff00, 0xffff00, 0x0000ff, 0xff00ff, 0x00ffff, 0xffffff,
            ],
        }
    }
}

impl Theme {
    /// The built-ins, as herdr-gpui lists them.
    #[allow(dead_code)]
    pub const BUILTIN_NAMES: [&'static str; 5] = [
        "Default",
        "Nord",
        "Dracula",
        "Catppuccin Mocha",
        "Catppuccin Latte",
    ];

    fn derive_chrome(&mut self) {
        let blend = |p| mix(self.background, self.foreground, p);
        self.surface = blend(5);
        self.active = blend(12);
        self.muted = blend(55);
    }

    pub fn builtin(name: &str) -> Option<Theme> {
        let (background, foreground, ansi): (u32, u32, [u32; 16]) = match name {
            "Default" => return Some(Theme::default()),
            "Nord" => (
                0x2e3440,
                0xd8dee9,
                [
                    0x3b4252, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x88c0d0, 0xe5e9f0,
                    0x4c566a, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x8fbcbb, 0xeceff4,
                ],
            ),
            "Dracula" => (
                0x282a36,
                0xf8f8f2,
                [
                    0x21222c, 0xff5555, 0x50fa7b, 0xf1fa8c, 0xbd93f9, 0xff79c6, 0x8be9fd, 0xf8f8f2,
                    0x6272a4, 0xff6e6e, 0x69ff94, 0xffffa5, 0xd6acff, 0xff92df, 0xa4ffff, 0xffffff,
                ],
            ),
            "Catppuccin Mocha" => (
                0x1e1e2e,
                0xcdd6f4,
                [
                    0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de,
                    0x585b70, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xa6adc8,
                ],
            ),
            "Catppuccin Latte" => (
                0xeff1f5,
                0x4c4f69,
                [
                    0x5c5f77, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0xacb0be,
                    0x6c6f85, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0xbcc0cc,
                ],
            ),
            _ => return None,
        };
        let mut t = Theme {
            background,
            foreground,
            cursor: foreground,
            palette: ansi,
            ..Theme::default()
        };
        t.derive_chrome();
        Some(t)
    }

    /// A Ghostty theme file: `background`, `foreground`, `cursor-color`,
    /// `palette = N=#rrggbb`. Other keys are never interpreted.
    pub fn parse_ghostty(text: &str) -> Result<Theme, String> {
        let mut t = Theme::default();
        let mut cursor_set = false;
        for (i, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, value) = line.split_once('=').unwrap_or((line, ""));
            let (key, value) = (key.trim(), value.trim());
            let color = |v: &str| -> Result<u32, String> {
                let hex = v.strip_prefix('#').unwrap_or(v);
                if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err(format!("строка {}: не цвет «{}»", i + 1, v));
                }
                u32::from_str_radix(hex, 16).map_err(|e| e.to_string())
            };
            match key {
                "background" => t.background = color(value)?,
                "foreground" => t.foreground = color(value)?,
                "cursor-color" => {
                    t.cursor = color(value)?;
                    cursor_set = true;
                }
                "palette" => {
                    let (idx, v) = value
                        .split_once('=')
                        .ok_or(format!("строка {}: palette без цвета", i + 1))?;
                    let idx: usize = idx
                        .trim()
                        .parse()
                        .map_err(|_| format!("строка {}: индекс palette", i + 1))?;
                    if idx < 16 {
                        t.palette[idx] = color(v.trim())?;
                    }
                }
                _ => {}
            }
        }
        if !cursor_set {
            t.cursor = t.foreground;
        }
        t.derive_chrome();
        Ok(t)
    }

    pub fn with_contrast(mut self, high: bool) -> Theme {
        if high {
            self.active = mix(self.active, self.foreground, 12);
        }
        self
    }

    pub fn is_light(&self) -> bool {
        luminance(self.background) > 0.5
    }
}

// ---------------------------------------------------------------------------
// «Follow Herdr»: the daemon's own UI palettes (ported from herdr-gpui's
// herdr_settings/palette.rs, which credits upstream herdr's src/app/state.rs).

const HERDR_THEME_NAMES: [&str; 18] = [
    "catppuccin",
    "catppuccin-latte",
    "terminal",
    "tokyo-night",
    "tokyo-night-day",
    "dracula",
    "nord",
    "gruvbox",
    "gruvbox-light",
    "one-dark",
    "one-light",
    "solarized",
    "solarized-light",
    "kanagawa",
    "kanagawa-lotus",
    "rose-pine",
    "rose-pine-dawn",
    "vesper",
];

const RESET: u32 = u32::MAX;
// accent, panel, active, selection, surface0, surface1, dim, overlay0,
// overlay1, text, subtext, mauve, green, yellow, red, blue, teal, peach.
const HERDR_PALETTES: [[u32; 18]; 18] = [
    [
        0x89b4fa, 0x181825, 0x1e1e2e, 0x313244, 0x313244, 0x45475a, 0x1e1e2e, 0x6c7086, 0x7f849c,
        0xcdd6f4, 0xa6adc8, 0xcba6f7, 0xa6e3a1, 0xf9e2af, 0xf38ba8, 0x89b4fa, 0x94e2d5, 0xfab387,
    ],
    [
        0x1e66f5, 0xeff1f5, 0xe6e9ef, 0xbdd0f5, 0xccd0da, 0xbcc0cc, 0xe6e9ef, 0x9ca0b0, 0x8c8fa1,
        0x4c4f69, 0x6c6f85, 0x8839ef, 0x40a02b, 0xdf8e1d, 0xd20f39, 0x1e66f5, 0x179299, 0xfe640b,
    ],
    [
        0x000080, RESET, 0x808080, RESET, RESET, 0x808080, 0x808080, 0xc0c0c0, 0xffffff, RESET,
        0xc0c0c0, 0xc0c0c0, 0x008000, 0x808000, 0xff0000, 0x000080, 0x008080, 0x808000,
    ],
    [
        0x7aa2f7, 0x1a1b26, 0x232636, 0x2d3650, 0x24283b, 0x414868, 0x1a1b26, 0x565f89, 0x697196,
        0xc0caf5, 0xa9b1d6, 0xbb9af7, 0x9ece6a, 0xe0af68, 0xf7768e, 0x7aa2f7, 0x7dcfff, 0xff9e64,
    ],
    [
        0x2e7de9, 0xe1e2e7, 0xd2d3da, 0xb6cae7, 0xc4c8da, 0xa8aecb, 0xd2d3da, 0x8990b3, 0x68709a,
        0x3760bf, 0x6172b0, 0x7847bd, 0x587539, 0x8c6c3e, 0xf52a65, 0x2e7de9, 0x118c74, 0xb15c00,
    ],
    [
        0xbd93f9, 0x282a36, 0x373c52, 0x463f5d, 0x44475a, 0x6272a4, 0x282a36, 0x6272a4, 0x828cb4,
        0xf8f8f2, 0xd2d2dc, 0xff79c6, 0x50fa7b, 0xf1fa8c, 0xff5555, 0x8be9fd, 0x8be9fd, 0xffb86c,
    ],
    [
        0x88c0d0, 0x2e3440, 0x434c5e, 0x40505d, 0x3b4252, 0x434c5e, 0x2e3440, 0x4c566a, 0x646e82,
        0xeceff4, 0xd8dee9, 0xb48ead, 0xa3be8c, 0xebcb8b, 0xbf616a, 0x81a1c1, 0x8fbcbb, 0xd08770,
    ],
    [
        0xd79921, 0x282828, 0x323130, 0x4b3f27, 0x3c3836, 0x504945, 0x282828, 0x928374, 0xa89984,
        0xebdbb2, 0xd5c4a1, 0xd3869b, 0xb8bb26, 0xfabd2f, 0xfb4934, 0x83a598, 0x8ec07c, 0xfe8019,
    ],
    [
        0x076678, 0xfbf1c7, 0xf2e5bc, 0xebdbb2, 0xebdbb2, 0xd5c4a1, 0xf2e5bc, 0x928374, 0x7c6f64,
        0x3c3836, 0x504945, 0x8f3f71, 0x79740e, 0xb57614, 0x9d0006, 0x076678, 0x427b58, 0xaf3a03,
    ],
    [
        0x61afef, 0x282c34, 0x313640, 0x334659, 0x2c313a, 0x3e4451, 0x282c34, 0x5c6370, 0x737a87,
        0xabb2bf, 0x969ca8, 0xc678dd, 0x98c379, 0xe5c07b, 0xe06c75, 0x61afef, 0x56b6c2, 0xd19a66,
    ],
    [
        0x4078f2, 0xfafafa, 0xd8dbe2, 0xcddbf8, 0xf0f0f1, 0xe5e5e6, 0xf5f5f6, 0xa0a1a7, 0x686b77,
        0x383a42, 0x686b77, 0xa626a4, 0x50a14f, 0xc18401, 0xe45649, 0x4078f2, 0x0184bc, 0x986801,
    ],
    [
        0x268bd2, 0x002b36, 0x164b57, 0x083e55, 0x073642, 0x586e75, 0x002b36, 0x586e75, 0x657b83,
        0x93a1a1, 0x839496, 0xd33682, 0x859900, 0xb58900, 0xdc322f, 0x268bd2, 0x2aa198, 0xcb4b16,
    ],
    [
        0x268bd2, 0xfdf6e3, 0xeee8d5, 0xc9dcdf, 0xeee8d5, 0x93a1a1, 0xeee8d5, 0x93a1a1, 0x586e75,
        0x657b83, 0x839496, 0xd33682, 0x859900, 0xb58900, 0xdc322f, 0x268bd2, 0x2aa198, 0xcb4b16,
    ],
    [
        0x7e9cd8, 0x1f1f28, 0x363646, 0x32384b, 0x2a2a37, 0x363646, 0x1f1f28, 0x727169, 0x87867d,
        0xdcd7ba, 0xc8c3aa, 0x957fb8, 0x76946a, 0xc0a36e, 0xc34043, 0x7e9cd8, 0x7fb4ca, 0xffa066,
    ],
    [
        0x4d699b, 0xf2ecbc, 0xd5cea3, 0xdcd5ac, 0xdcd5ac, 0xc9cbd1, 0xd5cea3, 0xa09cac, 0x8a8980,
        0x545464, 0x43436c, 0x624c83, 0x6f894e, 0x77713f, 0xc84053, 0x4d699b, 0x4e8ca2, 0xcc6d00,
    ],
    [
        0xc4a7e7, 0x191724, 0x26233a, 0x3b344b, 0x1f1d2e, 0x26233a, 0x26233a, 0x6e6a86, 0x908caa,
        0xe0def4, 0xc8c5dc, 0xc4a7e7, 0x31748f, 0xf6c177, 0xeb6f92, 0x31748f, 0x9ccfd8, 0xea9a97,
    ],
    [
        0x907aa9, 0xfaf4ed, 0xe3d9cf, 0xf2e9e1, 0xf2e9e1, 0xfffaf3, 0xf2e9e1, 0x9893a5, 0x797593,
        0x464261, 0x797593, 0x907aa9, 0x286983, 0xea9d34, 0xb4637a, 0x286983, 0x56949f, 0xd7827e,
    ],
    [
        0xffc799, 0x1a1a1a, 0x101010, 0x232323, 0x232323, 0x282828, 0x101010, 0x5c5c5c, 0x7e7e7e,
        0xffffff, 0xa0a0a0, 0xffd1a8, 0x99ffe4, 0xffc799, 0xff8080, 0xb0b0b0, 0x66ddcc, 0xffc799,
    ],
];

fn herdr_canonical(name: &str) -> Option<&'static str> {
    let n = name.to_lowercase().replace([' ', '_'], "-");
    Some(match n.as_str() {
        "catppuccin-mocha" => "catppuccin",
        "latte" | "light" => "catppuccin-latte",
        "tokyonight" => "tokyo-night",
        "tokyo-day" | "tokyonight-day" => "tokyo-night-day",
        "gruvbox-dark" => "gruvbox",
        "onedark" => "one-dark",
        "onelight" => "one-light",
        "solarized-dark" => "solarized",
        "lotus" => "kanagawa-lotus",
        "rosepine" => "rose-pine",
        "rosepine-dawn" | "dawn" => "rose-pine-dawn",
        _ => return HERDR_THEME_NAMES.iter().copied().find(|c| *c == n),
    })
}

/// `#rrggbb`, `#rgb` or `rgb(r,g,b)`; anything else keeps the palette's color.
fn parse_color(input: &str) -> Option<u32> {
    let v = input.trim().to_lowercase();
    if let Some(hex) = v.strip_prefix('#') {
        return match hex.len() {
            6 => u32::from_str_radix(hex, 16).ok(),
            3 => u32::from_str_radix(hex, 16)
                .ok()
                .map(|rgb| ((rgb & 0xf00) << 8 | (rgb & 0xf0) << 4 | rgb & 0xf) * 17),
            _ => None,
        };
    }
    let inner = v.strip_prefix("rgb(")?.strip_suffix(')')?;
    let parts: Vec<u32> = inner
        .split(',')
        .filter_map(|p| p.trim().parse::<u8>().ok().map(u32::from))
        .collect();
    (parts.len() == 3).then(|| parts[0] << 16 | parts[1] << 8 | parts[2])
}

const OVERRIDE_KEYS: [&str; 19] = [
    "accent",
    "panel_bg",
    "active_row_bg",
    "selection_bg",
    "surface0",
    "surface1",
    "surface_dim",
    "overlay0",
    "overlay1",
    "text",
    "subtext0",
    "mauve",
    "green",
    "yellow",
    "red",
    "blue",
    "teal",
    "peach",
    "sidebar_bg",
];

fn apply_overrides(colors: &mut [u32; 19], table: Option<&toml::Value>) {
    let Some(t) = table.and_then(|t| t.as_table()) else {
        return;
    };
    for (i, key) in OVERRIDE_KEYS.iter().enumerate() {
        if let Some(c) = t.get(*key).and_then(|v| v.as_str()).and_then(parse_color) {
            colors[i] = c;
        }
    }
}

/// Herdr's theme for the appearance `light`, from the daemon's `config.toml` text.
pub fn herdr_theme(config_text: &str, light: bool) -> Theme {
    let doc: toml::Value =
        toml::from_str(config_text).unwrap_or(toml::Value::Table(Default::default()));
    let theme = doc.get("theme");
    let get = |k: &str| {
        theme
            .and_then(|t| t.get(k))
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    let manual = get("name").unwrap_or_else(|| "catppuccin".into());
    let auto_switch = theme
        .and_then(|t| t.get("auto_switch"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let sibling = match herdr_canonical(&manual) {
        Some("catppuccin" | "catppuccin-latte") => Some(("catppuccin", "catppuccin-latte")),
        Some("tokyo-night" | "tokyo-night-day") => Some(("tokyo-night", "tokyo-night-day")),
        Some("gruvbox" | "gruvbox-light") => Some(("gruvbox", "gruvbox-light")),
        Some("one-dark" | "one-light") => Some(("one-dark", "one-light")),
        Some("solarized" | "solarized-light") => Some(("solarized", "solarized-light")),
        Some("kanagawa" | "kanagawa-lotus") => Some(("kanagawa", "kanagawa-lotus")),
        Some("rose-pine" | "rose-pine-dawn") => Some(("rose-pine", "rose-pine-dawn")),
        _ => None,
    };
    let name = if auto_switch {
        if light {
            get("light_name").unwrap_or_else(|| sibling.map_or(manual.clone(), |p| p.1.into()))
        } else {
            get("dark_name").unwrap_or_else(|| sibling.map_or(manual.clone(), |p| p.0.into()))
        }
    } else {
        manual.clone()
    };
    let fallback = usize::from(auto_switch && light);
    let index = herdr_canonical(&name)
        .and_then(|n| HERDR_THEME_NAMES.iter().position(|c| *c == n))
        .unwrap_or(fallback);
    let mut colors = [RESET; 19];
    colors[..18].copy_from_slice(&HERDR_PALETTES[index]);
    let custom = theme.and_then(|t| t.get("custom"));
    apply_overrides(&mut colors, custom);
    if auto_switch {
        apply_overrides(
            &mut colors,
            custom.and_then(|c| c.get(if light { "light" } else { "dark" })),
        );
    }
    let rgb = |i: usize, fb: u32| if colors[i] == RESET { fb } else { colors[i] };
    let mut t = Theme::default();
    t.background = rgb(6, t.background);
    t.sidebar = (colors[18] != RESET).then_some(colors[18]);
    t.surface = rgb(1, t.background);
    t.active = rgb(2, t.background);
    t.muted = rgb(7, t.muted);
    t.foreground = rgb(9, t.foreground);
    t.cursor = t.foreground;
    for (slot, token) in [6, 14, 12, 13, 15, 0, 16, 9, 7, 14, 12, 13, 15, 11, 16, 9]
        .into_iter()
        .enumerate()
    {
        t.palette[slot] = rgb(token, t.palette[slot]);
    }
    t
}

// ---------------------------------------------------------------------------
// The herdr-gpui config: theme name, contrast, fonts.

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Font {
    pub family: Option<String>,
    pub size: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GpuiConfig {
    pub theme: String,
    pub contrast_high: bool,
    pub ui: Font,
    pub terminal: Font,
}

fn font_of(doc: &toml::Value, section: &str, default_size: f64) -> Font {
    let t = doc.get(section);
    Font {
        family: t
            .and_then(|t| t.get("family"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .filter(|s| !s.is_empty()),
        size: t
            .and_then(|t| t.get("size"))
            .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
            .unwrap_or(default_size),
    }
}

/// Tables merge key by key: a local key wins, unset keys inherit the defaults.
fn merge(base: &mut toml::Value, over: toml::Value) {
    match (base, over) {
        (toml::Value::Table(b), toml::Value::Table(o)) => {
            for (k, v) in o {
                match b.get_mut(&k) {
                    Some(slot) if slot.is_table() && v.is_table() => merge(slot, v),
                    _ => {
                        b.insert(k, v);
                    }
                }
            }
        }
        (b, o) => *b = o,
    }
}

impl GpuiConfig {
    pub fn parse(defaults: &str, local: &str) -> GpuiConfig {
        let mut doc: toml::Value =
            toml::from_str(defaults).unwrap_or(toml::Value::Table(Default::default()));
        if let Ok(over) = toml::from_str::<toml::Value>(local) {
            merge(&mut doc, over);
        }
        GpuiConfig {
            theme: doc
                .get("theme")
                .and_then(|v| v.as_str())
                .unwrap_or("Default")
                .trim()
                .to_string(),
            contrast_high: doc.get("contrast").and_then(|v| v.as_str()) == Some("high"),
            ui: font_of(&doc, "ui", 12.),
            terminal: font_of(&doc, "terminal", 14.),
        }
    }
}

/// A `theme` value: one theme, or `light:NAME,dark:NAME`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeName {
    Single(String),
    System { light: String, dark: String },
}

impl ThemeName {
    pub fn parse(value: &str) -> ThemeName {
        let value = value.trim();
        let side = |part: &str| {
            let part = part.trim();
            part.strip_prefix("light:")
                .map(|n| (true, n.trim().to_string()))
                .or_else(|| {
                    part.strip_prefix("dark:")
                        .map(|n| (false, n.trim().to_string()))
                })
        };
        if side(value).is_none() {
            return ThemeName::Single(value.to_string());
        }
        let mut parts = value.split(',');
        match (
            parts.next().and_then(side),
            parts.next().and_then(side),
            parts.next(),
        ) {
            (Some((true, light)), Some((false, dark)), None)
            | (Some((false, dark)), Some((true, light)), None)
                if !light.is_empty() && !dark.is_empty() =>
            {
                ThemeName::System { light, dark }
            }
            _ => ThemeName::Single(value.to_string()),
        }
    }
}

/// Where Ghostty theme files are looked for, in order.
pub fn theme_directories() -> Vec<PathBuf> {
    let root = config_home();
    let mut dirs = vec![root.join("herdr/themes"), root.join("ghostty/themes")];
    if let Some(r) = env::var_os("GHOSTTY_RESOURCES_DIR").filter(|v| !v.is_empty()) {
        dirs.push(PathBuf::from(r).join("themes"));
    }
    match env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        Some(d) => dirs.push(PathBuf::from(d).join("ghostty/themes")),
        None => dirs.push(home().join(".local/share/ghostty/themes")),
    }
    let data_dirs = env::var_os("XDG_DATA_DIRS")
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    dirs.extend(env::split_paths(&data_dirs).map(|d| d.join("ghostty/themes")));
    dirs
}

/// The files the theme depends on; a change of any of them re-resolves it.
pub fn watched_files() -> Vec<PathBuf> {
    let root = config_home().join("herdr");
    vec![
        root.join("config-gpui.local.toml"),
        root.join("config-gpui.toml"),
        root.join("config.toml"),
    ]
}

/// Resolves one side's name to a palette: built-in, Follow Herdr, a file.
pub fn resolve_side(
    name: &str,
    light: bool,
    herdr_config: &str,
    dirs: &[PathBuf],
) -> Result<Theme, String> {
    if name == FOLLOW_HERDR {
        return Ok(herdr_theme(herdr_config, light));
    }
    if let Some(t) = Theme::builtin(name) {
        return Ok(t);
    }
    let path = if let Some(rel) = name.strip_prefix("~/") {
        home().join(rel)
    } else if Path::new(name).is_absolute() {
        PathBuf::from(name)
    } else {
        if name.is_empty() || name.contains('/') {
            return Err(format!("тема «{name}» не найдена"));
        }
        dirs.iter()
            .map(|d| d.join(name))
            .find(|p| p.is_file())
            .ok_or_else(|| format!("тема «{name}» не найдена"))?
    };
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {}", path.display(), e))?;
    Theme::parse_ghostty(&text)
}

/// What the page gets: both sides when the theme follows the system
/// appearance, the single theme as `dark` otherwise (with `light` unset).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ThemeReport {
    pub name: String,
    pub system: bool,
    pub dark: Side,
    pub light: Option<Side>,
    pub ui: Font,
    pub terminal: Font,
    pub contrast_high: bool,
    /// Why the fallback is shown, when it is.
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Side {
    pub is_light: bool,
    pub vars: std::collections::BTreeMap<String, String>,
}

fn hex(c: u32) -> String {
    format!("#{c:06x}")
}

impl Side {
    pub fn of(t: &Theme) -> Side {
        let mut vars = std::collections::BTreeMap::new();
        let mut put = |k: &str, v: u32| {
            vars.insert(k.to_string(), hex(v));
        };
        put("bg", t.background);
        put("fg", t.foreground);
        put("cursor", t.cursor);
        put("surface", t.surface);
        put("active", t.active);
        put("muted", t.muted);
        put("sidebar", t.sidebar.unwrap_or(t.surface));
        put("subtext", mix(t.background, t.foreground, 78));
        put("accent", t.palette[5]);
        put("accent-wash", mix(t.surface, t.palette[5], 22));
        put("border", mix(t.background, t.foreground, 16));
        let names = [
            "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
        ];
        for (i, n) in names.iter().enumerate() {
            put(n, t.palette[i]);
            put(&format!("bright-{n}"), t.palette[i + 8]);
        }
        Side {
            is_light: t.is_light(),
            vars,
        }
    }
}

pub fn report(cfg: &GpuiConfig, herdr_config: &str, dirs: &[PathBuf]) -> ThemeReport {
    let name = ThemeName::parse(&cfg.theme);
    let (dark, light, system, error) = match &name {
        ThemeName::Single(n) => match resolve_side(n, false, herdr_config, dirs) {
            Ok(t) => (t, None, false, None),
            Err(e) => (Theme::default(), None, false, Some(e)),
        },
        ThemeName::System { light, dark } => {
            let d = resolve_side(dark, false, herdr_config, dirs);
            let l = resolve_side(light, true, herdr_config, dirs);
            match (d, l) {
                (Ok(d), Ok(l)) => (d, Some(l), true, None),
                (Ok(d), Err(e)) | (Err(e), Ok(d)) => (d, None, false, Some(e)),
                (Err(e), Err(_)) => (Theme::default(), None, false, Some(e)),
            }
        }
    };
    let dark = dark.with_contrast(cfg.contrast_high);
    let light = light.map(|l| l.with_contrast(cfg.contrast_high));
    ThemeReport {
        name: cfg.theme.clone(),
        system,
        dark: Side::of(&dark),
        light: light.as_ref().map(Side::of),
        ui: cfg.ui.clone(),
        terminal: cfg.terminal.clone(),
        contrast_high: cfg.contrast_high,
        error,
    }
}

/// Reads the config files and resolves the theme. Never fails: a theme that
/// cannot be read is replaced by the Default palette with the error noted.
pub fn load() -> ThemeReport {
    let files = watched_files();
    let read = |p: &Path| std::fs::read_to_string(p).unwrap_or_default();
    let cfg = GpuiConfig::parse(&read(&files[1]), &read(&files[0]));
    report(&cfg, &read(&files[2]), &theme_directories())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_overrides_defaults() {
        let cfg = GpuiConfig::parse(
            "theme = \"Default\"\ncontrast = \"standard\"\n[ui]\nsize = 12\n[terminal]\nsize = 14\n",
            "theme = \"Nord\"\n[ui]\nfamily = \"FiraCode Nerd Font\"\n",
        );
        assert_eq!(cfg.theme, "Nord");
        assert_eq!(cfg.ui.family.as_deref(), Some("FiraCode Nerd Font"));
        assert_eq!(cfg.ui.size, 12.);
        assert_eq!(cfg.terminal.family, None);
        assert_eq!(cfg.terminal.size, 14.);
    }

    #[test]
    fn every_builtin_resolves() {
        for name in Theme::BUILTIN_NAMES {
            assert!(Theme::builtin(name).is_some(), "{name}");
        }
    }

    #[test]
    fn builtin_nord_and_report() {
        let cfg = GpuiConfig::parse("", "theme = \"Nord\"");
        let r = report(&cfg, "", &[]);
        assert_eq!(r.dark.vars["bg"], "#2e3440");
        assert_eq!(r.dark.vars["accent"], "#b48ead");
        assert!(!r.dark.is_light && !r.system && r.light.is_none() && r.error.is_none());
        let latte = Theme::builtin("Catppuccin Latte").unwrap();
        assert!(latte.is_light());
    }

    #[test]
    fn system_pair_and_fallback() {
        let r = report(
            &GpuiConfig::parse("", "theme = \"light:Catppuccin Latte,dark:Dracula\""),
            "",
            &[],
        );
        assert!(r.system);
        assert_eq!(r.dark.vars["bg"], "#282a36");
        assert!(r.light.unwrap().is_light);
        let r = report(&GpuiConfig::parse("", "theme = \"No Such Theme\""), "", &[]);
        assert!(r.error.is_some());
        assert_eq!(r.dark.vars["bg"], "#101419");
    }

    #[test]
    fn follow_herdr_reads_the_daemon_config() {
        let r = report(
            &GpuiConfig::parse("", "theme = \"Follow Herdr\""),
            "[theme]\nname = \"nord\"\n[theme.custom]\npanel_bg = \"#000000\"\n",
            &[],
        );
        assert_eq!(r.dark.vars["bg"], "#2e3440");
        assert_eq!(r.dark.vars["surface"], "#000000");
        assert_eq!(r.dark.vars["accent"], "#88c0d0");
        let auto = herdr_theme("[theme]\nname = \"catppuccin\"\nauto_switch = true\n", true);
        assert!(auto.is_light());
        let bad = herdr_theme("not toml [", false);
        assert_eq!(bad.background, 0x1e1e2e);
    }

    #[test]
    fn ghostty_file_and_name_parse() {
        let t = Theme::parse_ghostty("# c\nbackground = #111111\nforeground = #eeeeee\npalette = 1=#ff0000\nbold-color = bright\n").unwrap();
        assert_eq!(t.background, 0x111111);
        assert_eq!(t.palette[1], 0xff0000);
        assert_eq!(t.cursor, 0xeeeeee);
        assert!(Theme::parse_ghostty("background = zzz").is_err());
        assert_eq!(
            ThemeName::parse("dark:A, light:B"),
            ThemeName::System {
                light: "B".into(),
                dark: "A".into()
            }
        );
        assert_eq!(
            ThemeName::parse("light:A"),
            ThemeName::Single("light:A".into())
        );
        assert_eq!(parse_color("#abc"), Some(0xaabbcc));
        assert_eq!(parse_color("rgb(1, 2, 3)"), Some(0x010203));
    }
}
