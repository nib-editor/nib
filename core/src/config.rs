//! Settings: `config.toml` for the core, and `plugins/<name>.toml` for each
//! plugin.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::{fs, io};

use serde::Deserialize;

use crate::Error;
use crate::grid::{Color, Style};
use crate::input::KeyEvent;
use crate::ui::Theme;

/// Settings of the core.
///
/// Editing behavior (`tab_width`, `indent`, `scroll_margin`) is for plugins
/// to read, and the first two to override per buffer. Safety settings
/// (`menu_key`, and the plugin limits) are for the user alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub tab_width: u16,
    pub indent: Indent,
    /// Lines kept visible above and below the cursor.
    pub scroll_margin: u16,
    /// The base plugin to use (docs/base.md).
    pub base: String,
    /// Set by the user; otherwise the base's, or Ctrl-g.
    pub menu_key: Option<KeyEvent>,
    /// A plugin call taking longer is stopped, unless the plugin's own
    /// settings say otherwise.
    pub plugin_timeout: Duration,
    pub plugin_init_timeout: Duration,
    /// Maximum size of a plugin's memory, in bytes.
    pub plugin_memory: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Indent {
    Tab,
    Spaces(u8),
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            tab_width: 4,
            indent: Indent::Spaces(4),
            scroll_margin: 5,
            base: "helix".into(),
            menu_key: None,
            plugin_timeout: Duration::from_secs(1),
            plugin_init_timeout: Duration::from_secs(5),
            plugin_memory: 256 << 20,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config {
    pub core: Settings,
    pub theme: Theme,
    /// From `plugins/<name>.toml`, by plugin name.
    pub plugins: BTreeMap<String, PluginConfig>,
}

/// `plugins/<name>.toml`: how nib runs one plugin, and the plugin's own
/// settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginConfig {
    /// Where to load the plugin from; `None` for a built-in one.
    pub path: Option<PathBuf>,
    pub enabled: bool,
    /// Limits for this plugin, instead of the ones in `[core]`.
    pub timeout: Option<Timeout>,
    pub init_timeout: Option<Timeout>,
    pub memory: Option<usize>,
    pub load: Load,
    /// `[settings]` as JSON, passed to the plugin's `init` as is.
    pub settings: String,
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            path: None,
            enabled: true,
            timeout: None,
            init_timeout: None,
            memory: None,
            load: Load::Start,
            settings: "{}".into(),
        }
    }
}

/// How long a plugin call may take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Timeout {
    After(Duration),
    /// Only Ctrl-g stops it.
    Never,
}

impl Timeout {
    pub fn limit(self) -> Option<Duration> {
        match self {
            Timeout::After(duration) => Some(duration),
            Timeout::Never => None,
        }
    }
}

/// When a plugin starts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Load {
    /// When nib starts.
    #[default]
    Start,
    /// When one of its commands is called or one of its events arrives.
    Lazy,
}

