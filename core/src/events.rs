//! Events plugins receive: changes to buffers, events other plugins emit,
//! and timers. They are queued while a call runs and delivered after it.

use std::time::Instant;

use ropey::Rope;

use crate::change::ChangeSet;
use crate::plugin::PluginId;
use crate::process::Stream;
use crate::prompt::Action;

/// One change of a buffer, at positions in the text as it is when the
/// changes before it in its list have been applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextChange {
    pub start: usize,
    pub end: usize,
    pub start_line: usize,
    /// In bytes from the start of the line.
    pub start_column: usize,
    pub end_line: usize,
    pub end_column: usize,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    BufferOpened(usize),
    BufferSaved(usize),
    BufferChanged {
        buffer: usize,
        version: u64,
        changes: Vec<TextChange>,
    },
    /// Emitted by a plugin; `name` starts with the plugin's name.
    Custom {
        name: String,
        data: String,
    },
    Timer(u64),
    ProcessOutput {
        process: u32,
        stream: Stream,
        data: Vec<u8>,
    },
    ProcessExit {
        process: u32,
        code: Option<i32>,
    },
    FilesListed {
        job: u32,
        paths: Vec<String>,
        done: bool,
    },
    PromptChanged {
        prompt: u32,
        text: String,
        cursor: usize,
    },
    PromptAction {
        prompt: u32,
        action: Action,
    },
}

impl Event {
    /// What plugins list in `events` in their manifest to get it.
    pub fn kind(&self) -> &str {
        match self {
            Event::BufferOpened(_) => "buffer-opened",
            Event::BufferSaved(_) => "buffer-saved",
            Event::BufferChanged { .. } => "buffer-changed",
            Event::Custom { name, .. } => name,
            Event::Timer(_) => "timer",
            Event::ProcessOutput { .. } => "process-output",
            Event::ProcessExit { .. } => "process-exit",
            Event::FilesListed { .. } => "files-listed",
            Event::PromptChanged { .. } => "prompt-changed",
            Event::PromptAction { .. } => "prompt-action",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Timer {
    pub id: u64,
    pub owner: PluginId,
    pub due: Instant,
}

/// A command a plugin registered.
#[derive(Clone, Debug)]
pub(crate) struct Command {
    pub owner: PluginId,
    /// With the plugin's name in front, as callers use it.
    pub name: String,
    /// As the plugin registered it, as its `run-command` gets it.
    pub short: String,
    pub description: String,
}

/// The changes `changesets` made to `before`, one after another, as a list
/// to apply in order.
pub(crate) fn text_changes(before: &Rope, changesets: &[ChangeSet]) -> Vec<TextChange> {
    let mut text = before.clone();
    let mut changes = Vec::new();
    for (i, set) in changesets.iter().enumerate() {
        // Back to front: a change never moves the positions before it, so
        // each is still at its place in the text before the set.
        for edit in set.edits().iter().rev() {
            let (start_line, start_column) = line_column(&text, edit.start);
            let (end_line, end_column) = line_column(&text, edit.end);
            changes.push(TextChange {
                start: edit.start,
                end: edit.end,
                start_line,
                start_column,
                end_line,
                end_column,
                text: edit.text.clone(),
            });
        }
        if i + 1 < changesets.len() {
            set.apply(&mut text);
        }
    }
    changes
}

fn line_column(text: &Rope, pos: usize) -> (usize, usize) {
    let line = text.byte_to_line(pos);
    (line, pos - text.line_to_byte(line))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Edit;

    /// Applies `changes` one after another, as a plugin would.
    fn replay(text: &str, changes: &[TextChange]) -> String {
        let mut text = text.to_string();
        for change in changes {
            let line_start: usize = text
                .split_inclusive('\n')
                .take(change.start_line)
                .map(str::len)
                .sum();
            assert_eq!(line_start + change.start_column, change.start);
            text.replace_range(change.start..change.end, &change.text);
        }
        text
    }

    #[test]
    fn changes_apply_in_order() {
        let before = Rope::from_str("one\ntwo\nthree\n");
        let first = ChangeSet::new(
            vec![
                Edit::new(0, 3, "1"),
                Edit::insert(8, "2.5\n"),
                Edit::delete(4, 7),
            ],
            &before,
        )
        .unwrap();
        let mut middle = before.clone();
        first.apply(&mut middle);
        assert_eq!(middle.to_string(), "1\n\n2.5\nthree\n");
        let second = ChangeSet::new(vec![Edit::insert(2, "two")], &middle).unwrap();
        let changes = text_changes(&before, &[first, second]);
        assert_eq!(
            (changes[0].start_line, changes[0].start_column),
            (2, 0),
            "back to front"
        );
        assert_eq!(
            replay("one\ntwo\nthree\n", &changes),
            "1\ntwo\n2.5\nthree\n"
        );
    }
}
