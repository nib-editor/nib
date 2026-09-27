//! What nib's base plugins share (docs/base.md): reading text around a
//! position, text objects, keys and keymaps, edits over every selection,
//! and the command line. Where a cursor sits, and whether a motion moves it
//! or selects, is each base's own.

pub mod cmdline;
pub mod doc;
pub mod edit;
pub mod hints;
pub mod keys;
pub mod tree;

use nib_plugin::nib::plugin::types::Span;
use nib_plugin::nib::plugin::{commands, editor, ui};

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

pub fn error_message(err: editor::Error) -> String {
    match err {
        editor::Error::InvalidPattern(message) => message,
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
    fn strings_are_escaped_for_json_and_regex() {
        assert_eq!(json_string("a \"b\"\\\n"), r#""a \"b\"\\\u000a""#);
        assert_eq!(regex_escape("a.b*"), r"a\.b\*");
    }
}
