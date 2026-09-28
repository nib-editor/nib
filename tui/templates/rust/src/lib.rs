//! {{name}}: shows how many words the shown buffer has in the status line,
//! and answers the `{{name}}.count` command with the number. Replace it with
//! what your plugin does.

use nib_plugin::exports::nib::plugin::guest::{Guest, KeyResult};
use nib_plugin::nib::plugin::events::Event;
use nib_plugin::nib::plugin::types::{KeyEvent, Span};
use nib_plugin::nib::plugin::ui::{self, Side};
use nib_plugin::nib::plugin::{commands, view};

struct Plugin;

impl Guest for Plugin {
    fn init(_config: String) -> Result<(), String> {
        commands::register("count", "The number of words in the shown buffer");
        show();
        Ok(())
    }

    fn handle_key(_key: KeyEvent) -> KeyResult {
        // Keys reach a plugin only after it pushes a layer on the input
        // stack with input::push_layer.
        KeyResult::Pass
    }

    fn handle_paste(_text: String) -> KeyResult {
        // Pasted text, as keys come: through the layers on the input stack.
        KeyResult::Pass
    }

    fn run_command(name: String, _args: String) -> Result<String, String> {
        match name.as_str() {
            "count" => Ok(count().to_string()),
            _ => Err(format!("no command {name}")),
        }
    }

    fn on_event(_event: Event) {
        show();
    }
}

fn count() -> usize {
    let buffer = view::active().buffer();
    buffer
        .slice(0, buffer.len())
        .map_or(0, |text| text.split_whitespace().count())
}

fn show() {
    let label = match count() {
        1 => "1 word".to_string(),
        words => format!("{words} words"),
    };
    let span = Span {
        text: label,
        style: String::new(),
    };
    ui::set_status("words", Side::Right, 20, &[span]);
}

nib_plugin::export!(Plugin);
