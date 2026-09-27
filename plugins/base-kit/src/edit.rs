//! Selections and edits over every selection at once, the same whichever
//! way a base places its cursors.

use nib_plugin::nib::plugin::editor::View;
use nib_plugin::nib::plugin::settings;
use nib_plugin::nib::plugin::types::{Edit, SelRange, Selection, UndoMode};
use nib_plugin::nib::plugin::ui::{self, Decoration};

use crate::doc::{self, Doc};
use crate::{error_message, tree};

pub fn range(anchor: u64, head: u64) -> SelRange {
    SelRange { anchor, head }
}

pub fn point(pos: u64) -> SelRange {
    range(pos, pos)
}

pub fn insertion(pos: u64, text: String) -> Edit {
    Edit {
        start: pos,
        end: pos,
        text,
    }
}

pub fn deletion(start: u64, end: u64) -> Edit {
    Edit {
        start,
        end,
        text: String::new(),
    }
}

pub fn set_ranges(view: &View, ranges: Vec<SelRange>, primary: u32) {
    let primary = primary.min(ranges.len().saturating_sub(1) as u32);
    let _ = view.set_selection(&Selection { ranges, primary });
}

/// The edits `edit` makes of the selections.
pub fn edits_for(view: &View, edit: impl Fn(&SelRange) -> Option<Edit>) -> Vec<Edit> {
    view.selection().ranges.iter().filter_map(edit).collect()
}

/// Applies `edits` as a new undo step, mapping the selections through them.
pub fn apply(view: &View, edits: &[Edit]) -> bool {
    if edits.is_empty() {
        return false;
    }
    let version = view.buffer().version();
    view.apply(version, edits, None, UndoMode::NewStep).is_ok()
}

/// Applies one edit per selection and makes each selection
/// `start..end`, counted in bytes from the start of its edit's text.
pub fn apply_placing(view: &View, mut changes: Vec<(Edit, u64, u64)>, undo: UndoMode) -> bool {
    changes.sort_by_key(|(edit, _, _)| edit.start);
    let mut shift = 0i64;
    let ranges = changes
        .iter()
        .map(|(edit, start, end)| {
            let at = (edit.start as i64 + shift) as u64;
            shift += edit.text.len() as i64 - (edit.end - edit.start) as i64;
            range(at + start, at + end)
        })
        .collect::<Vec<_>>();
    let edits: Vec<Edit> = changes.into_iter().map(|(edit, _, _)| edit).collect();
    let primary = view.selection().primary;
    let after = Selection {
        primary: primary.min(ranges.len().saturating_sub(1) as u32),
        ranges,
    };
    let version = view.buffer().version();
    view.apply(version, &edits, Some(&after), undo).is_ok()
}

pub fn flip_selections(view: &View) {
    let selection = view.selection();
    let ranges = selection
        .ranges
        .iter()
        .map(|r| range(r.head, r.anchor))
        .collect();
    set_ranges(view, ranges, selection.primary);
}

pub fn delete_selections(view: &View) {
    let edits = edits_for(view, |r| {
        let (from, to) = (r.anchor.min(r.head), r.anchor.max(r.head));
        (from < to).then(|| deletion(from, to))
    });
    apply(view, &edits);
}

/// Replaces every char of each selection with `c`, keeping line breaks.
pub fn replace_with(view: &View, c: char) {
    let doc = Doc::new(view.buffer());
    let changes = view
        .selection()
        .ranges
        .iter()
        .filter(|r| r.anchor != r.head)
        .map(|r| {
            let (from, to) = (r.anchor.min(r.head), r.anchor.max(r.head));
            let text: String = doc
                .slice(from, to)
                .chars()
                .map(|ch| if ch == '\n' { ch } else { c })
                .collect();
            let len = text.len() as u64;
            let (start, end) = if r.head < r.anchor {
                (len, 0)
            } else {
                (0, len)
            };
            (
                Edit {
                    start: from,
                    end: to,
                    text,
                },
                start,
                end,
            )
        })
        .collect();
    apply_placing(view, changes, UndoMode::NewStep);
}

/// The text Tab inserts, from the core's indent setting.
pub fn indent_unit() -> String {
    match settings::get("indent").as_deref() {
        Some("\"tab\"") => "\t".to_string(),
        Some(n) => " ".repeat(n.parse().unwrap_or(4)),
        None => "    ".to_string(),
    }
}

/// The lines each selection touches, each once.
pub fn selected_lines(doc: &Doc, view: &View) -> Vec<u64> {
    let mut lines: Vec<u64> = view
        .selection()
        .ranges
        .iter()
        .flat_map(|r| {
            let (from, to) = (r.anchor.min(r.head), r.anchor.max(r.head));
            let last = doc.prev_grapheme(to).max(from);
            doc.line_of(from)..=doc.line_of(last)
        })
        .collect();
    lines.sort_unstable();
    lines.dedup();
    lines
}

