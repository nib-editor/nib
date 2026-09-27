//! Keys as the settings write them (`C-s`, `A-ret`, `space`), and the keymaps
//! of `[settings.keys.<mode>]`: a key runs a command, replays the keys of
//! one of the base's own actions, or leads to a table of keys.

use nib_plugin::nib::plugin::types::{KeyCode, KeyEvent, Modifiers};
use serde_json::Value;

#[derive(Clone)]
pub enum Binding {
    /// A command of the core or another plugin, by its dotted name.
    Command(String),
    /// The keys that do one of the base's actions.
    Keys(Vec<KeyEvent>),
    /// A table of the keys that may follow.
    Prefix(Keymap),
}

pub type Keymap = Vec<(KeyEvent, Binding)>;

/// The keymap of `mode` in `settings`, and what was wrong in it. Wrong
/// entries are left out, so the rest still works. `action` gives the keys
/// that do an action of the base, by its name.
pub fn keymap(
    settings: &Value,
    mode: &str,
    action: &dyn Fn(&str) -> Option<Vec<KeyEvent>>,
) -> (Keymap, Vec<String>) {
    let mut keymap = Keymap::new();
    let mut errors = Vec::new();
    if let Some(entries) = settings["keys"][mode].as_object() {
        parse_table(
            entries,
            &format!("keys.{mode}"),
            action,
            &mut keymap,
            &mut errors,
        );
    }
    (keymap, errors)
}

fn parse_table(
    entries: &serde_json::Map<String, Value>,
    at: &str,
    action: &dyn Fn(&str) -> Option<Vec<KeyEvent>>,
    keymap: &mut Keymap,
    errors: &mut Vec<String>,
) {
    for (key, value) in entries {
        let place = format!("{at}.{key}");
        let key = match parse_key(key) {
            Ok(key) => key,
            Err(err) => {
                errors.push(format!("{place}: {err}"));
                continue;
            }
        };
        let binding = match value {
            Value::String(name) if name.contains('.') => Binding::Command(name.clone()),
            Value::String(name) => match action(name) {
                Some(keys) => Binding::Keys(keys),
                None => {
                    errors.push(format!("{place}: unknown command {name:?}"));
                    continue;
                }
            },
            Value::Object(entries) => {
                let mut prefix = Keymap::new();
                parse_table(entries, &place, action, &mut prefix, errors);
                Binding::Prefix(prefix)
            }
            _ => {
                errors.push(format!("{place}: expected a command name or a table"));
                continue;
            }
        };
        keymap.push((key, binding));
    }
}

pub fn lookup<'a>(keymap: &'a Keymap, key: &KeyEvent) -> Option<&'a Binding> {
    keymap.iter().find(|(k, _)| k == key).map(|(_, b)| b)
}

/// Keys separated by spaces, such as `g g` or `C-w v`.
pub fn parse_keys(text: &str) -> Result<Vec<KeyEvent>, String> {
    text.split_whitespace().map(parse_key).collect()
}

/// A key: a char, or a name such as `ret`, with `C-`, `A-`, and `S-` in
/// front for modifiers.
pub fn parse_key(text: &str) -> Result<KeyEvent, String> {
    let mut modifiers = Modifiers::empty();
    let mut rest = text;
    loop {
        let (modifier, tail) = match rest.split_once('-') {
            Some((m @ ("C" | "A" | "S"), tail)) if !tail.is_empty() => (m, tail),
            _ => break,
        };
        modifiers |= match modifier {
            "C" => Modifiers::CTRL,
            "A" => Modifiers::ALT,
            _ => Modifiers::SHIFT,
        };
        rest = tail;
    }
    let mut chars = rest.chars();
    let code = match (chars.next(), chars.next()) {
        (Some(c), None) => KeyCode::Char(c),
        _ => match rest {
            "ret" | "enter" => KeyCode::Enter,
            "esc" => KeyCode::Escape,
            "space" => KeyCode::Char(' '),
            "tab" => KeyCode::Tab,
            "backspace" => KeyCode::Backspace,
            "del" => KeyCode::Delete,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" => KeyCode::PageUp,
            "pagedown" => KeyCode::PageDown,
            "minus" => KeyCode::Char('-'),
            f if f.starts_with('F') => KeyCode::F(
                f[1..]
                    .parse()
                    .map_err(|_| format!("unknown key {text:?}"))?,
            ),
            _ => return Err(format!("unknown key {text:?}")),
        },
    };
    // A char already says whether it is shifted, as the core sends it.
    if let KeyCode::Char(c) = code
        && modifiers.contains(Modifiers::SHIFT)
    {
        modifiers.remove(Modifiers::SHIFT);
        let upper: Vec<char> = c.to_uppercase().collect();
        if let [upper] = upper[..] {
            return Ok(KeyEvent {
                code: KeyCode::Char(upper),
                modifiers,
            });
        }
    }
    Ok(KeyEvent { code, modifiers })
}

