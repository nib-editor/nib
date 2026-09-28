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

use nib_plugin::nib::plugin::types::{Error, Span};
use nib_plugin::nib::plugin::{buffer, commands, ui, view};

pub fn span(text: &str, style: &str) -> Span {
    Span {
        text: text.into(),
        style: style.into(),
    }
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
