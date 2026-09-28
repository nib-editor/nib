//! vim's text objects: `iw`, `aw`, `ip`, `i(`, `i"`, and the rest.

use base_kit::doc::Doc;

use crate::motion::char_class;
use base_kit::text::Text;

/// The text an object covers, `start..end`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Object {
    pub start: u64,
    pub end: u64,
    pub linewise: bool,
}

fn chars(start: u64, end: u64) -> Object {
    Object {
        start,
        end,
        linewise: false,
    }
}

/// The run of chars of one class that `pos` is in, on its line.
fn run(t: &mut Text, pos: u64, big: bool) -> Option<(u64, u64)> {
    let c = t.char_at(pos).filter(|&c| c != '\n')?;
    let kind = char_class(c, big);
    let same = |c: Option<char>| c.is_some_and(|c| c != '\n' && char_class(c, big) == kind);
    let mut start = pos;
    while let Some(prev) = t.prev(start) {
        if !same(t.char_at(prev)) {
            break;
        }
        start = prev;
    }
    let mut end = pos;
    while same(t.char_at(end)) {
        end = t.next(end)?;
    }
    Some((start, end))
}

fn is_blank(t: &mut Text, pos: u64) -> bool {
    matches!(t.char_at(pos), Some(' ' | '\t'))
}

/// `iw`, `aw`, `iW`, and `aW`, `count` of them.
pub fn word(t: &mut Text, pos: u64, big: bool, around: bool, count: u64) -> Option<Object> {
    let (mut start, mut end) = run(t, pos, big)?;
    let on_blank = char_class(t.char_at(pos)?, big) == 0;
    if !around {
        for _ in 1..count {
            (_, end) = run(t, end, big)?;
        }
        return Some(chars(start, end));
    }
    if on_blank {
        // The blanks and the word after them.
        (_, end) = run(t, end, big).unwrap_or((end, end));
        for _ in 1..count {
            end = word_and_blanks(t, end, big)?;
        }
        return Some(chars(start, end));
    }
    for _ in 1..count {
        end = blanks_and_word(t, end, big)?;
    }
    if is_blank(t, end) {
        (_, end) = run(t, end, big)?;
    } else {
        // No blanks after: the ones before go instead.
        while let Some(prev) = t.prev(start) {
            if !is_blank(t, prev) {
                break;
            }
            start = prev;
        }
    }
    Some(chars(start, end))
}

/// Past the blanks at `pos`, then the word after them.
fn blanks_and_word(t: &mut Text, pos: u64, big: bool) -> Option<u64> {
    let mut end = pos;
    if is_blank(t, end) {
        (_, end) = run(t, end, big)?;
    }
    Some(run(t, end, big)?.1)
}

/// Past the word at `pos`, then the blanks after it.
fn word_and_blanks(t: &mut Text, pos: u64, big: bool) -> Option<u64> {
    let (_, mut end) = run(t, pos, big)?;
    if is_blank(t, end) {
        (_, end) = run(t, end, big)?;
    }
    Some(end)
}

/// `ip` and `ap`: lines of text, or blank lines, and with `ap` the blank
/// lines after them.
pub fn paragraph(doc: &Doc, pos: u64, around: bool, count: u64) -> Object {
    let last = doc.last_line();
    let blank = |line: u64| {
        let start = doc.line_start(line);
        doc.slice(start, doc.line_end(start)).trim().is_empty()
    };
    let line = doc.line_of(pos).min(last);
    let mut first = line;
    while first > 0 && blank(first - 1) == blank(line) {
        first -= 1;
    }
    let mut end = line;
    let mut runs = if around { count * 2 } else { count };
    // Each run is lines that are all blank or all not.
    loop {
        let kind = blank(end);
        while end < last && blank(end + 1) == kind {
            end += 1;
        }
        runs -= 1;
        if runs == 0 || end >= last {
            break;
        }
        end += 1;
    }
    // `ap` at the end takes the blank lines before instead.
    if around && runs > 0 && !blank(line) {
        while first > 0 && blank(first - 1) {
            first -= 1;
        }
    }
    Object {
        start: doc.line_start(first),
        end: (doc.line_end(doc.line_start(end)) + 1).min(doc.len),
        linewise: true,
    }
}

