mod clipboard;
mod commands;
mod draw;
mod install;
mod pluginbuild;
mod plugintest;
mod scaffold;
mod settings;
mod terminal;

mod builtin {
    include!(concat!(env!("OUT_DIR"), "/builtin_plugins.rs"));
}

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use nib_core::{Config, Editor, PluginSource, plugin_name};

use settings::{Entry, Source};

fn main() -> ExitCode {
    let args: Vec<_> = env::args_os().skip(1).collect();
    match args.first().and_then(|a| a.to_str()) {
        Some("config") => return commands::config(&args[1..]),
        Some("plugin") => return commands::plugin(&args[1..]),
        Some("--help" | "-h") => {
            println!("{}", commands::USAGE);
            return ExitCode::SUCCESS;
        }
        _ => {}
    }

    let mut files = Vec::new();
    let mut plugins = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg == "--plugin" {
            let Some(dir) = args.next() else {
                eprintln!("nib: --plugin needs a directory\n{}", commands::USAGE);
                return ExitCode::FAILURE;
            };
            plugins.push(PathBuf::from(dir));
        } else {
            files.push(PathBuf::from(arg));
        }
    }

    let mut editor = Editor::default();
    editor.set_background_parsing(true);
    // Without one, as over SSH with no display, plugins get a clipboard
    // inside the editor.
    if let Some(system) = clipboard::System::new() {
        editor.set_clipboard(Box::new(system));
    }
    // Broken settings should not keep the editor from starting: fall back to
    // the defaults and say why.
    let dir = settings::config_dir();
    let (config, config_error) = match dir.as_deref().map(settings::load) {
        Some(Ok(config)) => (config, None),
        Some(Err(err)) => (Config::default(), Some(err)),
        None => (Config::default(), None),
    };
    // A broken record of installed plugins leaves them out, not nib.
    let store = settings::store();
    let (entries, store_error) = match settings::entries(&config, dir.as_deref(), store.as_ref()) {
        Ok(entries) => (entries, None),
        Err(err) => {
            let entries = settings::entries(&config, dir.as_deref(), None);
            (entries.expect("nothing to read without a store"), Some(err))
        }
    };
    editor.apply_config(config);
    if let Some(store) = &store {
        let loaded = entries
            .iter()
            .filter(|e| {
                e.enabled && matches!(&e.source, Source::Dir(dir) if *dir == store.dir(&e.name))
            })
            .map(|e| e.name.clone())
            .collect();
        let updates = install::StoreUpdates {
            store: store.clone(),
            loaded,
        };
        editor.set_plugin_updates(Some(std::sync::Arc::new(updates)));
    }
    for path in &files {
        if let Err(err) = editor.open(path) {
            eprintln!("nib: {}: {err}", path.display());
            return ExitCode::FAILURE;
        }
    }
    editor.set_plugin_cache_dir(settings::cache_dir());
    editor.set_plugin_data_dir(settings::data_dir().map(|dir| dir.join("plugins")));
    let failures = match load_plugins(&mut editor, entries, &plugins) {
        Ok(failures) => failures,
        Err(err) => {
            eprintln!("nib: {err}");
            return ExitCode::FAILURE;
        }
    };
    if let Some(err) = config_error {
        editor.show_message(format!("{err}; using the defaults"));
    } else if let Some(err) = store_error {
        editor.show_message(format!("{err}; installed plugins left out"));
    } else if !failures.is_empty() {
        editor.show_message(failures.join("; "));
    }
    // The keymap takes every key, so nothing else tells people the menu key.
    if editor.message().is_none() {
        editor.show_message(format!("{}: plugin menu", editor.settings().menu_key));
    }

    if let Err(err) = terminal::run(&mut editor) {
        eprintln!("nib: {err}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// Loads the enabled plugins in order, then the `--plugin` ones, which
/// replace a plugin of the same name, e.g. while working on it. A plugin
/// that fails to load is left out and returned as a message, so it cannot
/// keep nib from starting, such as an installed one made for another nib.
/// A `--plugin` one that fails is an error: it was asked for by hand.
fn load_plugins(
    editor: &mut Editor,
    entries: Vec<Entry>,
    extra: &[PathBuf],
) -> Result<Vec<String>, String> {
    let replaced = extra
        .iter()
        .map(|dir| plugin_name(dir))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| err.to_string())?;
    let mut failures = Vec::new();
    let chosen: Vec<Source> = entries
        .into_iter()
        .filter(|entry| entry.enabled && !replaced.contains(&entry.name))
        .filter_map(|entry| {
            let checked = match &entry.source {
                Source::Builtin(_) => Ok(()),
                Source::Dir(dir) => settings::check_name(&entry.name, dir),
            };
            match checked {
                Ok(()) => Some(entry.source),
                Err(err) => {
                    failures.push(err);
                    None
                }
            }
        })
        .collect();
    let sources: Vec<_> = chosen
        .iter()
        .map(|source| match source {
            Source::Builtin(i) => {
                let (_, manifest, files) = builtin::PLUGINS[*i];
                PluginSource::Bytes { manifest, files }
            }
            Source::Dir(dir) => PluginSource::Dir(dir),
        })
        .collect();
    for loaded in editor.load_plugins(&sources) {
        if let Err(err) = loaded {
            failures.push(err.to_string());
        }
    }
    for dir in extra {
        editor.load_plugin(dir).map_err(|err| err.to_string())?;
    }
    if builtin::PLUGINS.is_empty() {
        editor.show_message("built without the standard plugins; run `cargo xtask build-plugins`");
    }
    Ok(failures)
}