/// Indents or unindents the selected lines by one unit.
pub fn indent(view: &View, more: bool) {
    let doc = Doc::new(view.buffer());
    let unit = indent_unit();
    let width = if unit == "\t" { 1 } else { unit.len() };
    let edits: Vec<Edit> = selected_lines(&doc, view)
        .into_iter()
        .filter_map(|line| {
            let start = doc.line_start(line);
            let leading = doc::indentation(&doc, start);
            if more {
                (doc.line_end(start) > start).then(|| insertion(start, unit.clone()))
            } else {
                // A tab counts as a whole unit.
                let mut end = start;
                for (i, c) in leading.chars().enumerate() {
                    if i >= width {
                        break;
                    }
                    end += c.len_utf8() as u64;
                    if c == '\t' {
                        break;
                    }
                }
                (end > start).then(|| deletion(start, end))
            }
        })
        .collect();
    apply(view, &edits);
}

/// Joins the selected lines, or the line with the next one, replacing each
/// line break and the next line's indentation with a space.
pub fn join_lines(view: &View) {
    let doc = Doc::new(view.buffer());
    let mut breaks: Vec<u64> = view
        .selection()
        .ranges
        .iter()
        .flat_map(|r| {
            let (from, to) = (r.anchor.min(r.head), r.anchor.max(r.head));
            let first = doc.line_of(from);
            let last = doc.line_of(doc.prev_grapheme(to).max(from)).max(first + 1);
            first..last
        })
        .collect();
    breaks.sort_unstable();
    breaks.dedup();
    let edits: Vec<Edit> = breaks
        .into_iter()
        .filter(|&line| line + 1 < doc.line_count())
        .map(|line| {
            let newline = doc.line_end(doc.line_start(line));
            let next = newline + 1;
            let text_start = doc::first_non_blank(&doc, next);
            let empty = text_start == doc.line_end(next);
            Edit {
                start: newline,
                end: text_start,
                text: if empty { String::new() } else { " ".into() },
            }
        })
        .collect();
    apply(view, &edits);
}

/// The next match of `pattern` after the primary selection, or the previous
/// one before it, wrapping around the buffer. Says so when it wraps, finds
/// nothing, or the pattern is wrong.
pub fn find(view: &View, pattern: &str, backward: bool) -> Option<(u64, u64)> {
    let buffer = view.buffer();
    let selection = view.selection();
    let r = selection.ranges[selection.primary as usize];
    let (from, to) = (r.anchor.min(r.head), r.anchor.max(r.head));
    let (start, wrap_start) = if backward {
        (from, buffer.len())
    } else {
        (to, 0)
    };
    let found = match buffer.find(pattern, start, backward) {
        Ok(Some(found)) => Some((found, false)),
        Ok(None) => match buffer.find(pattern, wrap_start, backward) {
            Ok(found) => found.map(|found| (found, true)),
            Err(err) => {
                ui::show_message(&error_message(err));
                return None;
            }
        },
        Err(err) => {
            ui::show_message(&error_message(err));
            return None;
        }
    };
    let Some((found, wrapped)) = found else {
        ui::show_message(&format!("no matches for {pattern}"));
        return None;
    };
    if wrapped {
        ui::show_message("search wrapped around");
    }
    Some(found)
}

/// Replaces the selections with the matches of `pattern` inside them.
pub fn select_matches(view: &View, pattern: &str) {
    let buffer = view.buffer();
    let mut ranges = Vec::new();
    for r in view.selection().ranges {
        match buffer.find_all(pattern, r.anchor.min(r.head), r.anchor.max(r.head)) {
            Ok(found) => ranges.extend(
                found
                    .into_iter()
                    .filter(|(start, end)| start < end)
                    .map(|(start, end)| range(start, end)),
            ),
            Err(err) => return ui::show_message(&error_message(err)),
        }
    }
    if ranges.is_empty() {
        ui::show_message(&format!("no matches for {pattern}"));
    } else {
        set_ranges(view, ranges, 0);
    }
}

/// The text object named by `c` around `pos` (`f` for a function, and so
/// on), or the inside or all of the pair of `c` around it.
pub fn object_or_pair(doc: &Doc, pos: u64, c: char, around: bool) -> Option<(u64, u64)> {
    if let Some(name) = tree::object_name(c) {
        let part = if around { "around" } else { "inside" };
        return tree::object_around(&doc.buffer, &format!("{name}.{part}"), pos);
    }
    // The tree knows which brackets are in strings and comments; the text
    // is the fallback, e.g. inside a comment.
    let (open, close) = tree::surrounding_pair(&doc.buffer, pos, c)
        .or_else(|| doc::surrounding_pair(doc, pos, c))?;
    Some(if around {
        (open, close + 1)
    } else {
        (open + 1, close)
    })
}

/// The `count`th text object named by `c` after `from`, or before it.
pub fn next_object(doc: &Doc, c: char, from: u64, forward: bool, count: u64) -> Option<(u64, u64)> {
    let name = tree::object_name(c)?;
    // Jumping between arguments means landing on them, not their commas.
    let part = if c == 'a' { "inside" } else { "around" };
    tree::next_object(&doc.buffer, &format!("{name}.{part}"), from, forward, count)
}

/// Highlights the bracket that pairs with the one at `pos`. Only the syntax
/// tree is asked: searching the text for a bracket without a pair would
/// scan to the end of the file on every key.
pub fn highlight_match(doc: &Doc, pos: u64) {
    let decorations: Vec<Decoration> = tree::matching_pair(&doc.buffer, pos)
        .map(|other| Decoration {
            start: other,
            end: other + 1,
            style: "ui.cursor.match".into(),
        })
        .into_iter()
        .collect();
    ui::set_decorations(&doc.buffer, "match", &decorations);
}
