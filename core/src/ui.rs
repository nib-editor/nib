//! UI parts that plugins put on screen: status line items, panels, popups,
//! and decorations. The core lays them out; plugins never see screen
//! coordinates.

use std::collections::BTreeMap;
use std::ops::Range;

use crate::grid::{Color, Style};
use crate::plugin::PluginId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    /// A theme entry, e.g. "ui.mode.normal".
    pub style: String,
}

pub type StyledLine = Vec<Span>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

#[derive(Clone, Debug)]
pub(crate) struct StatusItem {
    pub owner: PluginId,
    pub id: String,
    pub side: Side,
    pub priority: i32,
    pub content: StyledLine,
}

#[derive(Clone, Debug)]
pub(crate) struct Panel {
    pub id: u32,
    pub owner: PluginId,
    pub lines: Vec<StyledLine>,
    /// A line index and a byte offset into that line's text.
    pub cursor: Option<(u32, u32)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PopupAnchor {
    /// Below or above the line of `offset` in buffer `buffer`.
    Position { buffer: usize, offset: usize },
    /// The bottom-right corner of the text.
    Corner,
}

#[derive(Clone, Debug)]
pub(crate) struct Popup {
    pub id: u32,
    pub owner: PluginId,
    pub anchor: PopupAnchor,
    pub lines: Vec<StyledLine>,
}

/// Text a plugin put after the end of a line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Note {
    pub owner: PluginId,
    pub namespace: String,
    pub at: usize,
    pub text: String,
    pub style: String,
}

/// A style a plugin put over part of a buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Decoration {
    pub owner: PluginId,
    pub namespace: String,
    pub range: Range<usize>,
    pub style: String,
}

fn fg(color: u8) -> Style {
    Style {
        fg: Color::Indexed(color),
        ..Style::default()
    }
}

/// Styles by name, such as "ui.selection" or the tree-sitter capture
/// "function.method": the built-in theme, overridden by `[theme]` in
/// config.toml.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Theme {
    overrides: BTreeMap<String, Style>,
}

impl Theme {
    pub fn new(overrides: BTreeMap<String, Style>) -> Self {
        Self { overrides }
    }

    /// The style for `name`, falling back to its parents: "function.method"
    /// uses "function" when it has no style of its own. `None` keeps the
    /// style around it.
    pub fn style(&self, name: &str) -> Option<Style> {
        let mut name = name;
        loop {
            if let Some(style) = self.overrides.get(name).copied().or_else(|| builtin(name)) {
                return Some(style);
            }
            name = &name[..name.rfind('.')?];
        }
    }
}

/// The built-in theme.
fn builtin(name: &str) -> Option<Style> {
    let mode = |bg| Style {
        fg: Color::Indexed(0),
        bg: Color::Indexed(bg),
        bold: true,
        ..Style::default()
    };
    match name {
        "ui.mode.normal" => Some(mode(4)),
        "ui.mode.insert" => Some(mode(2)),
        "ui.mode.select" => Some(mode(5)),
        "ui.selection" => Some(Style {
            bg: Color::Indexed(8),
            ..Style::default()
        }),
        "ui.menu.selected" => Some(Style {
            reverse: true,
            ..Style::default()
        }),
        // The lines between split views.
        "ui.window" => Some(fg(8)),
        "ui.popup" => Some(Style {
            bg: Color::Indexed(8),
            ..Style::default()
        }),
        "ui.popup.title" => Some(Style {
            bold: true,
            ..Style::default()
        }),
        "ui.popup.key" => Some(fg(3)),
        // Directories in a listing of files.
        "ui.directory" => Some(Style {
            fg: Color::Indexed(4),
            bold: true,
            ..Style::default()
        }),
        // LSP diagnostics: the note after the line and the status counts
        // by severity, and an underline for the range.
        "diagnostic.error" => Some(fg(1)),
        "diagnostic.warning" => Some(fg(3)),
        "diagnostic.info" => Some(fg(4)),
        "diagnostic.hint" => Some(fg(6)),
        "diagnostic.underline" => Some(Style {
            underline: true,
            ..Style::default()
        }),
        "ui.cursor.match" => Some(Style {
            bold: true,
            underline: true,
            ..Style::default()
        }),
        // Syntax, by tree-sitter capture name. The base colors follow the
        // terminal's own palette.
        "keyword" => Some(fg(5)),
        "function" => Some(fg(4)),
        "function.macro" => Some(fg(6)),
        "type" | "constructor" | "attribute" | "label" => Some(fg(3)),
        "string" => Some(fg(2)),
        "escape" | "constant" | "number" | "boolean" => Some(fg(6)),
        // Keys in TOML, YAML, and JSON, and fields.
        "property" | "string.special.key" => Some(fg(4)),
        "punctuation.special" => Some(fg(8)),
        // Markdown.
        "text.title" => Some(Style {
            fg: Color::Indexed(4),
            bold: true,
            ..Style::default()
        }),
        "text.literal" => Some(fg(2)),
        "text.emphasis" => Some(Style {
            italic: true,
            ..Style::default()
        }),
        "text.strong" => Some(Style {
            bold: true,
            ..Style::default()
        }),
        "text.reference" => Some(fg(6)),
        "text.uri" => Some(Style {
            fg: Color::Indexed(6),
            underline: true,
            ..Style::default()
        }),
        "variable.builtin" => Some(fg(1)),
        "comment" => Some(Style {
            fg: Color::Indexed(8),
            italic: true,
            ..Style::default()
        }),
        "ui.error" => Some(Style {
            fg: Color::Indexed(1),
            bold: true,
            ..Style::default()
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_fall_back_to_their_parents_and_config_wins() {
        let theme = Theme::default();
        assert_eq!(theme.style("function.method.call"), builtin("function"));
        assert_eq!(theme.style("nothing.known"), None);

        let red = Style {
            fg: Color::Indexed(1),
            ..Style::default()
        };
        let theme = Theme::new(BTreeMap::from([("function.method".to_string(), red)]));
        assert_eq!(theme.style("function.method.call"), Some(red));
        assert_eq!(theme.style("function"), builtin("function"));
    }
}
