//! vim's motions over the text: where each one takes the cursor. Motions
//! that need the screen, marks, or searches are in `lib.rs`.

use base_kit::doc::{self, Doc, FindKind};

use crate::text::Text;

/// How an operator takes the text a motion passes over.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// Up to the target, leaving the char there out, as `w`.
    Exclusive,
    /// Through the char at the target, as `e`.
    Inclusive,
    /// Whole lines, as `j`.
    Linewise,
}

/// A cursor moving over the text as vim's does: on a char, or on the end
/// of a line (its line break, or the end of the buffer).
struct Walk<'t, 'a> {
    t: &'t mut Text<'a>,
    pos: u64,
    big: bool,
}

/// What stepping did, as vim's `inc_cursor` and `dec_cursor` say.
#[derive(PartialEq, Eq)]
enum Stepped {
    /// Within the line.
    Char,
    /// Onto the end of the line.
    OntoEnd,
    /// To another line.
    Line,
    /// Nowhere: at the start or end of the buffer.
    Edge,
}

impl Walk<'_, '_> {
    /// The class of the char at the cursor; the end of a line is blank.
    fn cls(&mut self) -> u32 {
        match self.t.char_at(self.pos) {
            None | Some('\n') => 0,
            Some(c) => char_class(c, self.big),
        }
    }

    fn on_line_end(&mut self) -> bool {
        matches!(self.t.char_at(self.pos), None | Some('\n'))
    }

    fn on_empty_line(&mut self) -> bool {
        self.on_line_end() && self.pos == self.t.line_start(self.pos)
    }

    fn on_last_line(&mut self) -> bool {
        let doc = self.t.doc;
        doc.line_of(self.pos.min(doc.len)) >= doc.last_line()
    }

    fn inc(&mut self) -> Stepped {
        if !self.on_line_end() {
            self.pos = self.t.next(self.pos).unwrap_or(self.pos);
            return if self.on_line_end() {
                Stepped::OntoEnd
            } else {
                Stepped::Char
            };
        }
        if self.on_last_line() {
            return Stepped::Edge;
        }
        self.pos = self.t.next(self.pos).unwrap_or(self.pos);
        Stepped::Line
    }

    fn dec(&mut self) -> Stepped {
        let Some(prev) = self.t.prev(self.pos) else {
            return Stepped::Edge;
        };
        let crossed = self.pos == self.t.line_start(self.pos);
        self.pos = prev;
        if crossed {
            Stepped::Line
        } else {
            Stepped::Char
        }
    }

    /// Moves while the class is `class`; true when it hit an edge.
    fn skip(&mut self, class: u32, forward: bool) -> bool {
        while self.cls() == class {
            let stepped = if forward { self.inc() } else { self.dec() };
            if stepped == Stepped::Edge {
                return true;
            }
        }
        false
    }
}

/// vim's classes of chars: 0 for blanks, 1 for punctuation, 2 for keyword
/// chars, and more for scripts that make words of their own, such as
/// hiragana and kanji. A WORD has only blanks and the rest.
pub fn char_class(c: char, big: bool) -> u32 {
    let class = match c as u32 {
        0x20 | 0x09 | 0x3000 => 0,
        0x3001..=0x303f | 0xff01..=0xff0f | 0xff1a..=0xff20 => 1,
        0x3040..=0x309f => 0x3040,
        0x30a0..=0x30ff => 0x30a0,
        0x4e00..=0x9fff | 0x3400..=0x4dbf => 0x4e00,
        0xac00..=0xd7a3 => 0xac00,
        _ if crate::text::is_keyword(c) => 2,
        _ => 1,
    };
    if big && class != 0 { 1 } else { class }
}

/// `w` and `W`, `count` times: the start of the next word, or an empty
/// line. `for_op` is for an operator, which stops at the end of a line
/// the last word ends. `Err` has where the cursor got to when it hit the
/// end of the buffer.
pub fn word_start(t: &mut Text, pos: u64, big: bool, count: u64, for_op: bool) -> Result<u64, u64> {
    let mut w = Walk { t, pos, big };
    for n in (0..count).rev() {
        let last = n == 0;
        let class = w.cls();
        let last_line = w.on_last_line();
        let stepped = w.inc();
        if stepped == Stepped::Edge || (stepped != Stepped::Char && last_line) {
            return Err(w.pos);
        }
        if stepped != Stepped::Char && for_op && last {
            return Ok(w.pos);
        }
        if class != 0 {
            while w.cls() == class {
                let stepped = w.inc();
                if stepped == Stepped::Edge || (stepped != Stepped::Char && for_op && last) {
                    return Ok(w.pos);
                }
            }
        }
        while w.cls() == 0 {
            if w.on_empty_line() {
                break;
            }
            let stepped = w.inc();
            if stepped == Stepped::Edge || (stepped != Stepped::Char && last_line) {
                return Err(w.pos);
            }
            // An operator stops at the end of the line, blanks and all.
            if stepped == Stepped::OntoEnd && for_op && last {
                return Ok(w.pos);
            }
        }
    }
    Ok(w.pos)
}

