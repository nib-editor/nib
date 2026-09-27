use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::Deserialize;

use crate::Error;
use crate::input::KeyEvent;

/// `plugin.toml`: what the editor needs to know before loading a plugin.
#[derive(Debug, Deserialize)]
pub(crate) struct Manifest {
    /// Also the namespace of the plugin's commands.
    pub name: String,
    pub version: String,
    /// The `nib:plugin` version the plugin was built against.
    pub api: String,
    #[serde(default)]
    pub languages: Vec<LanguageManifest>,
    /// The kinds of events the plugin gets, e.g. "buffer-changed".
    #[serde(default)]
    pub events: Vec<String>,
    /// What it may do beyond the editor API.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// A base: the whole way of editing, keys and all (docs/base.md). Only
    /// the one `[core] base` names runs.
    #[serde(default)]
    pub base: bool,
    /// The key that opens the core menu while this base is in use.
    #[serde(default, rename = "menu-key")]
    pub menu_key: Option<String>,
}

pub(crate) const CAPABILITIES: [&str; 5] =
    ["process", "fs-read", "fs-write", "network", "clipboard"];

/// A language the plugin provides: a tree-sitter grammar as WebAssembly and
/// its queries, as files in the plugin.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) struct LanguageManifest {
    pub name: String,
    pub file_types: Vec<String>,
    pub grammar: String,
    /// Query files by name. The core uses "highlights"; plugins can run any
    /// of them through the syntax API.
    #[serde(default)]
    pub queries: BTreeMap<String, String>,
}

pub(crate) fn read(path: &Path) -> Result<Manifest, Error> {
    let text = fs::read_to_string(path)
        .map_err(|err| Error::Plugin(format!("{}: {err}", path.display())))?;
    parse(&text, &path.display().to_string())
}

/// `origin` says where the manifest came from, for errors.
pub(crate) fn parse(text: &str, origin: &str) -> Result<Manifest, Error> {
    let fail = |message: String| Error::Plugin(format!("{origin}: {message}"));
    let manifest: Manifest = toml::from_str(text).map_err(|err| fail(err.to_string()))?;
    let valid_name = !manifest.name.is_empty()
        && manifest
            .name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !valid_name {
        return Err(fail(format!(
            "name {:?} must be lowercase letters, digits, and '-'",
            manifest.name
        )));
    }
    // Command names start with the plugin's name, so these would pass for
    // core commands.
    if ["buffer", "config", "core", "editor", "view"].contains(&manifest.name.as_str()) {
        return Err(fail(format!("name {:?} is reserved", manifest.name)));
    }
    // A misspelled capability would leave the plugin without it, failing
    // later in confusing ways.
    if let Some(unknown) = manifest
        .capabilities
        .iter()
        .find(|c| !CAPABILITIES.contains(&c.as_str()))
    {
        return Err(fail(format!(
            "unknown capability {unknown:?}; known ones are {}",
            CAPABILITIES.join(", ")
        )));
    }
    if let Some(key) = &manifest.menu_key {
        if !manifest.base {
            return Err(fail("menu-key is only for bases (base = true)".into()));
        }
        key.parse::<KeyEvent>()
            .map_err(|err| fail(format!("menu-key: {err}")))?;
    }
    Ok(manifest)
}
