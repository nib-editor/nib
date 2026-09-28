//! Where Emacs's motions go, from the chars of `fundamental-mode`'s
//! standard syntax table: words, sentences, paragraphs, expressions, and
//! columns.

use base_kit::doc::Doc;
use base_kit::text::Text;
use nib_plugin::nib::plugin::{settings, view};

/// Word constituents: letters, digits, `$`, and `%`.
pub fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '$' || c == '%'
}

/// Symbol constituents: with words, they make up an expression's atom.
pub fn is_symbol(c: char) -> bool {
    "_&*+-/<=>|".contains(c)
}

fn is_blank(c: char) -> bool {
    c == ' ' || c == '\t'
}

fn opening(c: char) -> Option<char> {
    match c {
        ')' => Some('('),
        ']' => Some('['),
        '}' => Some('{'),
        _ => None,
    }
}

fn closing(c: char) -> Option<char> {
    match c {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        _ => None,
    }
}

/// `M-f`: past the next word, or to the end of the buffer if none is
/// left. `None` when already at the end.
pub fn forward_word(t: &mut Text, pos: u64) -> Option<u64> {
    t.char_at(pos)?;
    let mut at = pos;
    while let Some(c) = t.char_at(at)
        && !is_word(c)
    {
        at = t.next(at).expect("a char is here");
    }
    while let Some(c) = t.char_at(at)
        && is_word(c)
    {
        at = t.next(at).expect("a char is here");
    }
    Some(at)
}

/// `M-b`: to the start of the word before, or to the start of the buffer
/// if none is left. `None` when already at the start.
pub fn backward_word(t: &mut Text, pos: u64) -> Option<u64> {
    t.prev(pos)?;
    let mut at = pos;
    while let Some(prev) = t.prev(at)
        && !t.char_at(prev).is_some_and(is_word)
    {
        at = prev;
    }
    while let Some(prev) = t.prev(at)
        && t.char_at(prev).is_some_and(is_word)
    {
        at = prev;
    }
    Some(at)
}

/// Moves over `count` words, back for a negative count, as far as it goes.
pub fn words(t: &mut Text, pos: u64, count: i64) -> u64 {
    let mut at = pos;
    for _ in 0..count.unsigned_abs() {
        let next = if count > 0 {
            forward_word(t, at)
        } else {
            backward_word(t, at)
        };
        match next {
            Some(next) => at = next,
            None => break,
        }
    }
    at
}

/// Whether line `line` holds nothing but whitespace.
pub fn blank_line(doc: &Doc, line: u64) -> bool {
    let start = doc.line_start(line);
    let end = doc.line_end(start);
    doc.slice(start, end).chars().all(is_blank)
}

/// `M-}`: to the blank line after the paragraph, or the end.
pub fn forward_paragraph(doc: &Doc, pos: u64) -> u64 {
    let count = doc.line_count();
    let mut line = doc.line_of(pos);
    while line < count && blank_line(doc, line) {
        line += 1;
    }
    while line < count && !blank_line(doc, line) {
        line += 1;
    }
    if line >= count {
        doc.len
    } else {
        doc.line_start(line)
    }
}

/// `M-{`: to the blank line before the paragraph, or the start.
pub fn backward_paragraph(doc: &Doc, pos: u64) -> u64 {
    let here = doc.line_of(pos);
    let mut line = if pos > doc.line_start(here) && !blank_line(doc, here) {
        Some(here)
    } else {
        here.checked_sub(1)
    };
    while let Some(l) = line
        && blank_line(doc, l)
    {
        line = l.checked_sub(1);
    }
    while let Some(l) = line
        && !blank_line(doc, l)
    {
        line = l.checked_sub(1);
    }
    line.map_or(0, |l| doc.line_start(l))
}

/// The paragraph around or after `pos`, as the start of its first line and
/// the end of its last, or `None` past the last one.
fn paragraph_at(doc: &Doc, pos: u64) -> Option<(u64, u64)> {
    let count = doc.line_count();
    let mut line = doc.line_of(pos);
    while line < count && blank_line(doc, line) {
        line += 1;
    }
    if line >= count {
        return None;
    }
    let mut first = line;
    while first > 0 && !blank_line(doc, first - 1) {
        first -= 1;
    }
    let mut last = line;
    while last + 1 < count && !blank_line(doc, last + 1) {
        last += 1;
    }
    Some((doc.line_start(first), doc.line_end(doc.line_start(last))))
}