/// `e` and `E`, `count` times: the end of the word. With `stop`, as for
/// `cw`, the end of the word the cursor is in, even on its last char.
pub fn word_end(t: &mut Text, pos: u64, big: bool, count: u64, stop: bool) -> Option<u64> {
    let mut w = Walk { t, pos, big };
    let mut stop = stop;
    for _ in 0..count {
        let class = w.cls();
        if w.inc() == Stepped::Edge {
            return None;
        }
        if w.cls() == class && class != 0 {
            if w.skip(class, true) {
                return None;
            }
        } else if !stop || class == 0 {
            while w.cls() == 0 {
                if w.inc() == Stepped::Edge {
                    return None;
                }
            }
            let class = w.cls();
            if w.skip(class, true) {
                return None;
            }
        }
        w.dec();
        stop = false;
    }
    Some(w.pos)
}

/// `b` and `B`, `count` times: the start of the word before, or an empty
/// line.
pub fn word_back(t: &mut Text, pos: u64, big: bool, count: u64) -> Option<u64> {
    let mut w = Walk { t, pos, big };
    'words: for _ in 0..count {
        if w.dec() == Stepped::Edge {
            return None;
        }
        while w.cls() == 0 {
            if w.on_empty_line() {
                continue 'words;
            }
            if w.dec() == Stepped::Edge {
                return Some(w.pos);
            }
        }
        let class = w.cls();
        if w.skip(class, false) {
            return Some(w.pos);
        }
        w.inc();
    }
    Some(w.pos)
}

/// `ge` and `gE`, `count` times: the end of the word before, or an empty
/// line.
pub fn word_end_back(t: &mut Text, pos: u64, big: bool, count: u64) -> Option<u64> {
    let mut w = Walk { t, pos, big };
    for _ in 0..count {
        let class = w.cls();
        if w.dec() == Stepped::Edge {
            return None;
        }
        if class != 0 {
            while w.cls() == class {
                if w.dec() == Stepped::Edge {
                    return Some(w.pos);
                }
            }
        }
        while w.cls() == 0 {
            if w.on_empty_line() {
                break;
            }
            if w.dec() == Stepped::Edge {
                return Some(w.pos);
            }
        }
    }
    Some(w.pos)
}

/// `}` and `{`: the next or previous empty line, or the end or start of
/// the buffer.
pub fn paragraph(doc: &Doc, pos: u64, forward: bool) -> u64 {
    let empty = |line: u64| doc.line_end(doc.line_start(line)) == doc.line_start(line);
    let mut line = doc.line_of(pos);
    let last = doc.last_line();
    // Past the empty lines the cursor is in, then to the next one.
    if forward {
        while line < last && empty(line) {
            line += 1;
        }
        while line < last {
            line += 1;
            if empty(line) {
                return doc.line_start(line);
            }
        }
        // The end of the last line, past its last char.
        doc.line_end(doc.line_start(last))
    } else {
        while line > 0 && empty(line) {
            line -= 1;
        }
        while line > 0 {
            line -= 1;
            if empty(line) {
                return doc.line_start(line);
            }
        }
        0
    }
}

/// `f`, `t`, `F`, and `T`: the `count`th `c` on the cursor's line.
/// `repeating` is for `;` and `,` after `t` and `T`, which skip a `c`
/// right next to the cursor.
pub fn find_char(
    t: &mut Text,
    pos: u64,
    c: char,
    kind: FindKind,
    count: u64,
    repeating: bool,
) -> Option<u64> {
    let forward = matches!(kind, FindKind::Forward | FindKind::Till);
    let till = matches!(kind, FindKind::Till | FindKind::TillBackward);
    let mut p = pos;
    // `t` finds from the char after the next, so `tx` next to an x moves
    // on only when repeated.
    if till && repeating {
        p = if forward { t.next(p)? } else { t.prev(p)? };
        if t.char_at(p) == Some('\n') {
            return None;
        }
    }
    for _ in 0..count {
        loop {
            p = if forward { t.next(p)? } else { t.prev(p)? };
            match t.char_at(p) {
                Some('\n') | None => return None,
                Some(found) if found == c => break,
                Some(_) => {}
            }
        }
    }
    if till {
        p = if forward { t.prev(p)? } else { t.next(p)? };
    }
    Some(p)
}

/// `%` without a count: the bracket pairing with the one at the cursor, or
/// with the first one after it on the line.
pub fn bracket(doc: &Doc, t: &mut Text, pos: u64) -> Option<u64> {
    let mut p = pos;
    loop {
        match t.char_at(p)? {
            '\n' => return None,
            '(' | ')' | '[' | ']' | '{' | '}' => return doc::matching_bracket(doc, p),
            _ => p = t.next(p)?,
        }
    }
}

/// The first non-blank char of the line of `pos`, or its end.
pub fn first_non_blank(doc: &Doc, pos: u64) -> u64 {
    doc::first_non_blank(doc, pos)
}

/// `g_`: the last non-blank char of the line of `pos`, or its start.
pub fn last_non_blank(t: &mut Text, pos: u64) -> u64 {
    let start = t.line_start(pos);
    let mut p = t.line_end(pos);
    while p > start {
        let prev = t.prev(p).unwrap_or(start);
        if !matches!(t.char_at(prev), Some(' ' | '\t')) {
            return prev;
        }
        p = prev;
    }
    start
}

#[cfg(test)]
mod tests {
    use super::char_class;

    #[test]
    fn classes_are_vims() {
        assert_eq!(char_class('a', false), 2);
        assert_eq!(char_class('_', false), 2);
        assert_eq!(char_class('.', false), 1);
        assert_eq!(char_class('.', true), 1);
        assert_eq!(char_class('\t', true), 0);
        assert_ne!(char_class('あ', false), char_class('漢', false));
        assert_eq!(char_class('あ', true), char_class('漢', true));
    }
}