/// A key as the settings write it, for hints.
pub fn label(key: &KeyEvent) -> String {
    let mut label = String::new();
    for (flag, prefix) in [
        (Modifiers::CTRL, "C-"),
        (Modifiers::ALT, "A-"),
        (Modifiers::SHIFT, "S-"),
    ] {
        if key.modifiers.contains(flag) {
            label.push_str(prefix);
        }
    }
    match key.code {
        KeyCode::Char(' ') => label.push_str("space"),
        KeyCode::Char(c) => label.push(c),
        KeyCode::Enter => label.push_str("ret"),
        KeyCode::Escape => label.push_str("esc"),
        KeyCode::Tab => label.push_str("tab"),
        KeyCode::Backspace => label.push_str("backspace"),
        KeyCode::Delete => label.push_str("del"),
        KeyCode::Up => label.push_str("up"),
        KeyCode::Down => label.push_str("down"),
        KeyCode::Left => label.push_str("left"),
        KeyCode::Right => label.push_str("right"),
        KeyCode::Home => label.push_str("home"),
        KeyCode::End => label.push_str("end"),
        KeyCode::PageUp => label.push_str("pageup"),
        KeyCode::PageDown => label.push_str("pagedown"),
        KeyCode::F(n) => label.push_str(&format!("F{n}")),
    }
    label
}

/// What a binding does, for hints.
pub fn describe(binding: &Binding) -> String {
    match binding {
        Binding::Command(name) => name.clone(),
        Binding::Keys(keys) => {
            let keys: Vec<String> = keys.iter().map(label).collect();
            format!("as {}", keys.join(" "))
        }
        Binding::Prefix(_) => "…".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keys_parse_as_the_settings_write_them() {
        let key = |code, modifiers| KeyEvent { code, modifiers };
        assert_eq!(
            parse_key("a"),
            Ok(key(KeyCode::Char('a'), Modifiers::empty()))
        );
        assert_eq!(
            parse_key("C-s"),
            Ok(key(KeyCode::Char('s'), Modifiers::CTRL))
        );
        assert_eq!(
            parse_key("S-a"),
            Ok(key(KeyCode::Char('A'), Modifiers::empty()))
        );
        assert_eq!(parse_key("S-tab"), Ok(key(KeyCode::Tab, Modifiers::SHIFT)));
        assert_eq!(parse_key("A-ret"), Ok(key(KeyCode::Enter, Modifiers::ALT)));
        assert_eq!(
            parse_key("-"),
            Ok(key(KeyCode::Char('-'), Modifiers::empty()))
        );
        assert_eq!(parse_key("F5"), Ok(key(KeyCode::F(5), Modifiers::empty())));
        assert!(parse_key("hello").is_err());
        assert_eq!(label(&parse_key("C-space").unwrap()), "C-space");
        assert_eq!(parse_keys("C-w v").unwrap().len(), 2);
    }

    #[test]
    fn wrong_entries_are_left_out() {
        let settings = json!({"keys": {"normal": {
            "C-s": "buffer.save",
            "q": "no_such_command",
            "nope": "undo",
            "g": {"a": "goto_file_start"},
        }}});
        let action = |name: &str| (name != "no_such_command").then(Vec::new);
        let (keymap, errors) = keymap(&settings, "normal", &action);
        assert_eq!(keymap.len(), 2);
        assert_eq!(
            errors,
            [
                "keys.normal.nope: unknown key \"nope\"",
                "keys.normal.q: unknown command \"no_such_command\"",
            ]
        );
    }
}
