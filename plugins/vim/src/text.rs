//! Walking a buffer char by char, as vim's motions do, copying only the
//! lines around the walk.

use base_kit::doc::Doc;

/// Lines copied on each side at first; doubled whenever the walk leaves
/// them.
const FIRST_SPAN: u64 = 32;

pub struct Text<'a> {
    pub doc: &'a Doc,
    start: u64,
    text: String,
    span: u64,
}

impl<'a> Text<'a> {
    pub fn new(doc: &'a Doc) -> Self {
        Self {
            doc,
            start: 0,
            text: String::new(),
            span: FIRST_SPAN / 2,
        }
    }

    fn end(&self) -> u64 {
        self.start + self.text.len() as u64
    }

    /// Copies the lines around `pos` again, more of them, unless the copy
    /// already has what is wanted.
    fn cover(&mut self, pos: u64, has: bool) {
        if has {
            return;
        }
        self.span *= 2;
        let line = self.doc.line_of(pos.min(self.doc.len));
        let window = self
            .doc
            .lines(line.saturating_sub(self.span), line + self.span + 1);
        self.start = window.start;
        self.text = window.text;
    }

    /// The char at `pos`, or `None` at the end of the buffer.
    pub fn char_at(&mut self, pos: u64) -> Option<char> {
        if pos >= self.doc.len {
            return None;
        }
        self.cover(pos, self.start <= pos && pos < self.end());
        self.text[(pos - self.start) as usize..].chars().next()
    }

    /// The position after the char at `pos`, or `None` at the end.
    pub fn next(&mut self, pos: u64) -> Option<u64> {
        let c = self.char_at(pos)?;
        Some(pos + c.len_utf8() as u64)
    }

    /// The position of the char before `pos`, or `None` at the start.
    pub fn prev(&mut self, pos: u64) -> Option<u64> {
        if pos == 0 {
            return None;
        }
        self.cover(pos, self.start < pos && pos <= self.end());
        let at = (pos - self.start) as usize;
        let (i, _) = self.text[..at].char_indices().next_back()?;
        Some(self.start + i as u64)
    }

    pub fn line_start(&self, pos: u64) -> u64 {
        self.doc.line_start(self.doc.line_of(pos))
    }

    /// The line break ending the line of `pos`, or the end of the buffer.
    pub fn line_end(&self, pos: u64) -> u64 {
        self.doc.line_end(pos)
    }
}

/// vim's default 'iskeyword': letters, digits, `_`, and chars past ASCII.
pub fn is_keyword(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || (c as u32) >= 0xc0
}
