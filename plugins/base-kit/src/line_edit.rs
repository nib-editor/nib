//! Editing a prompt's text, for the keys a base gives it: a text and a byte
//! offset in, the new text and offset out. Words are runs of letters,
//! digits, and `_`.

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The start of the word before `cursor`, past the spaces and punctuation
/// in front of it.
pub fn word_left(text: &str, cursor: usize) -> usize {
    let before: Vec<(usize, char)> = text[..cursor].char_indices().collect();
    let mut i = before.len();
    while i > 0 && !is_word(before[i - 1].1) {
        i -= 1;
    }
    while i > 0 && is_word(before[i - 1].1) {
        i -= 1;
    }
    before.get(i).map_or(cursor, |&(at, _)| at)
}

/// The end of the word after `cursor`.
pub fn word_right(text: &str, cursor: usize) -> usize {
    let mut chars = text[cursor..].char_indices().peekable();
    while chars.next_if(|&(_, c)| !is_word(c)).is_some() {}
    while chars.next_if(|&(_, c)| is_word(c)).is_some() {}
    chars.peek().map_or(text.len(), |&(at, _)| cursor + at)
}

/// Deletes the word before the cursor.
pub fn delete_word_before(text: &str, cursor: usize) -> (String, usize) {
    let start = word_left(text, cursor);
    (format!("{}{}", &text[..start], &text[cursor..]), start)
}

pub fn delete_to_start(text: &str, cursor: usize) -> (String, usize) {
    (text[cursor..].to_string(), 0)
}

pub fn delete_to_end(text: &str, cursor: usize) -> (String, usize) {
    (text[..cursor].to_string(), cursor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_are_found_past_spaces_and_punctuation() {
        let text = "buffer.open path=src";
        assert_eq!(word_left(text, text.len()), 17);
        assert_eq!(word_left(text, 17), 12);
        assert_eq!(word_left(text, 6), 0);
        assert_eq!(word_right(text, 0), 6);
        assert_eq!(word_right(text, 6), 11);
        assert_eq!(word_right(text, 17), text.len());
        assert_eq!(
            delete_word_before(text, 11),
            ("buffer. path=src".to_string(), 7)
        );
        assert_eq!(delete_to_start(text, 12), ("path=src".to_string(), 0));
        assert_eq!(delete_to_end(text, 6), ("buffer".to_string(), 6));
        assert_eq!(word_left("日本 語", 10), 7);
        assert_eq!(word_left("日本 語", 7), 0);
    }
}
