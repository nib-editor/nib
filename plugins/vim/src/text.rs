//! Chars as vim's motions tell them apart.

/// vim's default 'iskeyword': letters, digits, `_`, and chars past ASCII.
pub fn is_keyword(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || (c as u32) >= 0xc0
}
