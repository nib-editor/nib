//! What the keys after a prefix such as `g` or `m` do, shown in a popup
//! while the next key is awaited, as Helix does.

use base_kit::hints;
use base_kit::keys::{self, Binding};
use base_kit::leader::Leader;
use nib_plugin::nib::plugin::commands;
use nib_plugin::nib::plugin::types::Span;

use crate::Pending;

const OBJECTS: [(&str, &str); 5] = [
    ("f", "function"),
    ("t", "type"),
    ("a", "argument"),
    ("c", "comment"),
    ("T", "test"),
];

/// The popup's lines for `pending`, or `None` for keys that wait for any
/// char, such as `f`.
pub fn lines(pending: Pending) -> Option<Vec<Vec<Span>>> {
    let (title, entries): (&str, Vec<(&str, &str)>) = match pending {
        Pending::Goto => (
            "Goto",
            vec![
                ("g", "first line, or line <count>"),
                ("e", "last line"),
                ("h", "line start"),
                ("l", "line end"),
                ("s", "first non-blank"),
                ("d", "definition"),
                ("n", "next buffer"),
                ("p", "previous buffer"),
            ],
        ),
        Pending::Match => (
            "Match",
            vec![
                ("m", "matching bracket"),
                ("i", "select inside"),
                ("a", "select around"),
            ],
        ),
        Pending::MatchPair { around } => {
            let title = if around {
                "Select around"
            } else {
                "Select inside"
            };
            let mut entries = OBJECTS.to_vec();
            entries.push(("( [ { \" ' `", "pair"));
            (title, entries)
        }
        Pending::Object { forward } => {
            let title = if forward { "Next" } else { "Previous" };
            (title, OBJECTS.to_vec())
        }
        Pending::Window => (
            "Window",
            vec![
                ("v", "split side by side"),
                ("s", "split one above another"),
                ("w", "next view"),
                ("h j k l", "view to the left, below, above, right"),
                ("q", "close this view"),
                ("o", "close the other views"),
            ],
        ),
        Pending::Space => return None,
        Pending::Find(_) | Pending::Replace | Pending::Register => return None,
    };
    Some(hints::lines(title, &entries))
}

/// The keys under Space: this keymap's own, then what plugins suggest,
/// with their commands' descriptions, then the ones that lost.
pub fn leader_lines(leader: &Leader) -> Vec<Vec<Span>> {
    let described: Vec<(String, String)> = commands::all();
    let describe = |name: &str| {
        described
            .iter()
            .find(|(command, _)| command == name)
            .map_or(name, |(_, what)| what.as_str())
            .to_string()
    };
    let mut entries: Vec<(String, String)> = [
        ("w", "views…"),
        ("y", "yank to the clipboard"),
        ("p", "paste the clipboard after"),
        ("P", "paste the clipboard before"),
    ]
    .iter()
    .map(|&(key, what)| (key.to_string(), what.to_string()))
    .collect();
    for (key, binding) in &leader.keymap {
        let what = match binding {
            Binding::Command(name) => describe(name),
            other => keys::describe(other),
        };
        entries.push((keys::label(key), what));
    }
    for lost in &leader.taken {
        let what = format!("taken: {} ({})", lost.command, lost.plugin);
        entries.push((lost.keys.clone(), what));
    }
    let entries: Vec<(&str, &str)> = entries
        .iter()
        .map(|(key, what)| (key.as_str(), what.as_str()))
        .collect();
    hints::lines("Space", &entries)
}
