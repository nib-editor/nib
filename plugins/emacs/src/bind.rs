//! Emacs's commands by name, the keys that run them, and what they do: the
//! built-in keymap, `M-x`'s completions, and the names `[settings.keys.*]`
//! may use.

use base_kit::keys::{self, Binding, Keymap};
use nib_plugin::nib::plugin::types::KeyEvent;

/// Each command's name, its keys as the settings write them, and what it
/// does.
pub const COMMANDS: &[(&str, &[&str], &str)] = &[
    // Moving
    ("forward-char", &["C-f", "right"], "Move forward a char"),
    ("backward-char", &["C-b", "left"], "Move back a char"),
    ("next-line", &["C-n", "down"], "Move down a line"),
    ("previous-line", &["C-p", "up"], "Move up a line"),
    (
        "move-beginning-of-line",
        &["C-a", "home"],
        "Move to the start of the line",
    ),
    (
        "move-end-of-line",
        &["C-e", "end"],
        "Move to the end of the line",
    ),
    (
        "back-to-indentation",
        &["A-m"],
        "Move to the first char of the line that is not whitespace",
    ),
    (
        "forward-word",
        &["A-f", "C-right", "A-right"],
        "Move to the end of the next word",
    ),
    (
        "backward-word",
        &["A-b", "C-left", "A-left"],
        "Move to the start of the word before",
    ),
    (
        "forward-sentence",
        &["A-e"],
        "Move to the end of the sentence",
    ),
    (
        "backward-sentence",
        &["A-a"],
        "Move to the start of the sentence",
    ),
    (
        "forward-paragraph",
        &["A-}", "C-down"],
        "Move to the end of the paragraph",
    ),
    (
        "backward-paragraph",
        &["A-{", "C-up"],
        "Move to the start of the paragraph",
    ),
    ("forward-sexp", &["C-A-f"], "Move over the next expression"),
    ("backward-sexp", &["C-A-b"], "Move back over an expression"),
    (
        "backward-up-list",
        &["C-A-u"],
        "Move to the bracket that opens the list around",
    ),
    ("down-list", &["C-A-d"], "Move into the next list"),
    (
        "beginning-of-defun",
        &["C-A-a"],
        "Move to the start of the function",
    ),
    (
        "end-of-defun",
        &["C-A-e"],
        "Move to the end of the function",
    ),
    (
        "beginning-of-buffer",
        &["A-<", "C-home"],
        "Move to the start of the buffer, leaving the mark",
    ),
    (
        "end-of-buffer",
        &["A->", "C-end"],
        "Move to the end of the buffer, leaving the mark",
    ),
    (
        "scroll-up-command",
        &["C-v", "pagedown"],
        "Scroll a screen down",
    ),
    (
        "scroll-down-command",
        &["A-v", "pageup"],
        "Scroll a screen up",
    ),
    (
        "recenter-top-bottom",
        &["C-l"],
        "Put the line in the middle, at the top, or at the bottom",
    ),
    (
        "move-to-window-line-top-bottom",
        &["A-r"],
        "Move to the middle, top, or bottom line on screen",
    ),
    ("goto-line", &["A-g g", "A-g A-g"], "Go to a line"),
    ("goto-char", &["A-g c"], "Go to a char, counted from 1"),
    ("move-to-column", &["A-g tab"], "Move to a column"),
    // Editing
    ("self-insert-command", &[], "Insert the char typed"),
    (
        "newline",
        &["ret"],
        "Break the line, indenting the new one as the one before",
    ),
    (
        "electric-newline-and-maybe-indent",
        &["C-j"],
        "Break the line",
    ),
    ("open-line", &["C-o"], "Insert a line break after the point"),
    (
        "indent-for-tab-command",
        &["tab"],
        "Indent to the next indent point of the line above",
    ),
    (
        "indent-rigidly",
        &["C-x tab"],
        "Indent the region's lines; arrows move them",
    ),
    ("delete-char", &["C-d"], "Delete the next char"),
    (
        "delete-backward-char",
        &["backspace"],
        "Delete the char before, or the region",
    ),
    (
        "delete-forward-char",
        &["del"],
        "Delete the next char, or the region",
    ),
    ("kill-line", &["C-k"], "Kill to the end of the line"),
    ("kill-whole-line", &["C-S-backspace"], "Kill the whole line"),
    (
        "kill-word",
        &["A-d", "C-del"],
        "Kill to the end of the word",
    ),
    (
        "backward-kill-word",
        &["A-backspace", "C-backspace"],
        "Kill to the start of the word",
    ),
    ("kill-sentence", &["A-k"], "Kill to the end of the sentence"),
    ("kill-sexp", &["C-A-k"], "Kill the next expression"),
    ("zap-to-char", &["A-z"], "Kill up to and including a char"),
    ("zap-up-to-char", &[], "Kill up to a char"),
    ("kill-region", &["C-w"], "Kill the region"),
    (
        "kill-ring-save",
        &["A-w"],
        "Copy the region to the kill ring",
    ),
    ("yank", &["C-y"], "Insert the last kill"),
    (
        "yank-pop",
        &["A-y"],
        "Replace the yanked text with the kill before",
    ),
    (
        "transpose-chars",
        &["C-t"],
        "Swap the chars around the point",
    ),
    (
        "transpose-words",
        &["A-t"],
        "Swap the words around the point",
    ),
    (
        "transpose-lines",
        &["C-x C-t"],
        "Swap the line with the one above",
    ),
    ("upcase-word", &["A-u"], "Make the word upper case"),
    ("downcase-word", &["A-l"], "Make the word lower case"),
    ("capitalize-word", &["A-c"], "Capitalize the word"),
    ("upcase-region", &["C-x C-u"], "Make the region upper case"),
    (
        "downcase-region",
        &["C-x C-l"],
        "Make the region lower case",
    ),
    (
        "capitalize-region",
        &[],
        "Capitalize the words of the region",
    ),
    (
        "delete-horizontal-space",
        &["A-\\"],
        "Delete the spaces and tabs around the point",
    ),
    (
        "cycle-spacing",
        &["A-space"],
        "Leave one space around the point, then none",
    ),
    ("just-one-space", &[], "Leave one space around the point"),
    (
        "delete-indentation",
        &["A-^"],
        "Join the line to the one above",
    ),
    (
        "delete-blank-lines",
        &["C-x C-o"],
        "Leave one blank line around the point",
    ),
    (
        "delete-trailing-whitespace",
        &[],
        "Delete whitespace at the ends of lines",
    ),
    ("quoted-insert", &["C-q"], "Insert the next key as it is"),
    (
        "dabbrev-expand",
        &["A-/"],
        "Complete the word from words in the buffer",
    ),
    (
        "completion-at-point",
        &["C-A-i", "A-tab"],
        "Show completions (lsp.complete)",
    ),
    ("undo", &["C-/", "C-_", "C-7", "C-x u"], "Undo"),
    ("undo-redo", &["C-?", "C-A-_", "C-A-7"], "Redo"),
    // The mark
    (
        "set-mark-command",
        &["C-space", "C-@", "C-2"],
        "Set the mark; with C-u, go back to it",
    ),
    (
        "exchange-point-and-mark",
        &["C-x C-x"],
        "Swap the point and the mark",
    ),
    ("mark-whole-buffer", &["C-x h"], "Mark the whole buffer"),
    ("mark-paragraph", &["A-h"], "Mark the paragraph"),
    ("mark-word", &["A-@"], "Mark to the end of the word"),
    (
        "mark-sexp",
        &["C-A-space", "C-A-@"],
        "Mark the next expression",
    ),
    ("mark-defun", &["C-A-h"], "Mark the function"),
    ("rectangle-mark-mode", &["C-x space"], "Mark a rectangle"),
    (
        "keyboard-quit",
        &["C-g"],
        "Stop: drop the region, the argument, and keys typed",
    ),
    (
        "keyboard-escape-quit",
        &["A-esc esc"],
        "Stop, and close a question",
    ),
    ("universal-argument", &["C-u"], "Begin an argument"),
    (
        "digit-argument",
        &[
            "A-0", "A-1", "A-2", "A-3", "A-4", "A-5", "A-6", "A-7", "A-8", "A-9", "C-0", "C-1",
            "C-3", "C-4", "C-5", "C-6", "C-8", "C-9",
        ],
        "Add a digit to the argument",
    ),
    (
        "negative-argument",
        &["A--", "C--"],
        "Make the argument negative",
    ),
    // Searching
    ("isearch-forward", &["C-s"], "Search as you type"),
    ("isearch-backward", &["C-r"], "Search back as you type"),
    (
        "isearch-forward-regexp",
        &["C-A-s"],
        "Search for a pattern as you type",
    ),
    (
        "isearch-backward-regexp",
        &["C-A-r"],
        "Search back for a pattern as you type",
    ),
    ("query-replace", &["A-%"], "Replace text, asking at each"),
    (
        "query-replace-regexp",
        &["C-A-%"],
        "Replace a pattern, asking at each",
    ),
    ("replace-string", &[], "Replace text after the point"),
    ("replace-regexp", &[], "Replace a pattern after the point"),
    // Rectangles
    ("kill-rectangle", &["C-x r k"], "Kill the rectangle"),
    (
        "copy-rectangle-as-kill",
        &["C-x r A-w"],
        "Copy the rectangle",
    ),
    ("delete-rectangle", &["C-x r d"], "Delete the rectangle"),
    (
        "yank-rectangle",
        &["C-x r y"],
        "Insert the last killed rectangle",
    ),
    (
        "open-rectangle",
        &["C-x r o"],
        "Push the rectangle right with spaces",
    ),
    (
        "clear-rectangle",
        &["C-x r c"],
        "Replace the rectangle with spaces",
    ),
    (
        "string-rectangle",
        &["C-x r t"],
        "Replace each line of the rectangle with a text",
    ),
    (
        "rectangle-number-lines",
        &["C-x r N"],
        "Number the lines of the rectangle",
    ),
    // Registers
    (
        "copy-to-register",
        &["C-x r s", "C-x r x"],
        "Copy the region to a register",
    ),
    (
        "insert-register",
        &["C-x r i", "C-x r g"],
        "Insert a register",
    ),
    (
        "copy-rectangle-to-register",
        &["C-x r r"],
        "Copy the rectangle to a register",
    ),
    (
        "point-to-register",
        &["C-x r space", "C-x r C-space", "C-x r C-@"],
        "Keep the point in a register",
    ),
    (
        "jump-to-register",
        &["C-x r j"],
        "Go to a register's position",
    ),
    (
        "number-to-register",
        &["C-x r n"],
        "Keep a number in a register",
    ),
    (
        "increment-register",
        &["C-x r +"],
        "Add to a register's number",
    ),
    // Keyboard macros and repeating
    (
        "kmacro-start-macro",
        &["C-x (", "F3"],
        "Start recording keys",
    ),
    ("kmacro-end-macro", &["C-x )"], "Stop recording keys"),
    (
        "kmacro-end-or-call-macro",
        &["F4"],
        "Stop recording keys, or play them",
    ),
    (
        "kmacro-end-and-call-macro",
        &["C-x e"],
        "Play the recorded keys; e plays them again",
    ),
    (
        "repeat",
        &["C-x z"],
        "Do the last command again; z does it again",
    ),
    // Files, buffers, and windows
    ("save-buffer", &["C-x C-s"], "Save the buffer"),
    ("save-some-buffers", &["C-x s"], "Save the buffer"),
    (
        "write-file",
        &["C-x C-w"],
        "Save the buffer under another name",
    ),
    ("find-file", &["C-x C-f"], "Open a file"),
    ("insert-file", &["C-x i"], "Insert a file"),
    ("switch-to-buffer", &["C-x b"], "Show another open buffer"),
    (
        "next-buffer",
        &["C-x right", "C-x C-right"],
        "Show the next buffer",
    ),
    (
        "previous-buffer",
        &["C-x left", "C-x C-left"],
        "Show the buffer before",
    ),
    ("kill-buffer", &["C-x k"], "Close a buffer"),
    (
        "save-buffers-kill-terminal",
        &["C-x C-c"],
        "Quit, asking about unsaved changes",
    ),
    (
        "split-window-below",
        &["C-x 2"],
        "Split the window into two above each other",
    ),
    (
        "split-window-right",
        &["C-x 3"],
        "Split the window into two side by side",
    ),
    ("other-window", &["C-x o"], "Go to the next window"),
    ("delete-window", &["C-x 0"], "Close the window"),
    (
        "delete-other-windows",
        &["C-x 1"],
        "Close the other windows",
    ),
    (
        "what-cursor-position",
        &["C-x ="],
        "Say what the char at the point is, and where",
    ),
    (
        "xref-find-definitions",
        &["A-."],
        "Go to the definition (lsp.definition)",
    ),
    ("xref-go-back", &["A-,"], "Go back to where A-. was pressed"),
    (
        "execute-extended-command",
        &["A-x"],
        "Run a command by its name",
    ),
    ("describe-key", &["C-h k"], "Say what a key runs"),
    (
        "quit-window",
        &[],
        "Close the window, or show the buffer before",
    ),
];