/// The ends of the sentences of the paragraph `start..end`: after `.`,
/// `?`, or `!` and any closing quotes or brackets, when a line end, a tab,
/// or two spaces follow. The paragraph's end is one too.
fn sentence_ends(doc: &Doc, start: u64, end: u64) -> Vec<u64> {
    let text = doc.slice(start, end);
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut ends = Vec::new();
    for (i, &(_, c)) in chars.iter().enumerate() {
        if !".?!…‽".contains(c) {
            continue;
        }
        let mut j = i + 1;
        while j < chars.len() && "]\"')}”’»›".contains(chars[j].1) {
            j += 1;
        }
        let at = chars.get(j).map_or(text.len(), |&(b, _)| b);
        let follows = &text[at..];
        if follows.is_empty()
            || follows.starts_with('\n')
            || follows.starts_with('\t')
            || follows.starts_with("  ")
        {
            ends.push(start + at as u64);
        }
    }
    ends.push(end);
    ends.dedup();
    ends
}

/// `M-e`: to the end of the sentence.
pub fn forward_sentence(doc: &Doc, pos: u64) -> Option<u64> {
    let mut from = pos;
    loop {
        let (start, end) = paragraph_at(doc, from)?;
        if let Some(&e) = sentence_ends(doc, start, end).iter().find(|&&e| e > pos) {
            return Some(e);
        }
        from = doc.line_start(doc.line_of(end) + 1);
        if from >= doc.len {
            return None;
        }
    }
}

/// The first char of a sentence that starts at or after `pos`.
fn skip_space(doc: &Doc, pos: u64, end: u64) -> u64 {
    let text = doc.slice(pos, end);
    let blank = text.len() - text.trim_start().len();
    pos + blank as u64
}

/// `M-a`: to the start of the sentence.
pub fn backward_sentence(doc: &Doc, pos: u64) -> Option<u64> {
    let mut at = pos;
    loop {
        let (start, end) = match paragraph_at(doc, at) {
            Some((start, end)) if start < pos => (start, end),
            _ => {
                // The paragraph before.
                let line = doc.line_of(at);
                let mut l = line.checked_sub(1)?;
                while blank_line(doc, l) {
                    l = l.checked_sub(1)?;
                }
                at = doc.line_start(l);
                continue;
            }
        };
        let mut starts = vec![skip_space(doc, start, end)];
        for e in sentence_ends(doc, start, end) {
            if e < end {
                starts.push(skip_space(doc, e, end));
            }
        }
        if let Some(&s) = starts.iter().rev().find(|&&s| s < pos) {
            return Some(s);
        }
        let line = doc.line_of(start);
        if line == 0 {
            return None;
        }
        at = doc.line_start(line - 1);
    }
}

/// `C-M-f`: past the next expression: an atom of word and symbol chars, a
/// bracketed list, or a string. Punctuation between is skipped.
pub fn forward_sexp(t: &mut Text, pos: u64) -> Result<Option<u64>, String> {
    let mut at = pos;
    let c = loop {
        let Some(c) = t.char_at(at) else {
            return Ok(None);
        };
        if is_word(c) || is_symbol(c) || c == '"' || c == '\\' || "([{)]}".contains(c) {
            break c;
        }
        at = t.next(at).expect("a char is here");
    };
    if opening(c).is_some() {
        return Err("Containing expression ends prematurely".into());
    }
    if let Some(close) = closing(c) {
        return list_end(t, at, c, close)
            .map(Some)
            .ok_or_else(|| "Unbalanced parentheses".into());
    }
    if c == '"' {
        return string_end(t, at)
            .map(Some)
            .ok_or_else(|| "Unbalanced parentheses".into());
    }
    while let Some(c) = t.char_at(at) {
        if c == '\\' {
            at = t.next(at).expect("a char is here");
            if t.char_at(at).is_none() {
                break;
            }
        } else if !(is_word(c) || is_symbol(c)) {
            break;
        }
        at = t.next(at).expect("a char is here");
    }
    Ok(Some(at))
}

