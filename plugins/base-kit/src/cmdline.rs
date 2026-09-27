//! The command line's commands, as `:` in helix and vim runs them: `:w`,
//! `:q`, `:config`, and the like.

use nib_plugin::nib::plugin::{commands, editor, ui};

use crate::json_string;

/// What running a line did, beyond its own effect.
#[derive(Debug, PartialEq, Eq)]
pub enum Ran {
    Done,
    /// Another buffer may be shown now, so the base may want to place its
    /// cursors there as it does.
    Switched,
}

/// Runs `input`, the line without its `:`.
pub fn run(input: &str) -> Result<Ran, String> {
    let (command, arg) = input.split_once(' ').unwrap_or((input, ""));
    let arg = arg.trim();
    match command {
        "w" | "write" => save().map(|()| Ran::Done),
        "q" | "quit" => quit(false),
        "q!" | "quit!" => quit(true),
        "wq" | "x" => save().and_then(|()| quit(false)),
        "bc" | "buffer-close" => close_buffer(false),
        "bc!" | "buffer-close!" => close_buffer(true),
        "config" => {
            let args = match arg {
                "" => "{}".to_string(),
                name => format!(r#"{{"plugin":{}}}"#, json_string(name)),
            };
            commands::call("config.open", &args).map(|_| Ran::Switched)
        }
        "config-reload" => commands::call("config.reload", "{}").map(|_| Ran::Done),
        "o" | "open" | "e" | "edit" if !arg.is_empty() => {
            let args = format!(r#"{{"path":{}}}"#, json_string(arg));
            commands::call("buffer.open", &args).map(|_| Ran::Switched)
        }
        "o" | "open" | "e" | "edit" => Err(format!(":{command} needs a path")),
        "" => Ok(Ran::Done),
        _ => Err(format!("unknown command: {command}")),
    }
}

fn save() -> Result<(), String> {
    commands::call("buffer.save", "{}")?;
    let path = editor::active_view().buffer().path().unwrap_or_default();
    ui::show_message(&format!("{path} written"));
    Ok(())
}

fn close_buffer(force: bool) -> Result<Ran, String> {
    commands::call("buffer.close", &format!(r#"{{"force":{force}}}"#))?;
    Ok(Ran::Switched)
}

fn quit(force: bool) -> Result<Ran, String> {
    commands::call("editor.quit", &format!(r#"{{"force":{force}}}"#))?;
    Ok(Ran::Done)
}
