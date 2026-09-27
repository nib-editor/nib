//! What the keys after a prefix do, shown in a popup while the next key is
//! awaited.

use nib_plugin::nib::plugin::types::{KeyEvent, Span};

use crate::keys::{self, Keymap};
use crate::span;

/// The popup's lines: `title`, then each key and what it does.
pub fn lines(title: &str, entries: &[(&str, &str)]) -> Vec<Vec<Span>> {
    let key_width = entries.iter().map(|(key, _)| key.len()).max().unwrap_or(0);
    let mut lines = vec![vec![span(title, "ui.popup.title")]];
    lines.extend(entries.iter().map(|(key, what)| {
        vec![
            span(&format!("{key:key_width$}"), "ui.popup.key"),
            span(&format!("  {what}"), ""),
        ]
    }));
    lines
}

/// The popup's lines for a table of the settings' keys, reached by `typed`.
pub fn keymap_lines(typed: &[KeyEvent], table: &Keymap) -> Vec<Vec<Span>> {
    let title: Vec<String> = typed.iter().map(keys::label).collect();
    let entries: Vec<(String, String)> = table
        .iter()
        .map(|(key, binding)| (keys::label(key), keys::describe(binding)))
        .collect();
    let entries: Vec<(&str, &str)> = entries
        .iter()
        .map(|(key, what)| (key.as_str(), what.as_str()))
        .collect();
    lines(&title.join(" "), &entries)
}