/// `C-M-b`: to the start of the expression before.
pub fn backward_sexp(t: &mut Text, pos: u64) -> Result<Option<u64>, String> {
    let mut at = pos;
    let (prev, c) = loop {
        let Some(prev) = t.prev(at) else {
            return Ok(None);
        };
        let c = t.char_at(prev).expect("a char is here");
        if is_word(c) || is_symbol(c) || c == '"' || "([{)]}".contains(c) {
            break (prev, c);
        }
        at = prev;
    };
    if closing(c).is_some() {
        return Err("Containing expression ends prematurely".into());
    }
    if let Some(open) = opening(c) {
        return list_start(t, prev, open, c)
            .map(Some)
            .ok_or_else(|| "Unbalanced parentheses".into());
    }
    if c == '"' {
        let mut at = prev;
        loop {
            let Some(p) = t.prev(at) else {
                return Err("Unbalanced parentheses".into());
            };
            at = p;
            if t.char_at(at) == Some('"') && !escaped(t, at) {
                return Ok(Some(at));
            }
        }
    }
    let mut at = prev;
    while let Some(p) = t.prev(at)
        && t.char_at(p).is_some_and(|c| is_word(c) || is_symbol(c))
    {
        at = p;
    }
    Ok(Some(at))
}

fn escaped(t: &mut Text, pos: u64) -> bool {
    let mut n = 0;
    let mut at = pos;
    while let Some(p) = t.prev(at)
        && t.char_at(p) == Some('\\')
    {
        n += 1;
        at = p;
    }
    n % 2 == 1
}

/// The position after the `close` that ends the list opened at `open_at`.
fn list_end(t: &mut Text, open_at: u64, open: char, close: char) -> Option<u64> {
    let mut depth = 0;
    let mut at = open_at;
    let mut in_string = false;
    while let Some(c) = t.char_at(at) {
        let next = t.next(at)?;
        if in_string {
            if c == '\\' {
                at = t.next(next).unwrap_or(next);
                continue;
            }
            in_string = c != '"';
        } else if c == '"' {
            in_string = true;
        } else if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Some(next);
            }
        }
        at = next;
    }
    None
}

/// The position of the `open` that starts the list closed at `close_at`.
fn list_start(t: &mut Text, close_at: u64, open: char, close: char) -> Option<u64> {
    let mut depth = 0;
    let mut at = t.next(close_at)?;
    while let Some(prev) = t.prev(at) {
        let c = t.char_at(prev)?;
        if c == close {
            depth += 1;
        } else if c == open {
            depth -= 1;
            if depth == 0 {
                return Some(prev);
            }
        }
        at = prev;
    }
    None
}

/// The position after the string that starts at `quote_at`.
fn string_end(t: &mut Text, quote_at: u64) -> Option<u64> {
    let mut at = t.next(quote_at)?;
    loop {
        let c = t.char_at(at)?;
        let next = t.next(at)?;
        match c {
            '\\' => at = t.next(next)?,
            '"' => return Some(next),
            _ => at = next,
        }
    }
}

/// `C-M-u`: the bracket that opens the list around `pos`.
pub fn up_list(t: &mut Text, pos: u64) -> Option<u64> {
    let mut depth = 0;
    let mut at = pos;
    while let Some(prev) = t.prev(at) {
        let c = t.char_at(prev)?;
        if opening(c).is_some() {
            depth += 1;
        } else if closing(c).is_some() {
            if depth == 0 {
                return Some(prev);
            }
            depth -= 1;
        }
        at = prev;
    }
    None
}

/// `C-M-d`: just inside the next list, unless the list around ends first.
pub fn down_list(t: &mut Text, pos: u64) -> Option<u64> {
    let mut at = pos;
    while let Some(c) = t.char_at(at) {
        let next = t.next(at)?;
        if closing(c).is_some() {
            return Some(next);
        }
        if opening(c).is_some() {
            return None;
        }
        at = next;
    }
    None
}

/// The width tabs take: the core's tab-width.
pub fn tab_width() -> u32 {
    u32::from(settings::tab_width(&view::active().buffer()))
}

