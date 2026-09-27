//! Registers: text, rectangles, positions, and numbers kept under a char.

use base_kit::doc::Doc;
use base_kit::json_string;
use nib_plugin::nib::plugin::commands;
use nib_plugin::nib::plugin::editor::{self, View};
use nib_plugin::nib::plugin::types::{KeyCode, KeyEvent};

use crate::{Arg, Emacs};

pub enum Register {
    Text(String),
    Rect(Vec<String>),
    /// A position, kept as the core's mark in the buffer at this path.
    Position(Option<String>),
    Number(i64),
}

fn namespace(name: char) -> String {
    format!("register:{name}")
}

impl Emacs {
    /// A register command, with the name of its register typed.
    pub fn register_command(
        &mut self,
        view: &View,
        command: &str,
        arg: Arg,
        ev: KeyEvent,
    ) -> Result<(), String> {
        let name = match ev.code {
            KeyCode::Char(c) => c,
            _ => return Err("Registers are named by chars".into()),
        };
        match command {
            "copy-to-register" | "copy-rectangle-to-register" => {
                let (from, to) = self.region(view)?;
                let register = if command == "copy-to-register" {
                    Register::Text(view.buffer().slice(from, to).unwrap_or_default())
                } else {
                    let mark = self.mark(&view.buffer()).expect("the region has a mark");
                    Register::Rect(self.rectangle_text(view, mark))
                };
                self.registers.insert(name, register);
                if arg.given() {
                    if command == "copy-to-register" {
                        self.replace(view, from, to, "", from);
                    } else {
                        self.rectangle_command(view, "delete-rectangle")?;
                    }
                }
                self.deactivate = true;
                Ok(())
            }
            "insert-register" => {
                let here = self.point(view);
                let text = match self.registers.get(&name) {
                    Some(Register::Text(text)) => text.clone(),
                    Some(Register::Number(n)) => n.to_string(),
                    Some(Register::Rect(lines)) => {
                        let lines = lines.clone();
                        self.push_mark(view, here);
                        self.insert_rectangle(view, here, &lines);
                        return Ok(());
                    }
                    _ => return Err("Register does not contain text".into()),
                };
                let end = here + text.len() as u64;
                self.replace(view, here, here, &text, end);
                if arg.given() {
                    self.push_mark(view, end);
                    self.goto(view, here);
                } else {
                    self.push_mark(view, here);
                }
                Ok(())
            }
            "point-to-register" => {
                let buffer = view.buffer();
                buffer.set_marks(&namespace(name), &[self.point(view)]);
                self.registers
                    .insert(name, Register::Position(buffer.path()));
                Ok(())
            }
            "jump-to-register" => {
                let Some(Register::Position(path)) = self.registers.get(&name) else {
                    return Err(
                        "Register doesn’t contain a buffer position or configuration".into(),
                    );
                };
                if path.is_some() && *path != view.buffer().path() {
                    let path = path.clone().expect("checked above");
                    commands::call(
                        "buffer.open",
                        &format!(r#"{{"path":{}}}"#, json_string(&path)),
                    )?;
                }
                let view = editor::active_view();
                let at = view
                    .buffer()
                    .marks(&namespace(name))
                    .first()
                    .copied()
                    .ok_or("That register’s buffer no longer exists")?;
                if !self.active {
                    let here = self.point(&view);
                    self.push_mark(&view, here);
                }
                self.goto(&view, at);
                Ok(())
            }
            "number-to-register" => {
                let n = if arg.given() { arg.count() } else { 0 };
                self.registers.insert(name, Register::Number(n));
                Ok(())
            }
            "increment-register" => {
                let region = self.region(view);
                match self.registers.get_mut(&name) {
                    Some(Register::Number(n)) => {
                        *n += arg.count();
                        Ok(())
                    }
                    Some(Register::Text(text)) => {
                        let (from, to) = region?;
                        text.push_str(&Doc::new(view.buffer()).slice(from, to));
                        self.deactivate = true;
                        Ok(())
                    }
                    _ => Err("Register does not contain a number or text".into()),
                }
            }
            _ => Err(format!("{command} is not a register command")),
        }
    }
}