/// A number of milliseconds, or "none".
#[derive(Deserialize)]
#[serde(untagged)]
enum RawTimeout {
    Millis(u64),
    Word(String),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawPluginConfig {
    path: Option<PathBuf>,
    #[serde(default = "enabled")]
    enabled: bool,
    timeout_ms: Option<RawTimeout>,
    init_timeout_ms: Option<RawTimeout>,
    memory_mib: Option<usize>,
    #[serde(default)]
    load: Load,
    #[serde(default)]
    settings: toml::Table,
}

fn enabled() -> bool {
    true
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    core: RawCore,
    #[serde(default)]
    theme: BTreeMap<String, RawStyle>,
    /// Moved to `plugins/<name>.toml`; read only to say so.
    plugins: Option<toml::Value>,
}

/// A color alone, or a table with colors and attributes.
#[derive(Deserialize)]
#[serde(untagged)]
enum RawStyle {
    Color(RawColor),
    Table(RawStyleTable),
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStyleTable {
    fg: Option<RawColor>,
    bg: Option<RawColor>,
    #[serde(default)]
    bold: bool,
    #[serde(default)]
    italic: bool,
    #[serde(default)]
    underline: bool,
    #[serde(default)]
    reverse: bool,
}

/// A name such as "blue" or "bright-black", "#rrggbb", or 0 to 255.
#[derive(Deserialize)]
#[serde(untagged)]
enum RawColor {
    Index(u8),
    Name(String),
}

const COLOR_NAMES: [&str; 8] = [
    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
];

fn parse_color(color: &RawColor) -> Result<Color, String> {
    let name = match color {
        RawColor::Index(n) => return Ok(Color::Indexed(*n)),
        RawColor::Name(name) => name.as_str(),
    };
    if name == "default" {
        return Ok(Color::Reset);
    }
    if let Some(hex) = name.strip_prefix('#')
        && hex.len() == 6
        && let Ok(rgb) = u32::from_str_radix(hex, 16)
    {
        return Ok(Color::Rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8));
    }
    let (bright, base) = match name.strip_prefix("bright-") {
        Some(base) => (8, base),
        None => (0, name),
    };
    COLOR_NAMES
        .iter()
        .position(|c| *c == base)
        .map(|i| Color::Indexed(i as u8 + bright))
        .ok_or_else(|| format!("unknown color {name:?}"))
}

fn parse_style(style: &RawStyle) -> Result<Style, String> {
    let table = match style {
        RawStyle::Color(color) => {
            return Ok(Style {
                fg: parse_color(color)?,
                ..Style::default()
            });
        }
        RawStyle::Table(table) => table,
    };
    let color = |c: &Option<RawColor>| c.as_ref().map_or(Ok(Color::Reset), parse_color);
    Ok(Style {
        fg: color(&table.fg)?,
        bg: color(&table.bg)?,
        bold: table.bold,
        italic: table.italic,
        underline: table.underline,
        reverse: table.reverse,
    })
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawCore {
    tab_width: Option<u16>,
    indent: Option<RawIndent>,
    scroll_margin: Option<u16>,
    base: Option<String>,
    menu_key: Option<String>,
    /// Moved to `path` in `plugins/<name>.toml`; read only to say so.
    plugin_dirs: Option<toml::Value>,
    plugin_timeout_ms: Option<u64>,
    plugin_init_timeout_ms: Option<u64>,
    plugin_memory_mib: Option<usize>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawIndent {
    Spaces(u8),
    Name(String),
}

impl Config {
    pub fn parse(text: &str) -> Result<Self, Error> {
        let fail = |message: String| Error::Config(message);
        let raw: RawConfig = toml::from_str(text).map_err(|err| fail(err.to_string()))?;
        let mut core = Settings::default();
        let raw_core = raw.core;

        if let Some(width) = raw_core.tab_width {
            if !(1..=16).contains(&width) {
                return Err(fail(format!("tab-width must be 1 to 16, not {width}")));
            }
            core.tab_width = width;
        }
        if let Some(indent) = raw_core.indent {
            core.indent = match indent {
                RawIndent::Spaces(n @ 1..=16) => Indent::Spaces(n),
                RawIndent::Name(name) if name == "tab" => Indent::Tab,
                RawIndent::Spaces(n) => {
                    return Err(fail(format!("indent must be 1 to 16 or \"tab\", not {n}")));
                }
                RawIndent::Name(name) => {
                    return Err(fail(format!(
                        "indent must be 1 to 16 or \"tab\", not {name:?}"
                    )));
                }
            };
        }
        if let Some(margin) = raw_core.scroll_margin {
            core.scroll_margin = margin;
        }
        if let Some(base) = raw_core.base {
            core.base = base;
        }
        if let Some(key) = raw_core.menu_key {
            core.menu_key = Some(
                key.parse()
                    .map_err(|err| fail(format!("menu-key: {err}")))?,
            );
        }
        if raw_core.plugin_dirs.is_some() {
            return Err(fail(
                "plugin-dirs moved: write `path = \"...\"` in plugins/<name>.toml".into(),
            ));
        }
        if raw.plugins.is_some() {
            return Err(fail(
                "[plugins.<name>] moved: write [settings] in plugins/<name>.toml".into(),
            ));
        }
        if let Some(ms) = raw_core.plugin_timeout_ms {
            core.plugin_timeout = timeout("plugin-timeout-ms", ms).map_err(fail)?;
        }
        if let Some(ms) = raw_core.plugin_init_timeout_ms {
            core.plugin_init_timeout = timeout("plugin-init-timeout-ms", ms).map_err(fail)?;
        }
        if let Some(mib) = raw_core.plugin_memory_mib {
            core.plugin_memory = memory("plugin-memory-mib", mib).map_err(fail)?;
        }

        let theme = raw
            .theme
            .iter()
            .map(|(name, style)| {
                let style =
                    parse_style(style).map_err(|err| fail(format!("theme.{name}: {err}")))?;
                Ok((name.clone(), style))
            })
            .collect::<Result<_, Error>>()?;
        Ok(Self {
            core,
            theme: Theme::new(theme),
            plugins: BTreeMap::new(),
        })
    }

    /// Parses `plugins/<name>.toml`.
    pub fn parse_plugin(name: &str, text: &str) -> Result<PluginConfig, Error> {
        let fail = |message: String| Error::Config(format!("plugins/{name}.toml: {message}"));
        let raw: RawPluginConfig = toml::from_str(text).map_err(|err| fail(err.to_string()))?;
        let limit = |raw: Option<RawTimeout>, key| {
            raw.map(|raw| match raw {
                RawTimeout::Millis(ms) => timeout(key, ms).map(Timeout::After),
                RawTimeout::Word(word) if word == "none" => Ok(Timeout::Never),
                RawTimeout::Word(word) => Err(format!(
                    "{key} must be milliseconds or \"none\", not {word:?}"
                )),
            })
            .transpose()
        };
        Ok(PluginConfig {
            path: raw.path,
            enabled: raw.enabled,
            timeout: limit(raw.timeout_ms, "timeout-ms").map_err(fail)?,
            init_timeout: limit(raw.init_timeout_ms, "init-timeout-ms").map_err(fail)?,
            load: raw.load,
            memory: raw
                .memory_mib
                .map(|mib| memory("memory-mib", mib))
                .transpose()
                .map_err(fail)?,
            settings: serde_json::to_string(&raw.settings).map_err(|err| fail(err.to_string()))?,
        })
    }
}

fn timeout(key: &str, ms: u64) -> Result<Duration, String> {
    if ms < 10 {
        return Err(format!("{key} must be at least 10, not {ms}"));
    }
    Ok(Duration::from_millis(ms))
}

fn memory(key: &str, mib: usize) -> Result<usize, String> {
    if !(16..=4096).contains(&mib) {
        return Err(format!("{key} must be 16 to 4096, not {mib}"));
    }
    Ok(mib << 20)
}

impl Config {
    /// The plugin's settings, or the defaults when it has no file.
    pub fn plugin(&self, name: &str) -> PluginConfig {
        self.plugins.get(name).cloned().unwrap_or_default()
    }
}

impl Config {
    /// Reads `config.toml` and `plugins/*.toml` in `dir`. Missing files mean
    /// the defaults.
    pub fn load(dir: &Path) -> Result<Config, String> {
        let mut config = match fs::read_to_string(dir.join("config.toml")) {
            Ok(text) => Self::parse(&text).map_err(|err| err.to_string())?,
            Err(err) if err.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(err) => return Err(format!("config.toml: {err}")),
        };
        for (name, path) in Self::plugin_files(dir)? {
            let text =
                fs::read_to_string(&path).map_err(|err| format!("{}: {err}", path.display()))?;
            let plugin = Self::parse_plugin(&name, &text).map_err(|err| err.to_string())?;
            config.plugins.insert(name, plugin);
        }
        Ok(config)
    }

    /// The `plugins/<name>.toml` files, sorted by name.
    pub fn plugin_files(dir: &Path) -> Result<Vec<(String, PathBuf)>, String> {
        let dir = dir.join("plugins");
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(format!("{}: {err}", dir.display())),
        };
        let mut files: Vec<_> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|e| e == "toml"))
            .filter_map(|path| Some((path.file_stem()?.to_str()?.to_string(), path)))
            .collect();
        files.sort();
        Ok(files)
    }
}

