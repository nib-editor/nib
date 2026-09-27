//! Where nib's settings live, and which plugins they load.

use std::env;
use std::path::{Path, PathBuf};

use nib_core::{Config, plugin_name};

use crate::builtin;
use crate::install::Store;

/// `~/.config/nib`, or under `$XDG_CONFIG_HOME` or `%APPDATA%`.
pub fn config_dir() -> Option<PathBuf> {
    let base = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .or_else(|| env::var_os("APPDATA").map(PathBuf::from))?;
    Some(base.join("nib"))
}

/// `~/.local/share/nib`, or under `$XDG_DATA_HOME` or `%LOCALAPPDATA%`:
/// installed plugins and what plugins keep.
pub fn data_dir() -> Option<PathBuf> {
    let base = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME").map(|home| PathBuf::from(home).join(".local").join("share"))
        })
        .or_else(|| env::var_os("LOCALAPPDATA").map(PathBuf::from))?;
    Some(base.join("nib"))
}

/// Where installed plugins are kept.
pub fn store() -> Option<Store> {
    data_dir().map(|data| Store { data })
}

/// Where compiled plugins are cached, so later starts skip compiling.
pub fn cache_dir() -> Option<PathBuf> {
    let base = env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .or_else(|| env::var_os("LOCALAPPDATA").map(PathBuf::from))?;
    Some(base.join("nib"))
}

pub enum Source {
    /// Index into `builtin::PLUGINS`.
    Builtin(usize),
    Dir(PathBuf),
}

/// A plugin nib knows about, from its build or the settings.
pub struct Entry {
    pub name: String,
    pub enabled: bool,
    pub source: Source,
    /// Its `plugins/<name>.toml`, if it has one.
    pub file: Option<PathBuf>,
    /// Where it was installed from, for installed ones.
    pub origin: Option<String>,
}

/// Every plugin, in load order: built-in ones first, so the keymap is at
/// the bottom of the input stack, then installed ones, then the ones with a
/// `path`. A `path` replaces the plugin of the same name.
pub fn entries(
    config: &Config,
    dir: Option<&Path>,
    store: Option<&Store>,
) -> Result<Vec<Entry>, String> {
    let file = |name: &str| {
        let path = dir?.join("plugins").join(format!("{name}.toml"));
        config.plugins.contains_key(name).then_some(path)
    };
    let mut entries = Vec::new();
    for (i, (name, _, _)) in builtin::PLUGINS.iter().enumerate() {
        let settings = config.plugin(name);
        if settings.path.is_none() {
            entries.push(Entry {
                name: name.to_string(),
                enabled: settings.enabled,
                source: Source::Builtin(i),
                file: file(name),
                origin: None,
            });
        }
    }
    if let Some(store) = store {
        for record in store.records()? {
            let settings = config.plugin(&record.name);
            if settings.path.is_none() {
                entries.push(Entry {
                    enabled: settings.enabled,
                    source: Source::Dir(store.dir(&record.name)),
                    file: file(&record.name),
                    origin: Some(record.source),
                    name: record.name,
                });
            }
        }
    }
    for (name, settings) in &config.plugins {
        if let Some(path) = &settings.path {
            entries.push(Entry {
                name: name.clone(),
                enabled: settings.enabled,
                source: Source::Dir(expand_home(path)),
                file: file(name),
                origin: None,
            });
        }
    }
    Ok(entries)
}

/// Checks that the plugin in `dir` is the one its settings file is named
/// after.
pub fn check_name(name: &str, dir: &Path) -> Result<(), String> {
    let found = plugin_name(dir).map_err(|err| err.to_string())?;
    if found != name {
        return Err(format!(
            "plugins/{name}.toml loads {}, a plugin named {found}",
            dir.display()
        ));
    }
    Ok(())
}

/// Expands a leading `~/`, as plugin paths are often written.
pub fn expand_home(path: &Path) -> PathBuf {
    match (path.strip_prefix("~"), env::var_os("HOME")) {
        (Ok(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => path.to_path_buf(),
    }
}