/// The commands that move the point: with Shift, they select as they go.
pub const MOTIONS: &[&str] = &[
    "forward-char",
    "backward-char",
    "next-line",
    "previous-line",
    "move-beginning-of-line",
    "move-end-of-line",
    "back-to-indentation",
    "forward-word",
    "backward-word",
    "forward-sentence",
    "backward-sentence",
    "forward-paragraph",
    "backward-paragraph",
    "forward-sexp",
    "backward-sexp",
    "backward-up-list",
    "down-list",
    "beginning-of-defun",
    "end-of-defun",
    "beginning-of-buffer",
    "end-of-buffer",
    "scroll-up-command",
    "scroll-down-command",
];

pub fn is_command(name: &str) -> bool {
    COMMANDS.iter().any(|(n, _, _)| *n == name)
}

/// The keys `name` is bound to, as settings write them.
pub fn keys_of(name: &str) -> &'static [&'static str] {
    COMMANDS
        .iter()
        .find(|(n, _, _)| *n == name)
        .map_or(&[], |(_, keys, _)| *keys)
}

pub fn describe(name: &str) -> Option<&'static str> {
    COMMANDS
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|(_, _, what)| *what)
}

/// The built-in keymap: each command's keys, with prefixes as tables.
pub fn keymap() -> Keymap {
    let mut keymap = Keymap::new();
    for (name, all, _) in COMMANDS {
        for keys in *all {
            let keys = keys::parse_keys(keys).expect("the table's keys parse");
            place(&mut keymap, &keys, name);
        }
    }
    keymap
}

