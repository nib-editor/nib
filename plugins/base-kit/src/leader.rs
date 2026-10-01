//! The keys under a base's leader (docs/design/bases/base.md): the base keeps some for
//! itself, and plugins suggest the rest in their manifests. Earlier ones
//! win; the ones that lose are kept to say so.

use nib_plugin::nib::plugin::input::LeaderKey;
use nib_plugin::nib::plugin::types::KeyEvent;

use crate::keys::{Binding, Keymap, parse_keys};

/// The table under the leader, and the suggested keys that lost to keys
/// taken before them.
pub struct Leader {
    pub keymap: Keymap,
    pub taken: Vec<LeaderKey>,
}

/// Builds the table from `suggested`, in order, leaving out the first keys
/// in `own`, which the base keeps.
pub fn keymap(own: &[KeyEvent], suggested: Vec<LeaderKey>) -> Leader {
    let mut keymap = Keymap::new();
    let mut taken = Vec::new();
    for key in suggested {
        let placed = match parse_keys(&key.keys) {
            Ok(keys) if !keys.is_empty() && !own.contains(&keys[0]) => {
                place(&mut keymap, &keys, &key.command)
            }
            _ => false,
        };
        if !placed {
            taken.push(key);
        }
    }
    Leader { keymap, taken }
}

/// Puts `command` at `keys`, making tables on the way, unless something
/// is there already or on the way.
pub(crate) fn place(keymap: &mut Keymap, keys: &[KeyEvent], command: &str) -> bool {
    let (first, rest) = keys.split_first().expect("keys are never empty");
    let at = keymap.iter().position(|(key, _)| key == first);
    match (at, rest.is_empty()) {
        (Some(_), true) => false,
        (None, true) => {
            keymap.push((*first, Binding::Command(command.to_string())));
            true
        }
        (Some(i), false) => match &mut keymap[i].1 {
            Binding::Prefix(table) => place(table, rest, command),
            _ => false,
        },
        (None, false) => {
            let mut table = Keymap::new();
            place(&mut table, rest, command);
            keymap.push((*first, Binding::Prefix(table)));
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{lookup, parse_key};

    fn suggest(plugin: &str, keys: &str, command: &str) -> LeaderKey {
        LeaderKey {
            plugin: plugin.into(),
            keys: keys.into(),
            command: command.into(),
        }
    }

    #[test]
    fn earlier_keys_win() {
        let key = |text| parse_key(text).unwrap();
        let leader = keymap(
            &[key("y")],
            vec![
                suggest("picker", "f", "picker.files"),
                suggest("lsp", "c d", "lsp.definition"),
                suggest("lsp", "c r", "lsp.rename"),
                // The base keeps y; f is picker's; c is a table.
                suggest("clip", "y", "clip.copy"),
                suggest("other", "f", "other.find"),
                suggest("other", "c", "other.code"),
                suggest("other", "f x", "other.under"),
            ],
        );
        let names: Vec<&str> = leader.taken.iter().map(|k| k.command.as_str()).collect();
        assert_eq!(
            names,
            ["clip.copy", "other.find", "other.code", "other.under"]
        );
        assert!(matches!(
            lookup(&leader.keymap, &key("f")),
            Some(Binding::Command(name)) if name == "picker.files"
        ));
        let Some(Binding::Prefix(code)) = lookup(&leader.keymap, &key("c")) else {
            panic!("c is a table");
        };
        assert_eq!(code.len(), 2);
    }
}
