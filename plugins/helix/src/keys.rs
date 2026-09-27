//! Key remapping from `[settings.keys.*]` in plugins/helix.toml, written as
//! in Helix's config. Helix command names stand for the keys that do the
//! same here, so a remapped key replays them.

use base_kit::keys::{self, Keymap};
use nib_plugin::nib::plugin::types::KeyEvent;
use serde_json::Value;

/// Helix command names, and the keys that run them in this keymap.
const COMMANDS: &[(&str, &str)] = &[
    ("move_char_left", "h"),
    ("move_char_right", "l"),
    ("move_visual_line_down", "j"),
    ("move_visual_line_up", "k"),
    ("move_line_down", "j"),
    ("move_line_up", "k"),
    ("move_next_word_start", "w"),
    ("move_prev_word_start", "b"),
    ("move_next_word_end", "e"),
    ("move_next_long_word_start", "W"),
    ("move_prev_long_word_start", "B"),
    ("move_next_long_word_end", "E"),
    ("find_next_char", "f"),
    ("find_till_char", "t"),
    ("find_prev_char", "F"),
    ("till_prev_char", "T"),
    ("replace", "r"),
    ("goto_file_start", "g g"),
    ("goto_last_line", "g e"),
    ("goto_line_start", "g h"),
    ("goto_line_end", "g l"),
    ("goto_first_nonwhitespace", "g s"),
    ("goto_definition", "g d"),
    ("goto_next_buffer", "g n"),
    ("goto_previous_buffer", "g p"),
    ("page_down", "C-f"),
    ("page_up", "C-b"),
    ("half_page_down", "C-d"),
    ("half_page_up", "C-u"),
    ("extend_line_below", "x"),
    ("select_all", "%"),
    ("collapse_selection", ";"),
    ("keep_primary_selection", ","),
    ("flip_selections", "A-;"),
    ("copy_selection_on_next_line", "C"),
    ("select_regex", "s"),
    ("select_mode", "v"),
    ("insert_mode", "i"),
    ("append_mode", "a"),
    ("insert_at_line_start", "I"),
    ("insert_at_line_end", "A"),
    ("open_below", "o"),
    ("open_above", "O"),
    ("yank", "y"),
    ("delete_selection", "d"),
    ("change_selection", "c"),
    ("paste_after", "p"),
    ("paste_before", "P"),
    ("indent", ">"),
    ("unindent", "<"),
    ("join_selections", "J"),
    ("undo", "u"),
    ("redo", "U"),
    ("command_mode", ":"),
    ("search", "/"),
    ("rsearch", "?"),
    ("search_next", "n"),
    ("search_prev", "N"),
    ("search_selection", "*"),
    ("match_brackets", "m m"),
    ("select_textobject_inner", "m i"),
    ("select_textobject_around", "m a"),
    ("expand_selection", "A-o"),
    ("shrink_selection", "A-i"),
    ("select_next_sibling", "A-n"),
    ("select_prev_sibling", "A-p"),
    ("goto_next_function", "] f"),
    ("goto_prev_function", "[ f"),
    ("goto_next_class", "] t"),
    ("goto_prev_class", "[ t"),
    ("goto_next_parameter", "] a"),
    ("goto_prev_parameter", "[ a"),
    ("goto_next_comment", "] c"),
    ("goto_prev_comment", "[ c"),
    ("goto_next_test", "] T"),
    ("goto_prev_test", "[ T"),
    ("file_picker", "space f"),
    ("hover", "space k"),
    ("vsplit", "C-w v"),
    ("hsplit", "C-w s"),
    ("rotate_view", "C-w w"),
    ("jump_view_left", "C-w h"),
    ("jump_view_down", "C-w j"),
    ("jump_view_up", "C-w k"),
    ("jump_view_right", "C-w l"),
    ("wclose", "C-w q"),
    ("wonly", "C-w o"),
    ("yank_to_clipboard", "space y"),
    ("paste_clipboard_after", "space p"),
    ("paste_clipboard_before", "space P"),
    ("normal_mode", "esc"),
    ("completion", "C-x"),
    ("insert_newline", "ret"),
    ("delete_char_backward", "backspace"),
    ("delete_char_forward", "del"),
    ("insert_tab", "tab"),
    ("no_op", ""),
];

#[derive(Default)]
pub struct Keymaps {
    pub normal: Keymap,
    pub insert: Keymap,
    pub select: Keymap,
}

/// The keymaps in `settings`, and what was wrong in them.
pub fn keymaps(settings: &Value) -> (Keymaps, Vec<String>) {
    let mut errors = Vec::new();
    let mut table = |mode| {
        let (keymap, wrong) = keys::keymap(settings, mode, &command_keys);
        errors.extend(wrong);
        keymap
    };
    let keymaps = Keymaps {
        normal: table("normal"),
        insert: table("insert"),
        select: table("select"),
    };
    (keymaps, errors)
}

/// The keys that run Helix command `name` here.
fn command_keys(name: &str) -> Option<Vec<KeyEvent>> {
    let (_, keys) = COMMANDS.iter().find(|(command, _)| *command == name)?;
    Some(keys::parse_keys(keys).expect("the table's keys parse"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_command_has_keys_that_parse() {
        for (name, _) in COMMANDS {
            command_keys(name).unwrap();
        }
    }

    #[test]
    fn helix_names_replay_their_keys() {
        let settings = json!({"keys": {"insert": {"j": {"k": "normal_mode"}}}});
        let (keymaps, errors) = keymaps(&settings);
        assert!(errors.is_empty());
        assert_eq!(keymaps.insert.len(), 1);
        assert!(keymaps.normal.is_empty());
    }
}
