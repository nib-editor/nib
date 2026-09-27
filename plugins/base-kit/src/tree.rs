//! Keys that use the syntax tree: text objects, selecting nodes, and
//! matching pairs. Everything returns `None` for buffers without a tree, so
//! callers can fall back to working on the text.

use nib_plugin::nib::plugin::editor::Buffer;
use nib_plugin::nib::plugin::syntax::{self, Node};

/// Pairs the tree can match, as node kinds.
const PAIRS: [(&str, &str); 6] = [
    ("(", ")"),
    ("[", "]"),
    ("{", "}"),
    ("<", ">"),
    ("\"", "\""),
    ("'", "'"),
];

/// The text object behind a key after `mi`, `ma`, `]`, or `[`.
pub fn object_name(key: char) -> Option<&'static str> {
    Some(match key {
        'f' => "function",
        't' => "class",
        'a' => "parameter",
        'c' => "comment",
        'T' => "test",
        _ => return None,
    })
}

/// `mif`, `maf`, and so on: the smallest object around `pos`.
pub fn object_around(buffer: &Buffer, capture: &str, pos: u64) -> Option<(u64, u64)> {
    syntax::captures(buffer, "textobjects", capture, pos, pos + 1)
        .into_iter()
        .filter(|&(start, end)| start <= pos && pos < end)
        .min_by_key(|&(start, end)| end - start)
}

/// `]f`, `[f`, and so on: the `count`th object starting after `pos`, or
/// before it.
pub fn next_object(
    buffer: &Buffer,
    capture: &str,
    pos: u64,
    forward: bool,
    count: u64,
) -> Option<(u64, u64)> {
    let len = buffer.len();
    let skip = count.saturating_sub(1) as usize;
    // Querying to the end of a large file takes milliseconds; the object
    // wanted is usually close, so look nearby first.
    let mut window = 4096;
    loop {
        let (start, end) = if forward {
            (pos, pos.saturating_add(window).min(len))
        } else {
            (pos.saturating_sub(window), pos)
        };
        let found = syntax::captures(buffer, "textobjects", capture, start, end);
        let hit = if forward {
            found.into_iter().filter(|&(s, _)| s > pos).nth(skip)
        } else {
            // Objects starting before the window may have others between
            // them and it, so they only count once the window reaches 0.
            found
                .into_iter()
                .filter(|&(s, _)| s < pos && (s >= start || start == 0))
                .rev()
                .nth(skip)
        };
        if hit.is_some() || (forward && end == len) || (!forward && start == 0) {
            return hit;
        }
        window *= 4;
    }
}

/// `Alt-o`: the smallest node larger than `start..end` that covers it.
pub fn expand(buffer: &Buffer, start: u64, end: u64) -> Option<(u64, u64)> {
    let mut node = syntax::node_at(buffer, start, end, false)?;
    while (node.start, node.end) == (start, end) {
        node = syntax::parent(buffer, &node)?;
    }
    Some((node.start, node.end))
}

/// `Alt-i` without an `Alt-o` to undo: the first named child of the node
/// that is exactly `start..end`.
pub fn shrink(buffer: &Buffer, start: u64, end: u64) -> Option<(u64, u64)> {
    let node = syntax::node_at(buffer, start, end, true)?;
    if (node.start, node.end) != (start, end) {
        return None;
    }
    let child = syntax::children(buffer, &node)
        .into_iter()
        .find(|child| child.named)?;
    Some((child.start, child.end))
}

/// `Alt-n` and `Alt-p`: the next or previous named sibling of the node at
/// `start..end`, or of the nearest ancestor that has one.
pub fn sibling(buffer: &Buffer, start: u64, end: u64, forward: bool) -> Option<(u64, u64)> {
    let mut node = syntax::node_at(buffer, start, end, true)?;
    loop {
        let parent = syntax::parent(buffer, &node)?;
        let siblings = syntax::children(buffer, &parent);
        let at = siblings.iter().position(|s| s.id == node.id)?;
        let found = if forward {
            siblings[at + 1..].iter().find(|s| s.named)
        } else {
            siblings[..at].iter().rev().find(|s| s.named)
        };
        if let Some(found) = found {
            return Some((found.start, found.end));
        }
        node = parent;
    }
}

/// `mm`: the other end of the pair whose opening or closing is at `pos`.
pub fn matching_pair(buffer: &Buffer, pos: u64) -> Option<u64> {
    let node = syntax::node_at(buffer, pos, pos + 1, false)?;
    if node.named || node.start != pos {
        return None;
    }
    let parent = syntax::parent(buffer, &node)?;
    let children = syntax::children(buffer, &parent);
    let (first, last) = ends(&children)?;
    if !is_pair(&first.kind, &last.kind) {
        return None;
    }
    if first.id == node.id {
        Some(last.start)
    } else if last.id == node.id {
        Some(first.start)
    } else {
        None
    }
}

/// `mi(`, `ma"`, and so on: the positions of the opening and closing of
/// the innermost pair of `c` around `pos`.
pub fn surrounding_pair(buffer: &Buffer, pos: u64, c: char) -> Option<(u64, u64)> {
    let c = c.to_string();
    let &(open, close) = PAIRS
        .iter()
        .find(|(open, close)| *open == c || *close == c)?;
    let mut node = syntax::node_at(buffer, pos, pos + 1, false)?;
    loop {
        if let Some((first, last)) = ends(&syntax::children(buffer, &node))
            && first.kind == open
            && last.kind == close
            && first.start <= pos
            && pos < last.end
        {
            return Some((first.start, last.start));
        }
        node = syntax::parent(buffer, &node)?;
    }
}

fn ends(children: &[Node]) -> Option<(&Node, &Node)> {
    match children {
        [first, .., last] => Some((first, last)),
        _ => None,
    }
}

fn is_pair(open: &str, close: &str) -> bool {
    PAIRS.contains(&(open, close))
}