/// How many columns `c` takes after column `col`.
fn width(c: char, col: u32, tab: u32) -> u32 {
    match c {
        '\t' => tab - col % tab,
        c if is_wide(c) => 2,
        _ => 1,
    }
}

/// East Asian wide and fullwidth chars, and emoji.
fn is_wide(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x115f
            | 0x2e80..=0x303e
            | 0x3041..=0x33ff
            | 0x3400..=0x4dbf
            | 0x4e00..=0x9fff
            | 0xa000..=0xa4cf
            | 0xac00..=0xd7a3
            | 0xf900..=0xfaff
            | 0xfe30..=0xfe4f
            | 0xff00..=0xff60
            | 0xffe0..=0xffe6
            | 0x1f300..=0x1f64f
            | 0x1f900..=0x1f9ff
            | 0x20000..=0x3fffd)
}

/// The display column of `pos` on its line.
pub fn column(doc: &Doc, pos: u64) -> u32 {
    let start = doc.line_start(doc.line_of(pos));
    let tab = tab_width();
    doc.slice(start, pos)
        .chars()
        .fold(0, |col, c| col + width(c, col, tab))
}

/// The position at display column `col` of the line starting at `start`,
/// or after the char that spans it, or the line's end; and the column
/// reached.
pub fn at_column(doc: &Doc, start: u64, col: u32) -> (u64, u32) {
    let end = doc.line_end(start);
    let tab = tab_width();
    let mut at = start;
    let mut reached = 0;
    for c in doc.slice(start, end).chars() {
        if reached >= col {
            break;
        }
        reached += width(c, reached, tab);
        at += c.len_utf8() as u64;
    }
    (at, reached)
}

/// The next indent point after column `col` in `line`'s text: past the
/// word at `col`, then past the whitespace after it.
pub fn indent_point(line: &str, col: u32, tab: u32) -> Option<u32> {
    let mut reached = 0;
    let mut chars = line.chars().peekable();
    while reached < col {
        let c = chars.next()?;
        reached += width(c, reached, tab);
    }
    while let Some(&c) = chars.peek()
        && !is_blank(c)
    {
        reached += width(c, reached, tab);
        chars.next();
    }
    let mut spaced = false;
    while let Some(&c) = chars.peek()
        && is_blank(c)
    {
        reached += width(c, reached, tab);
        chars.next();
        spaced = true;
    }
    (spaced && chars.peek().is_some()).then_some(reached)
}

/// Whitespace from column `from` to column `to`, with tabs when the indent
/// setting says so.
pub fn spaces(from: u32, to: u32, tabs: bool, tab: u32) -> String {
    let mut out = String::new();
    let mut col = from;
    if tabs {
        while col + (tab - col % tab) <= to {
            out.push('\t');
            col += tab - col % tab;
        }
    }
    while col < to {
        out.push(' ');
        col += 1;
    }
    out
}

/// The indentation's width in columns.
pub fn indent_width(text: &str, tab: u32) -> u32 {
    text.chars()
        .take_while(|&c| is_blank(c))
        .fold(0, |col, c| col + width(c, col, tab))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indent_points_follow_words_and_spaces() {
        assert_eq!(indent_point("    one two", 0, 8), Some(4));
        assert_eq!(indent_point("    one two", 2, 8), Some(4));
        assert_eq!(indent_point("    one two", 4, 8), Some(8));
        assert_eq!(indent_point("    one two", 6, 8), Some(8));
        assert_eq!(indent_point("    one two", 8, 8), None);
        assert_eq!(indent_point("one", 0, 8), None);
        assert_eq!(indent_point("ab", 5, 8), None);
    }

    #[test]
    fn chars_fall_in_their_classes() {
        assert!(is_word('$') && is_word('日') && !is_word('_'));
        assert!(is_symbol('_') && is_symbol('-') && !is_symbol('.'));
        assert!(is_wide('日') && !is_wide('a'));
        assert_eq!(spaces(0, 5, false, 4), "     ");
        assert_eq!(spaces(1, 9, true, 4), "\t\t ");
        assert_eq!(indent_width("\t  x", 4), 6);
    }
}
