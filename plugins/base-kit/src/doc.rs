//! Reading a buffer around a position, and the motions computed from its
//! text. Only the lines needed are copied out of the editor.

use nib_plugin::nib::plugin::editor::Buffer;

/// Lines copied at first when looking around a position; doubled while a
/// motion runs past the copied text.
const FIRST_WINDOW_LINES: u64 = 64;

pub struct Doc {
    pub buffer: Buffer,
    pub len: u64,
}

/// A copy of whole lines, starting at byte `start` of the buffer.
pub struct Window {
    pub start: u64,
    pub text: String,
    /// The copy reaches the start or end of the buffer.
    pub at_start: bool,
    pub at_end: bool,
}

impl Window {
    pub fn end(&self) -> u64 {
        self.start + self.text.len() as u64
    }

    /// Chars from `pos` to the end of the window, with their positions.
    pub fn chars_from(&self, pos: u64) -> impl Iterator<Item = (u64, char)> + '_ {
        let from = (pos.saturating_sub(self.start) as usize).min(self.text.len());
        self.text[from..]
            .char_indices()
            .map(move |(i, c)| (self.start + (from + i) as u64, c))
    }

    /// Chars before `pos`, nearest first, with their positions.
    pub fn chars_before(&self, pos: u64) -> impl Iterator<Item = (u64, char)> + '_ {
        let to = (pos.saturating_sub(self.start) as usize).min(self.text.len());
        self.text[..to]
            .char_indices()
            .rev()
            .map(move |(i, c)| (self.start + i as u64, c))
    }

    pub fn char_at(&self, pos: u64) -> Option<char> {
        self.chars_from(pos)
            .next()
            .filter(|(p, _)| *p == pos)
            .map(|(_, c)| c)
    }
}

impl Doc {
    pub fn new(buffer: Buffer) -> Self {
        let len = buffer.len();
        Self { buffer, len }
    }

    pub fn line_of(&self, pos: u64) -> u64 {
        self.buffer.line_of(pos).unwrap_or(0)
    }

    pub fn line_count(&self) -> u64 {
        self.buffer.line_count()
    }

    pub fn line_start(&self, line: u64) -> u64 {
        self.buffer.line_start(line).unwrap_or(self.len)
    }

    /// The last line with text: a final line break does not start a new
    /// line to jump to.
    pub fn last_line(&self) -> u64 {
        let last = self.line_count().saturating_sub(1);
        if last > 0 && self.line_start(last) == self.len {
            last - 1
        } else {
            last
        }
    }

    /// The line break ending the line of `pos`, or the end of the text.
    pub fn line_end(&self, pos: u64) -> u64 {
        match self.buffer.line_start(self.line_of(pos) + 1) {
            Some(next) => next - 1,
            None => self.len,
        }
    }

    pub fn slice(&self, start: u64, end: u64) -> String {
        self.buffer.slice(start, end).unwrap_or_default()
    }

    pub fn next_grapheme(&self, pos: u64) -> u64 {
        self.buffer.next_grapheme(pos).unwrap_or(pos)
    }

    pub fn prev_grapheme(&self, pos: u64) -> u64 {
        self.buffer.prev_grapheme(pos).unwrap_or(pos)
    }

    /// Lines `first..last`, clamped to the buffer.
    pub fn lines(&self, first: u64, last: u64) -> Window {
        let start = self.line_start(first);
        let end = if last >= self.line_count() {
            self.len
        } else {
            self.line_start(last)
        };
        Window {
            start,
            text: self.slice(start, end),
            at_start: first == 0,
            at_end: end == self.len,
        }
    }

