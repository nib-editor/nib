//! Test plugin: inserts typed chars at every cursor, and Ctrl with a char as
//! "^C". Escape removes its input layer.

use nib_plugin::exports::nib::plugin::guest::{Guest, KeyResult};
use nib_plugin::nib::plugin::events::Event;
use nib_plugin::nib::plugin::types::{Edit, KeyCode, KeyEvent, Modifiers, UndoMode};
use nib_plugin::nib::plugin::{input, view};

struct Insert;

impl Guest for Insert {
    fn init(_config: String) -> Result<(), String> {
        input::push_layer();
        Ok(())
    }

    fn handle_paste(_text: String) -> KeyResult {
        KeyResult::Pass
    }

    fn handle_key(ev: KeyEvent) -> KeyResult {
        let text = match ev.code {
            KeyCode::Char(c) if ev.modifiers.is_empty() => c.to_string(),
            KeyCode::Char(c) if ev.modifiers == Modifiers::CTRL => {
                format!("^{}", c.to_ascii_uppercase())
            }
            KeyCode::Escape => {
                input::pop_layer();
                return KeyResult::Handled;
            }
            _ => return KeyResult::Pass,
        };
        if insert(&text) {
            KeyResult::Handled
        } else {
            KeyResult::Pass
        }
    }

    fn run_command(name: String, _args: String) -> Result<String, String> {
        Err(format!("no command {name}"))
    }

    fn on_event(_ev: Event) {}
}

fn insert(text: &str) -> bool {
    let view = view::active();
    let edits: Vec<Edit> = view
        .selection()
        .ranges
        .iter()
        .map(|range| Edit {
            start: range.head,
            end: range.head,
            text: text.to_string(),
        })
        .collect();
    let version = view.buffer().version();
    view.apply(version, &edits, None, UndoMode::Merge).is_ok()
}

nib_plugin::export!(Insert);
