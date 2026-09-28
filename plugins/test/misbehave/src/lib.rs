//! Test plugin that breaks on purpose, to check that the host survives it.

use nib_plugin::exports::nib::plugin::guest::{Guest, KeyResult};
use nib_plugin::nib::plugin::events::Event;
use nib_plugin::nib::plugin::types::{KeyCode, KeyEvent};
use nib_plugin::nib::plugin::{clipboard, commands, files, input, process};

struct Misbehave;

impl Guest for Misbehave {
    fn init(config: String) -> Result<(), String> {
        if config.contains("\"fail\":true") {
            return Err("asked to fail".into());
        }
        input::push_layer();
        commands::register("panic", "test: panics");
        commands::register("call-back", "test: calls test-events.echo");
        commands::register("spawn", "test: starts a program without the capability");
        commands::register("walk", "test: lists files without the capability");
        commands::register("paste", "test: reads the clipboard without the capability");
        Ok(())
    }

    fn handle_paste(_text: String) -> KeyResult {
        KeyResult::Pass
    }

    fn handle_key(ev: KeyEvent) -> KeyResult {
        match ev.code {
            KeyCode::Char('l') => loop {
                std::hint::spin_loop();
            },
            KeyCode::Char('m') => {
                let huge = vec![1u8; 1 << 30];
                std::hint::black_box(huge);
                KeyResult::Handled
            }
            KeyCode::Char('p') => panic!("asked to panic"),
            _ => KeyResult::Pass,
        }
    }

    fn run_command(name: String, _args: String) -> Result<String, String> {
        match name.as_str() {
            "panic" => panic!("asked to panic"),
            // Called from test-events, this calls it back while it waits.
            "call-back" => commands::call("test-events.echo", "back"),
            "spawn" => process::spawn("sort", &[], None).map(|_| "started".into()),
            "walk" => files::walk(None).map(|id| id.to_string()),
            "paste" => clipboard::get(),
            _ => Err(format!("no command {name}")),
        }
    }

    fn on_event(_ev: Event) {}
}

nib_plugin::export!(Misbehave);
