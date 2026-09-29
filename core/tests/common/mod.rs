//! Helpers shared by the tests that run real plugins.

#![allow(dead_code)]

use std::path::PathBuf;

use nib_core::{Editor, Grid, KeyCode, KeyEvent, Symbol};

/// A plugin built by `cargo xtask build-plugins`.
pub fn plugin_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../target/plugins")
        .join(name);
    assert!(
        dir.join("plugin.toml").is_file(),
        "{} is missing; run `cargo xtask build-plugins` first",
        dir.display()
    );
    dir
}

pub fn key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c))
}

/// Types `keys`, where `<esc>`, `<ret>`, `<bs>`, and `<tab>` are named keys.
pub fn type_keys(editor: &mut Editor, keys: &str) {
    let mut rest = keys;
    while let Some(c) = rest.chars().next() {
        let named = [
            ("<esc>", KeyCode::Escape),
            ("<ret>", KeyCode::Enter),
            ("<bs>", KeyCode::Backspace),
            ("<tab>", KeyCode::Tab),
        ]
        .into_iter()
        .find(|(name, _)| rest.starts_with(name));
        match named {
            Some((name, code)) => {
                editor.handle_key(KeyEvent::new(code));
                rest = &rest[name.len()..];
            }
            None => {
                editor.handle_key(key(c));
                rest = &rest[c.len_utf8()..];
            }
        }
    }
}

/// Opens the core menu and, for each of `steps`, types it and presses
/// Enter, choosing the first row it narrows the list to.
pub fn menu(editor: &mut Editor, steps: &[&str]) {
    editor.handle_key(editor.menu_key());
    for step in steps {
        for c in step.chars() {
            editor.handle_key(key(c));
        }
        editor.handle_key(KeyEvent::new(KeyCode::Enter));
    }
}

/// The screen as text, one string per row.
pub fn screen(editor: &Editor) -> Vec<String> {
    let mut grid = Grid::default();
    editor.render(&mut grid);
    (0..grid.height())
        .map(|y| {
            (0..grid.width())
                .filter_map(|x| match &grid.cell(x, y).symbol {
                    Symbol::Char(c) => Some(c.to_string()),
                    Symbol::Str(s) => Some(s.to_string()),
                    Symbol::Continuation => None,
                })
                .collect()
        })
        .collect()
}
