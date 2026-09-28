//! What nib's base plugins share (docs/base.md): reading text around a
//! position, text objects, keys and keymaps, edits over every selection,
//! and the command line. Where a cursor sits, and whether a motion moves it
//! or selects, is each base's own.

pub mod cmdline;
pub mod doc;
pub mod edit;
pub mod hints;
pub mod keys;
pub mod leader;
pub mod line_edit;
pub mod text;
pub mod tree;

use nib_plugin::nib::plugin::types::{Edit, Error, Selection, Span, UndoMode};
use nib_plugin::nib::plugin::view::Direction;
use nib_plugin::nib::plugin::{buffer, commands, ui, view};

pub fn span(text: &str, style: &str) -> Span {
    Span {
        text: text.into(),
        style: style.into(),
    }
}

/// The name within `plugin` of command `name`, when it is one of
/// `plugin`'s own. A plugin runs those itself: the core cannot call back
/// into a plugin while it runs, so `commands.call` would find it busy.
pub fn own_command<'a>(name: &'a str, plugin: &str) -> Option<&'a str> {
    name.strip_prefix(plugin)?.strip_prefix('.')
}

/// Calls a command without arguments, showing its error if it fails.
pub fn call_or_show(command: &str) {
    if let Err(err) = commands::call(command, "") {
        ui::show_message(&err);
    }
}

/// The text of the match of `regex` at `start` and of each of its groups,
/// the whole match first; a group that took no part is empty.
pub fn match_groups(
    buffer: &buffer::Buffer,
    regex: &str,
    start: u64,
) -> Result<Vec<String>, String> {
    let groups = buffer
        .find_groups(regex, start, false)
        .map_err(error_message)?
        .unwrap_or_default();
    Ok(groups
        .into_iter()
        .map(|group| {
            group
                .and_then(|r| buffer.slice(r.start, r.end).ok())
                .unwrap_or_default()
        })
        .collect())
}

/// Whether a replacement refers to a group, as `\1` does.
pub fn refers_to_groups(replacement: &str) -> bool {
    let mut chars = replacement.chars();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.next().is_some_and(|d| ('1'..='9').contains(&d)) {
            return true;
        }
    }
    false
}

/// Opens `path` in the focused view.
pub fn open_file(path: &str) -> Result<(), String> {
    let buffer = buffer::open(path)?;
    view::active().show(&buffer);
    Ok(())
}

/// Shows `text` in this plugin's buffer `name`, made if need be, in a new
/// view below the focused one unless that shows it: help and lists too
/// long for a message. `keys` work in it, as `buffer.set-keys` takes them.
pub fn show_listing(name: &str, text: &str, keys: &[(&str, &str)]) {
    let rewrite = |buffer: &buffer::Buffer| {
        let edit = Edit {
            start: 0,
            end: buffer.len(),
            text: text.into(),
        };
        buffer.apply(buffer.version(), &[edit], UndoMode::NewStep)
    };
    // Ours if we may write it: other plugins' buffers are read-only to us.
    let buffer = buffer::all()
        .into_iter()
        .filter(|b| b.path().is_none() && b.name() == name)
        .find(|b| rewrite(b).is_ok())
        .unwrap_or_else(|| {
            let buffer = buffer::create(name);
            rewrite(&buffer).expect("a plugin writes its own buffer");
            buffer
        });
    let keys: Vec<(String, String)> = keys
        .iter()
        .map(|(key, command)| (key.to_string(), command.to_string()))
        .collect();
    buffer
        .set_keys(&keys)
        .expect("keys of this plugin's commands");
    if view::active().buffer().name() != name {
        view::split(Direction::Horizontal);
        view::active().show(&buffer);
    }
    let _ = view::active().set_selection(&Selection {
        ranges: vec![edit::point(0)],
        primary: 0,
    });
}

/// Closes the focused view, unless it is the last, and the listing it
/// shows.
pub fn close_listing() {
    let buffer = view::active().buffer();
    let _ = view::close();
    let _ = buffer.close(true);
}

/// `s` as a JSON string, for command arguments.
pub fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn error_message(err: Error) -> String {
    match err {
        Error::InvalidPattern(message) | Error::Other(message) => message,
        other => format!("{other:?}"),
    }
}

/// Escapes regex syntax, so the text is searched for as it is.
pub fn regex_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if "\\.+*?()|[]{}^$#&-~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacements_may_refer_to_groups() {
        assert!(refers_to_groups(r"<\1>"));
        assert!(!refers_to_groups(r"\\1 \0 &"));
    }

    #[test]
    fn strings_are_escaped_for_json_and_regex() {
        assert_eq!(json_string("a \"b\"\\\n"), r#""a \"b\"\\\u000a""#);
        assert_eq!(regex_escape("a.b*"), r"a\.b\*");
    }
}
