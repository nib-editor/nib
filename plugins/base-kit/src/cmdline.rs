//! The command line, as `:` in helix and vim runs it: short names such as
//! `:w` and `:config`, and any command by its full name, as in
//! `:lsp.definition` or `:buffer.open path=src/main.rs`.

use nib_plugin::nib::plugin::{commands, editor, ui, view};
use serde_json::{Map, Value};

/// The short names, for completion: the name, and what it does.
pub const ALIASES: &[(&str, &str)] = &[
    ("write", "Save the current buffer (also w)"),
    ("quit", "Quit (also q; q! drops unsaved changes)"),
    ("wq", "Save and quit (also x)"),
    (
        "buffer-close",
        "Close the buffer (also bc; bc! drops changes)",
    ),
    ("open", "Open a file: open <path> (also o, e, edit)"),
    ("config", "Open config.toml, or a plugin's: config <plugin>"),
    ("config-reload", "Read the settings again"),
];

/// Runs `input`, the line without its `:`.
pub fn run(input: &str) -> Result<(), String> {
    let (command, arg) = input.split_once(' ').unwrap_or((input, ""));
    let arg = arg.trim();
    match command {
        "w" | "write" => save(),
        "q" | "quit" => quit(false),
        "q!" | "quit!" => quit(true),
        "wq" | "x" => save().and_then(|()| quit(false)),
        "bc" | "buffer-close" => view::active().buffer().close(false),
        "bc!" | "buffer-close!" => view::active().buffer().close(true),
        "config" => editor::open_config((!arg.is_empty()).then_some(arg)),
        "config-reload" => {
            editor::reload_config();
            Ok(())
        }
        "o" | "open" | "e" | "edit" if !arg.is_empty() => crate::open_file(arg),
        "o" | "open" | "e" | "edit" => Err(format!(":{command} needs a path")),
        "" => Ok(()),
        name if name.contains('.') => call(name, &args(arg)?),
        _ => Err(format!("unknown command: {command}")),
    }
}

/// The arguments after a command's full name, as the JSON it takes: `{...}`
/// as it is, `key=value` pairs as an object, and nothing as `{}`. A value
/// that reads as JSON, such as `3` or `true`, is that; others are strings.
pub fn args(text: &str) -> Result<String, String> {
    let text = text.trim();
    if text.starts_with('{') {
        return serde_json::from_str::<Value>(text)
            .map(|_| text.to_string())
            .map_err(|err| format!("the arguments are not JSON: {err}"));
    }
    let mut object = Map::new();
    for pair in text.split_whitespace() {
        let (key, value) = pair
            .split_once('=')
            .ok_or_else(|| format!("{pair:?}: arguments are key=value, or JSON"))?;
        let value = serde_json::from_str(value).unwrap_or_else(|_| Value::from(value));
        object.insert(key.to_string(), value);
    }
    Ok(Value::Object(object).to_string())
}

/// What the command being typed may be: the short names, then every
/// command, that start with `prefix`, each with what it does.
pub fn candidates(prefix: &str) -> Vec<(String, String)> {
    let short = ALIASES
        .iter()
        .map(|&(name, what)| (name.to_string(), what.to_string()));
    let mut full = commands::all();
    full.sort();
    short
        .chain(full)
        .filter(|(name, _)| name.starts_with(prefix))
        .collect()
}

fn call(name: &str, args: &str) -> Result<(), String> {
    commands::call(name, args).map(|_| ())
}

fn save() -> Result<(), String> {
    let buffer = view::active().buffer();
    buffer.save(None)?;
    let path = buffer.path().unwrap_or_default();
    ui::show_message(&format!("{path} written"));
    Ok(())
}

fn quit(force: bool) -> Result<(), String> {
    editor::quit(force)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_pairs_or_json() {
        assert_eq!(args("").unwrap(), "{}");
        assert_eq!(
            args("path=src/main.rs force=true n=3").unwrap(),
            r#"{"force":true,"n":3,"path":"src/main.rs"}"#
        );
        assert_eq!(args(r#" {"a": [1]} "#).unwrap(), r#"{"a": [1]}"#);
        assert!(args("{nope").is_err());
        assert!(args("word").is_err());
    }
}
