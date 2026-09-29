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
    /// One line on what it is, for the core menu.
    #[serde(default)]
    pub description: Option<String>,
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
    /// Keys for the plugin's own commands, under the base's leader.
    #[serde(default)]
    pub keys: BTreeMap<String, String>,
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
    for (keys, command) in &manifest.keys {
        let own = command
            .strip_prefix(&manifest.name)
            .is_some_and(|rest| rest.starts_with('.') && rest.len() > 1);
        if !own {
            return Err(fail(format!(
                "keys.{keys}: {command:?} is not one of {}'s commands",
                manifest.name
            )));
        }
        let parsed: Result<Vec<KeyEvent>, _> = keys.split_whitespace().map(str::parse).collect();
        match parsed {
            Ok(parsed) if !parsed.is_empty() => {}
            Ok(_) => return Err(fail("keys: a key is missing".into())),
            Err(err) => return Err(fail(format!("keys.{keys}: {err}"))),
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn check(extra: &str) -> Result<Manifest, Error> {
        parse(
            &format!("name = \"demo\"\nversion = \"0.1.0\"\napi = \"0.5\"\n{extra}"),
            "test",
        )
    }

    #[test]
    fn keys_are_for_the_plugins_own_commands() {
        let manifest = check("[keys]\nw = \"demo.count\"\n\"c d\" = \"demo.go\"").unwrap();
        assert_eq!(manifest.keys.len(), 2);
        for (keys, wrong) in [
            ("[keys]\nw = \"lsp.hover\"", "not one of demo's commands"),
            ("[keys]\nw = \"demo\"", "not one of demo's commands"),
            ("[keys]\nw = \"democracy.x\"", "not one of demo's commands"),
            ("[keys]\n\"C-nope\" = \"demo.x\"", "keys.C-nope"),
            ("[keys]\n\" \" = \"demo.x\"", "a key is missing"),
        ] {
            let err = check(keys).unwrap_err().to_string();
            assert!(err.contains(wrong), "{keys}: {err}");
        }
    }
}