fn place(keymap: &mut Keymap, keys: &[KeyEvent], name: &str) {
    let (first, rest) = keys.split_first().expect("keys are never empty");
    let at = keymap.iter().position(|(key, _)| key == first);
    if rest.is_empty() {
        assert!(at.is_none(), "{name}: its keys are taken");
        keymap.push((*first, Binding::Command(name.to_string())));
        return;
    }
    let i = at.unwrap_or_else(|| {
        keymap.push((*first, Binding::Prefix(Keymap::new())));
        keymap.len() - 1
    });
    match &mut keymap[i].1 {
        Binding::Prefix(table) => place(table, rest, name),
        _ => panic!("{name}: a prefix of its keys is taken"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_parses_and_none_is_taken_twice() {
        let keymap = keymap();
        let key = |text| keys::parse_key(text).unwrap();
        assert!(matches!(
            keys::lookup(&keymap, &key("C-x")),
            Some(Binding::Prefix(_))
        ));
        for (name, _, _) in COMMANDS {
            assert!(describe(name).is_some());
        }
        for name in MOTIONS {
            assert!(is_command(name), "{name}");
        }
    }

    #[test]
    fn settings_go_over_the_built_in_keys() {
        let key = |text| keys::parse_key(text).unwrap();
        let mut keymap = keymap();
        let over = vec![
            (
                key("C-x"),
                Binding::Prefix(vec![(key("C-r"), Binding::Command("picker.files".into()))]),
            ),
            (key("C-z"), Binding::Command("undo".into())),
        ];
        keys::merge(&mut keymap, over);
        let Some(Binding::Prefix(cx)) = keys::lookup(&keymap, &key("C-x")) else {
            panic!("C-x is a table");
        };
        assert!(keys::lookup(cx, &key("C-s")).is_some());
        assert!(keys::lookup(cx, &key("C-r")).is_some());
        assert!(keys::lookup(&keymap, &key("C-z")).is_some());
    }
}
