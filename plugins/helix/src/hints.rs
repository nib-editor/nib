//! What the keys after a prefix such as `g` or `m` do, shown in a popup
//! while the next key is awaited, as Helix does.

use base_kit::hints;
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
        Pending::Space => (
            "Space",
            vec![
                ("f", "open a file"),
                ("w", "views…"),
                ("k", "show what it is"),
                ("?", "run a command by name"),
                ("y", "yank to the clipboard"),
                ("p", "paste the clipboard after"),
                ("P", "paste the clipboard before"),
            ],
        ),
        Pending::Find(_) | Pending::Replace | Pending::Register => return None,
    };
    Some(hints::lines(title, &entries))
}