    /// Calls `f` with ever larger windows around `pos`, going forward or
    /// backward, until it returns a result. `f` returns `None` when it ran
    /// past the window without reaching the buffer's edge.
    pub fn scan<T>(
        &self,
        pos: u64,
        forward: bool,
        mut f: impl FnMut(&Window) -> Option<T>,
    ) -> Option<T> {
        let line = self.line_of(pos);
        let mut lines = FIRST_WINDOW_LINES;
        loop {
            let window = if forward {
                self.lines(line, line + lines)
            } else {
                self.lines(line.saturating_sub(lines), line + 1)
            };
            if let Some(result) = f(&window) {
                return Some(result);
            }
            if (forward && window.at_end) || (!forward && window.at_start) {
                return None;
            }
            lines *= 2;
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Category {
    LineBreak,
    Space,
    Word,
    Punctuation,
}

/// With `long`, everything but whitespace is one kind, as in Helix's WORD.
fn category(c: char, long: bool) -> Category {
    if c == '\n' {
        Category::LineBreak
    } else if c.is_whitespace() {
        Category::Space
    } else if long || c.is_alphanumeric() || c == '_' {
        Category::Word
    } else {
        Category::Punctuation
    }
}

/// `None` from a scan step: the window ended before the buffer did.
fn ran_out(window: &Window) -> Option<(u64, u64)> {
    window.at_end.then_some((window.end(), window.end()))
}

/// `w`: from the cursor to the start of the next word, including the
/// whitespace after the word. Returns the range as (anchor, head).
pub fn next_word_start(doc: &Doc, cursor: u64, long: bool) -> Option<(u64, u64)> {
    doc.scan(cursor, true, |window| {
        let mut chars = window.chars_from(cursor).peekable();
        let (_, first) = chars.next()?;
        let mut start = cursor;
        // At the end of a word, the motion selects from the next char.
        if let Some(&(pos, next)) = chars.peek()
            && category(next, long) != category(first, long)
        {
            start = pos;
        }
        let mut chars = window.chars_from(start).peekable();
        let kind = category(chars.peek()?.1, long);
        let mut end = None;
        while let Some(&(pos, c)) = chars.peek() {
            if category(c, long) != kind {
                end = Some(pos);
                break;
            }
            chars.next();
        }
        let Some(mut end) = end else {
            return ran_out(window).map(|(_, e)| (start, e));
        };
        if kind != Category::Space && kind != Category::LineBreak {
            end = window
                .chars_from(end)
                .find(|&(_, c)| category(c, long) != Category::Space)
                .map(|(pos, _)| pos)
                .or(window.at_end.then(|| window.end()))?;
        }
        Some((start, end))
    })
}

/// `e`: from the cursor to the end of the next word.
pub fn next_word_end(doc: &Doc, cursor: u64, long: bool) -> Option<(u64, u64)> {
    doc.scan(cursor, true, |window| {
        let mut chars = window.chars_from(cursor).peekable();
        let (_, first) = chars.next()?;
        let mut start = cursor;
        if let Some(&(pos, next)) = chars.peek()
            && category(next, long) != category(first, long)
        {
            start = pos;
        }
        let word_start = window
            .chars_from(start)
            .find(|&(_, c)| !matches!(category(c, long), Category::Space | Category::LineBreak))
            .map(|(pos, c)| (pos, category(c, long)));
        let Some((word_start, kind)) = word_start else {
            return ran_out(window).map(|(_, e)| (start, e));
        };
        let end = window
            .chars_from(word_start)
            .find(|&(_, c)| category(c, long) != kind)
            .map(|(pos, _)| pos);
        match end {
            Some(end) => Some((start, end)),
            None => ran_out(window).map(|(_, e)| (start, e)),
        }
    })
}

/// `b`: from the cursor back to the start of the previous word. The range is
/// backward: its head is before its anchor.
pub fn prev_word_start(doc: &Doc, cursor: u64, long: bool) -> Option<(u64, u64)> {
    doc.scan(cursor, false, |window| {
        let here = window.char_at(cursor);
        let (_, before) = window.chars_before(cursor).next()?;
        // At the start of a word, the motion leaves the cursor's char out.
        let at_word_start = here.is_none_or(|c| category(c, long) != category(before, long));
        let anchor = match here {
            Some(c) if !at_word_start => cursor + c.len_utf8() as u64,
            _ => cursor,
        };
        let mut chars = window.chars_before(anchor).peekable();
        while chars.peek().is_some_and(|&(_, c)| {
            matches!(category(c, long), Category::Space | Category::LineBreak)
        }) {
            chars.next();
        }
        let &(_, c) = chars.peek()?;
        let kind = category(c, long);
        let mut head = anchor;
        for (pos, c) in chars {
            if category(c, long) != kind {
                return Some((anchor, head));
            }
            head = pos;
        }
        window.at_start.then_some((anchor, head))
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FindKind {
    /// `f`: through the char.
    Forward,
    /// `t`: up to the char.
    Till,
    /// `F`: back through the char.
    Backward,
    /// `T`: back up to the char.
    TillBackward,
}

/// Finds `target` on the cursor's line. Returns the range as (anchor, head).
pub fn find_char(doc: &Doc, cursor: u64, target: char, kind: FindKind) -> Option<(u64, u64)> {
    let line = doc.line_of(cursor);
    let window = doc.lines(line, line + 1);
    let cursor_end = doc.next_grapheme(cursor);
    match kind {
        FindKind::Forward | FindKind::Till => {
            let (pos, c) = window
                .chars_from(cursor_end)
                .take_while(|&(_, c)| c != '\n')
                .find(|&(_, c)| c == target)?;
            let head = if kind == FindKind::Forward {
                pos + c.len_utf8() as u64
            } else {
                pos
            };
            (head > cursor_end || kind == FindKind::Forward).then_some((cursor, head))
        }
        FindKind::Backward | FindKind::TillBackward => {
            let (pos, c) = window.chars_before(cursor).find(|&(_, c)| c == target)?;
            let head = if kind == FindKind::Backward {
                pos
            } else {
                pos + c.len_utf8() as u64
            };
            Some((cursor_end, head))
        }
    }
}

/// The first non-whitespace char of the line of `pos`, or its end.
pub fn first_non_blank(doc: &Doc, pos: u64) -> u64 {
    let line = doc.line_of(pos);
    let window = doc.lines(line, line + 1);
    window
        .chars_from(window.start)
        .find(|&(_, c)| c == '\n' || !c.is_whitespace())
        .map_or(window.end(), |(pos, _)| pos)
}

/// The leading whitespace of the line of `pos`.
pub fn indentation(doc: &Doc, pos: u64) -> String {
    let line = doc.line_of(pos);
    let window = doc.lines(line, line + 1);
    window
        .text
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect()
}

const BRACKETS: [(char, char); 3] = [('(', ')'), ('[', ']'), ('{', '}')];

/// `mm`: the bracket matching the one at `pos`, counting nesting but not
/// looking inside strings or comments, which needs a syntax tree.
pub fn matching_bracket(doc: &Doc, pos: u64) -> Option<u64> {
    let window = doc.lines(doc.line_of(pos), doc.line_of(pos) + 1);
    let c = window.char_at(pos)?;
    if let Some(&(open, close)) = BRACKETS.iter().find(|(open, _)| *open == c) {
        closing(doc, pos + 1, open, close)
    } else if let Some(&(open, close)) = BRACKETS.iter().find(|(_, close)| *close == c) {
        opening(doc, pos, open, close)
    } else {
        None
    }
}

/// The `close` that ends a pair opened just before `from`.
fn closing(doc: &Doc, from: u64, open: char, close: char) -> Option<u64> {
    doc.scan(from, true, |window| {
        let mut depth = 0;
        for (pos, c) in window.chars_from(from) {
            if c == open {
                depth += 1;
            } else if c == close {
                if depth == 0 {
                    return Some(Some(pos));
                }
                depth -= 1;
            }
        }
        window.at_end.then_some(None)
    })
    .flatten()
}

/// The `open` that starts a pair closed at or after `before`.
fn opening(doc: &Doc, before: u64, open: char, close: char) -> Option<u64> {
    doc.scan(before, false, |window| {
        let mut depth = 0;
        for (pos, c) in window.chars_before(before) {
            if c == close {
                depth += 1;
            } else if c == open {
                if depth == 0 {
                    return Some(Some(pos));
                }
                depth -= 1;
            }
        }
        window.at_start.then_some(None)
    })
    .flatten()
}

/// `mi` and `ma`: the pair of `c` around `pos`, as the positions of its
/// opening and closing chars. Quotes are looked for on the line of `pos`.
pub fn surrounding_pair(doc: &Doc, pos: u64, c: char) -> Option<(u64, u64)> {
    if let Some(&(open, close)) = BRACKETS.iter().find(|(o, cl)| *o == c || *cl == c) {
        let line = doc.lines(doc.line_of(pos), doc.line_of(pos) + 1);
        let start = match line.char_at(pos) {
            Some(here) if here == open => pos,
            Some(here) if here == close => opening(doc, pos, open, close)?,
            _ => opening(doc, pos, open, close)?,
        };
        return Some((start, closing(doc, start + 1, open, close)?));
    }
    if !matches!(c, '"' | '\'' | '`') {
        return None;
    }
    let line = doc.line_of(pos);
    let window = doc.lines(line, line + 1);
    let quotes: Vec<u64> = window
        .chars_from(window.start)
        .filter(|&(_, ch)| ch == c)
        .map(|(p, _)| p)
        .collect();
    // Quotes pair up from the start of the line.
    quotes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&[open, close]| (open, close))
        .find(|&(open, close)| open <= pos && pos <= close)
}
