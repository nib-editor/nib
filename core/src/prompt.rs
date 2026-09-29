//! Prompts: lines of text a plugin opens for typing, such as a command line
//! or a picker's query. The core keeps the text and draws it; the base in
//! use decides what keys do to it, and the core's defaults cover the keys
//! it leaves (docs/plugin-api.md).

use unicode_segmentation::UnicodeSegmentation;

use crate::input::{KeyCode, KeyEvent};
use crate::plugin::PluginId;
use crate::ui::{Panel, Span, StyledLine};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Accept,
    Cancel,
    Next,
    Previous,
    PageNext,
    PagePrevious,
    Complete,
    CompleteBack,
}

#[derive(Clone, Debug)]
pub(crate) struct Prompt {
    pub id: u32,
    pub owner: PluginId,
    pub label: String,
    pub text: String,
    /// A byte offset into `text`, on a char boundary.
    pub cursor: usize,
    /// Shown after the text, such as "3/10".
    pub hint: String,
    /// Shown as the input of a box in the middle, not above the status
    /// line.
    pub boxed: Option<PromptBox>,
}

/// What the box of a prompt shows under and beside it (docs/finder.md).
#[derive(Clone, Debug, Default)]
pub(crate) struct PromptBox {
    pub title: String,
    pub rows: Vec<StyledLine>,
    pub selected: Option<usize>,
    pub preview: Preview,
}

#[derive(Clone, Debug, Default)]
pub(crate) enum Preview {
    #[default]
    None,
    Lines(Vec<StyledLine>),
    /// A file, read into the editor's `preview`, and a line to mark.
    File {
        path: std::path::PathBuf,
        line: Option<usize>,
    },
}

/// A list without a line to type into, such as completions: keys the base
/// turns into its actions go to its owner.
#[derive(Clone, Debug)]
pub(crate) struct Choices {
    pub id: u32,
    pub owner: PluginId,
    pub actions: Vec<Action>,
}

/// What a key the base left did to a prompt.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Edited,
    Asked(Action),
}

impl Prompt {
    pub fn new(id: u32, owner: PluginId, label: String) -> Self {
        Self {
            id,
            owner,
            label,
            text: String::new(),
            cursor: 0,
            hint: String::new(),
            boxed: None,
        }
    }

    /// Replaces the text, with the cursor moved back to a char boundary
    /// inside it.
    pub fn set(&mut self, text: String, cursor: usize) {
        let mut cursor = cursor.min(text.len());
        while !text.is_char_boundary(cursor) {
            cursor -= 1;
        }
        self.text = text;
        self.cursor = cursor;
    }

    /// Inserts pasted text at the cursor, its line breaks as spaces, as a
    /// prompt holds one line.
    pub fn paste(&mut self, text: &str) {
        let text = text
            .trim_end_matches(['\n', '\r'])
            .replace(['\n', '\r'], " ");
        self.text.insert_str(self.cursor, &text);
        self.cursor += text.len();
    }

    /// What the core does with a key the base left.
    pub fn default_key(&mut self, key: KeyEvent) -> Option<Outcome> {
        let plain = !key.modifiers.ctrl && !key.modifiers.alt;
        let shift = key.modifiers.shift;
        let before = self.prev_boundary();
        let after = self.next_boundary();
        match key.code {
            KeyCode::Char(c) if plain => {
                self.text.insert(self.cursor, c);
                self.cursor += c.len_utf8();
            }
            KeyCode::Backspace if self.text.is_empty() => {
                return Some(Outcome::Asked(Action::Cancel));
            }
            KeyCode::Backspace if before < self.cursor => {
                self.text.replace_range(before..self.cursor, "");
                self.cursor = before;
            }
            KeyCode::Delete if after > self.cursor => {
                self.text.replace_range(self.cursor..after, "");
            }
            KeyCode::Left => return self.move_to(before),
            KeyCode::Right => return self.move_to(after),
            KeyCode::Home => return self.move_to(0),
            KeyCode::End => return self.move_to(self.text.len()),
            KeyCode::Up => return Some(Outcome::Asked(Action::Previous)),
            KeyCode::Down => return Some(Outcome::Asked(Action::Next)),
            KeyCode::PageUp => return Some(Outcome::Asked(Action::PagePrevious)),
            KeyCode::PageDown => return Some(Outcome::Asked(Action::PageNext)),
            KeyCode::Tab if shift => return Some(Outcome::Asked(Action::CompleteBack)),
            KeyCode::Tab => return Some(Outcome::Asked(Action::Complete)),
            KeyCode::Enter => return Some(Outcome::Asked(Action::Accept)),
            KeyCode::Escape => return Some(Outcome::Asked(Action::Cancel)),
            _ => return None,
        }
        Some(Outcome::Edited)
    }

    fn move_to(&mut self, cursor: usize) -> Option<Outcome> {
        if cursor == self.cursor {
            return None;
        }
        self.cursor = cursor;
        Some(Outcome::Edited)
    }

    fn prev_boundary(&self) -> usize {
        self.text[..self.cursor]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(i, _)| i)
    }

    fn next_boundary(&self) -> usize {
        self.text[self.cursor..]
            .graphemes(true)
            .next()
            .map_or(self.cursor, |g| self.cursor + g.len())
    }

    /// As a panel of one line, with the cursor in it.
    pub fn panel(&self) -> Panel {
        let mut line = vec![Span {
            text: format!("{}{}", self.label, self.text),
            style: String::new(),
        }];
        if !self.hint.is_empty() {
            line.push(Span {
                text: format!("  {}", self.hint),
                style: "comment".into(),
            });
        }
        Panel {
            id: 0,
            owner: self.owner,
            lines: vec![line],
            cursor: Some((0, (self.label.len() + self.cursor) as u32)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(prompt: &mut Prompt, keys: &[KeyEvent]) -> Vec<Option<Outcome>> {
        keys.iter().map(|&key| prompt.default_key(key)).collect()
    }

    #[test]
    fn default_keys_edit_and_ask() {
        let mut prompt = Prompt::new(1, 0, ":".into());
        let key = |code| KeyEvent::new(code);
        press(
            &mut prompt,
            &[
                key(KeyCode::Char('a')),
                key(KeyCode::Char('é')),
                key(KeyCode::Left),
                key(KeyCode::Char('b')),
            ],
        );
        assert_eq!((prompt.text.as_str(), prompt.cursor), ("abé", 2));
        press(
            &mut prompt,
            &[key(KeyCode::Delete), key(KeyCode::Backspace)],
        );
        assert_eq!((prompt.text.as_str(), prompt.cursor), ("a", 1));
        assert_eq!(prompt.default_key(key(KeyCode::Right)), None);
        assert_eq!(
            prompt.default_key(key(KeyCode::Enter)),
            Some(Outcome::Asked(Action::Accept))
        );
        assert_eq!(
            prompt.default_key(KeyEvent::ctrl('w')),
            None,
            "the base's keys are not guessed at"
        );
        press(&mut prompt, &[key(KeyCode::Backspace)]);
        assert_eq!(
            prompt.default_key(key(KeyCode::Backspace)),
            Some(Outcome::Asked(Action::Cancel))
        );
        prompt.set("日本".into(), 4);
        assert_eq!(prompt.cursor, 3);
        assert_eq!(prompt.panel().cursor, Some((0, 4)));
    }
}
