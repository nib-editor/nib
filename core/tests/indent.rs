//! Indentation by language, from the indent plugin in `plugins/indent`, as
//! the Helix keymap types it. Build the plugins first with
//! `cargo xtask build-plugins`.

use std::path::PathBuf;
use std::{env, fs};

use nib_core::{Config, Editor};

mod common;
use common::{plugin_dir, screen, type_keys};

/// An editor on a file named `name` with `text`, and `settings` for the
/// indent plugin.
fn editor(name: &str, text: &str, settings: &str) -> (Editor, PathBuf) {
    let path = env::temp_dir().join(format!("nib-{}-{name}", std::process::id()));
    fs::write(&path, text).unwrap();
    let mut config = Config::default();
    config.plugins.insert(
        "indent".into(),
        Config::parse_plugin("indent", settings).unwrap(),
    );
    let mut editor = Editor::default();
    editor.apply_config(config);
    editor.open(&path).unwrap();
    for plugin in ["helix", "go", "yaml", "rust", "indent"] {
        editor.load_plugin(&plugin_dir(plugin)).unwrap();
    }
    editor.resize(40, 6);
    editor.catch_up();
    (editor, path)
}

/// What Tab types on a new line.
fn tab_types(name: &str, text: &str, settings: &str) -> String {
    let (mut editor, path) = editor(name, text, settings);
    type_keys(&mut editor, "ggO<tab>x<esc>");
    let line = editor.buffer().text().line(0).to_string();
    fs::remove_file(path).unwrap();
    line
}

#[test]
fn languages_get_their_indentation() {
    assert_eq!(tab_types("a.go", "package a\n", ""), "\tx\n");
    assert_eq!(tab_types("a.yaml", "a: 1\n", ""), "  x\n");
    // Not in the defaults: config.toml's four spaces.
    assert_eq!(tab_types("a.rs", "fn a() {}\n", ""), "    x\n");
}

#[test]
fn settings_replace_the_defaults() {
    let settings = "[settings.languages.yaml]\nindent = 3\n";
    assert_eq!(tab_types("b.yaml", "a: 1\n", settings), "   x\n");
    // A tab is drawn as wide as the buffer's tab width.
    let settings = "[settings.languages.go]\ntab-width = 8\n";
    let (editor, path) = editor("b.go", "\tx\n", settings);
    assert_eq!(screen(&editor)[0].trim_end(), "        x");
    fs::remove_file(path).unwrap();
}

#[test]
fn wrong_settings_are_reported() {
    let (editor, path) = editor(
        "c.go",
        "package c\n",
        "[settings.languages.go]\nindent = 99\n",
    );
    let message = editor.message().unwrap_or_default().to_string();
    assert!(message.contains("languages.go.indent"), "{message}");
    assert!(message.contains("1 to 16 spaces"), "{message}");
    fs::remove_file(path).unwrap();
}