/// The unmatched `open` before `pos`.
fn opening(t: &mut Text, pos: u64, open: char, close: char) -> Option<u64> {
    let mut depth = 0;
    let mut p = pos;
    loop {
        p = t.prev(p)?;
        match t.char_at(p)? {
            c if c == close => depth += 1,
            c if c == open && depth == 0 => return Some(p),
            c if c == open => depth -= 1,
            _ => {}
        }
    }
}

/// The `close` pairing with the `open` at `pos`.
fn closing(t: &mut Text, pos: u64, open: char, close: char) -> Option<u64> {
    let mut depth = 0;
    let mut p = pos;
    loop {
        p = t.next(p)?;
        match t.char_at(p)? {
            c if c == open => depth += 1,
            c if c == close && depth == 0 => return Some(p),
            c if c == close => depth -= 1,
            _ => {}
        }
    }
}

/// `i(`, `a(`, and the other brackets: the `count`th pair around `pos`.
pub fn pair(
    t: &mut Text,
    pos: u64,
    open: char,
    close: char,
    around: bool,
    count: u64,
) -> Option<Object> {
    let mut start = match t.char_at(pos)? {
        c if c == open => pos,
        c if c == close => {
            let mut depth = 0;
            let mut p = pos;
            loop {
                p = t.prev(p)?;
                match t.char_at(p)? {
                    c if c == close => depth += 1,
                    c if c == open && depth == 0 => break p,
                    c if c == open => depth -= 1,
                    _ => {}
                }
            }
        }
        _ => opening(t, pos, open, close)?,
    };
    for _ in 1..count {
        start = opening(t, start, open, close)?;
    }
    let end = closing(t, start, open, close)?;
    if around {
        return Some(chars(start, end + 1));
    }
    let mut inner = t.next(start)?;
    let inner_end = end;
    // A block written over lines leaves out the line breaks after the
    // opening and before the closing, and the indent before the closing.
    if t.char_at(inner) == Some('\n') {
        inner = t.next(inner)?;
        let line_start = t.line_start(end);
        let mut only_blanks = true;
        let mut p = line_start;
        while p < end {
            if !is_blank(t, p) {
                only_blanks = false;
                break;
            }
            p = t.next(p)?;
        }
        if only_blanks && line_start > inner {
            return Some(Object {
                start: inner,
                end: line_start,
                linewise: true,
            });
        }
    }
    Some(chars(inner, inner_end.max(inner)))
}

/// `i"`, `a"`, and the other quotes, on the line of `pos`: the quotes
/// around it, or the first pair after it.
pub fn quote(t: &mut Text, pos: u64, q: char, around: bool) -> Option<Object> {
    let start = t.line_start(pos);
    let end = t.line_end(pos);
    let mut quotes = Vec::new();
    let mut p = start;
    let mut escaped = false;
    while p < end {
        let c = t.char_at(p)?;
        if c == q && !escaped {
            quotes.push(p);
        }
        escaped = c == '\\' && !escaped;
        p = t.next(p)?;
    }
    let pairs: Vec<(u64, u64)> = quotes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&[open, close]| (open, close))
        .collect();
    let (open, close) = pairs
        .iter()
        .copied()
        .find(|&(open, close)| open <= pos && pos <= close)
        .or_else(|| pairs.iter().copied().find(|&(open, _)| open > pos))?;
    if !around {
        return Some(chars(open + 1, close));
    }
    let mut from = open;
    let mut to = close + 1;
    if is_blank(t, to) {
        while is_blank(t, to) {
            to = t.next(to)?;
        }
    } else {
        while let Some(prev) = t.prev(from) {
            if prev < start || !is_blank(t, prev) {
                break;
            }
            from = prev;
        }
    }
    Some(chars(from, to))
}
