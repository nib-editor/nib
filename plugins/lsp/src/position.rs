//! LSP positions: a line and a character, which counts bytes with UTF-8
//! and UTF-16 code units otherwise.

use nib_plugin::nib::plugin::buffer::Buffer;

/// The line and character of `offset`.
pub fn to_lsp(buffer: &Buffer, offset: u64, utf8: bool) -> (u32, u32) {
    let line = buffer.line_of(offset).unwrap_or(0);
    let start = buffer.line_start(line).unwrap_or(0);
    let character = if utf8 {
        offset - start
    } else {
        let before = buffer.slice(start, offset).unwrap_or_default();
        before.encode_utf16().count() as u64
    };
    (line as u32, character as u32)
}

/// The offset of `line` and `character`, kept within the line and the
/// buffer.
pub fn from_lsp(buffer: &Buffer, line: u32, character: u32, utf8: bool) -> u64 {
    let Some(start) = buffer.line_start(u64::from(line)) else {
        return buffer.len();
    };
    let end = match buffer.line_start(u64::from(line) + 1) {
        Some(next) => next - 1,
        None => buffer.len(),
    };
    let text = buffer.slice(start, end).unwrap_or_default();
    start + byte_in_line(&text, character, utf8) as u64
}

/// The byte where `character` falls in `text`, on a char boundary.
fn byte_in_line(text: &str, character: u32, utf8: bool) -> usize {
    let character = character as usize;
    if utf8 {
        let mut at = character.min(text.len());
        while !text.is_char_boundary(at) {
            at -= 1;
        }
        return at;
    }
    let mut units = 0;
    for (at, c) in text.char_indices() {
        if units >= character {
            return at;
        }
        units += c.len_utf16();
    }
    text.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_bytes_or_utf16_units() {
        let text = "aé😀b";
        assert_eq!(byte_in_line(text, 3, true), 3);
        assert_eq!(byte_in_line(text, 2, true), 1, "inside é");
        assert_eq!(byte_in_line(text, 2, false), 3);
        assert_eq!(byte_in_line(text, 4, false), 7);
        assert_eq!(byte_in_line(text, 99, false), text.len());
    }
}