/// config.toml with every key commented out, for `nib config init` and
/// for opening a missing one.
pub const CONFIG_TEMPLATE: &str = r##"# nib's settings. Every line is optional; the values shown are the defaults.
# Each plugin has its own file in plugins/<name>.toml.

[core]
# tab-width = 4
# Spaces per indent, or "tab".
# indent = 4
# Lines kept visible above and below the cursor.
# scroll-margin = 5
# The way of editing: helix, or another base plugin.
# base = "helix"
# Opens the core menu to manage plugins; plugins never see this key. The
# base has its own (C-g for helix), and this replaces it.
# menu-key = "C-g"
# Limits for every plugin; plugins/<name>.toml can set its own.
# plugin-timeout-ms = 1000
# plugin-init-timeout-ms = 5000
# plugin-memory-mib = 256

# Styles by name: UI parts such as "ui.selection", and syntax such as
# "keyword" or "function.method", which falls back to "function". A color
# alone sets the text color; a table can set fg, bg, bold, italic, underline,
# and reverse. Colors: black, red, green, yellow, blue, magenta, cyan, white,
# bright-<color>, "#rrggbb", 0 to 255, or "default".
[theme]
# keyword = "magenta"
# comment = { fg = "bright-black", italic = true }
"##;

/// A `plugins/<name>.toml` with every key commented out.
pub fn plugin_template(name: &str) -> String {
    format!(
        r##"# How nib runs the {name} plugin, and the settings it gets. Every line is
# optional.

# false to not load it.
# enabled = true
# A directory to load it from, instead of the built-in {name}.
# path = "~/dev/{name}"
# Limits instead of the ones in config.toml's [core]. "none" for no time
# limit; Ctrl-g still stops it.
# timeout-ms = 1000
# init-timeout-ms = 5000
# memory-mib = 256
# "lazy" to start it when one of its commands is called or one of its
# events comes, rather than when nib starts.
# load = "start"

# Given to the plugin when it starts.
[settings]
"##
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn templates_parse_to_the_defaults() {
        assert_eq!(Config::parse(CONFIG_TEMPLATE).unwrap(), Config::default());
        let plugin = Config::parse_plugin("helix", &plugin_template("helix")).unwrap();
        assert_eq!(plugin, PluginConfig::default());
    }

    use super::*;

    #[test]
    fn empty_config_is_default() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
    }

    #[test]
    fn parses_core_and_plugin_tables() {
        let config = Config::parse(
            r#"
            [core]
            tab-width = 8
            indent = "tab"
            base = "vim"
            menu-key = "C-]"
            plugin-timeout-ms = 2000
            plugin-memory-mib = 512
            "#,
        )
        .unwrap();
        assert_eq!(config.core.tab_width, 8);
        assert_eq!(config.core.indent, Indent::Tab);
        assert_eq!(config.core.menu_key, Some(KeyEvent::ctrl(']')));
        assert_eq!(config.core.base, "vim");
        assert_eq!(config.core.plugin_timeout, Duration::from_secs(2));
        assert_eq!(config.core.plugin_init_timeout, Duration::from_secs(5));
        assert_eq!(config.core.plugin_memory, 512 << 20);
    }

    #[test]
    fn parses_plugin_files() {
        let plugin = Config::parse_plugin(
            "lsp",
            r#"
            path = "~/dev/nib-lsp"
            timeout-ms = 2000
            memory-mib = 512

            [settings]
            server = "rust-analyzer"
            keys.normal = { "C-s" = "buffer.save" }
            "#,
        )
        .unwrap();
        assert_eq!(plugin.path, Some(PathBuf::from("~/dev/nib-lsp")));
        assert!(plugin.enabled);
        assert_eq!(plugin.timeout, Some(Timeout::After(Duration::from_secs(2))));
        assert_eq!(plugin.init_timeout, None);
        assert_eq!(plugin.memory, Some(512 << 20));
        assert_eq!(
            plugin.settings,
            r#"{"keys":{"normal":{"C-s":"buffer.save"}},"server":"rust-analyzer"}"#
        );

        let empty = Config::parse_plugin("helix", "").unwrap();
        assert_eq!(empty, PluginConfig::default());
        let err = Config::parse_plugin("x", "enable = false").unwrap_err();
        assert!(err.to_string().contains("plugins/x.toml: "), "{err}");
    }

    #[test]
    fn plugins_can_go_without_a_time_limit_and_start_late() {
        let plugin = Config::parse_plugin("x", "timeout-ms = \"none\"\nload = \"lazy\"").unwrap();
        assert_eq!(plugin.timeout, Some(Timeout::Never));
        assert_eq!(plugin.load, Load::Lazy);
        let err = Config::parse_plugin("x", "timeout-ms = \"forever\"").unwrap_err();
        assert!(
            err.to_string().contains("milliseconds or \"none\""),
            "{err}"
        );
        assert!(Config::parse_plugin("x", "load = \"later\"").is_err());
    }

    #[test]
    fn says_where_old_settings_moved() {
        let err = Config::parse("[core]\nplugin-dirs = []").unwrap_err();
        assert!(err.to_string().contains("plugin-dirs moved"), "{err}");
        let err = Config::parse("[plugins.helix]\nx = 1").unwrap_err();
        assert!(err.to_string().contains("[plugins.<name>] moved"), "{err}");
    }

    #[test]
    fn rejects_mistakes() {
        for (text, expected) in [
            ("[core]\ntabwidth = 4", "unknown field"),
            ("[core]\ntab-width = 0", "tab-width must be"),
            ("[core]\nindent = \"spaces\"", "indent must be"),
            ("[core]\nmenu-key = \"C-nope\"", "menu-key"),
            ("[core]\nplugin-timeout-ms = 0", "plugin-timeout-ms must be"),
            ("[core]\nplugin-memory-mib = 1", "plugin-memory-mib must be"),
            ("[editor]\nx = 1", "unknown field"),
        ] {
            let err = Config::parse(text).unwrap_err().to_string();
            assert!(err.contains(expected), "{text:?}: {err}");
        }
    }

    #[test]
    fn parses_the_theme() {
        let config = Config::parse(
            r##"
            [theme]
            keyword = "magenta"
            "function.macro" = "#8be9fd"
            comment = { fg = "bright-black", italic = true }
            "ui.selection" = { bg = 238 }
            "##,
        )
        .unwrap();
        let theme = &config.theme;
        assert_eq!(theme.style("keyword").unwrap().fg, Color::Indexed(5));
        assert_eq!(
            theme.style("function.macro").unwrap().fg,
            Color::Rgb(0x8b, 0xe9, 0xfd)
        );
        let comment = theme.style("comment").unwrap();
        assert_eq!(comment.fg, Color::Indexed(8));
        assert!(comment.italic);
        assert_eq!(theme.style("ui.selection").unwrap().bg, Color::Indexed(238));

        let err = Config::parse("[theme]\nkeyword = \"purple\"").unwrap_err();
        assert!(
            err.to_string().contains("theme.keyword: unknown color"),
            "{err}"
        );
    }
}
