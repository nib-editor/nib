//! The minibuffer: questions asked in the core's prompt line, with
//! Emacs's editing keys, completion, and history, and the commands that
//! ask them. Every key is read here, so a macro replays them the same.

use base_kit::{cmdline, json_string, line_edit, span};
use nib_plugin::nib::plugin::commands;
use nib_plugin::nib::plugin::editor::{self, View};
use nib_plugin::nib::plugin::prompt::Line;
use nib_plugin::nib::plugin::types::{KeyCode, KeyEvent, Modifiers};
use nib_plugin::nib::plugin::ui::{self, Panel};

use crate::{Arg, Emacs, Waiting, bind, ctrl, is_quit, meta, plain};

/// At most this many completions are listed.
const LISTED: usize = 30;

/// What the minibuffer asks for.
#[derive(Clone, Debug)]
pub enum Ask {
    Command(Arg),
    Number(&'static str),
    ReplaceFrom {
        regexp: bool,
        query: bool,
        region: bool,
    },
    ReplaceWith {
        from: String,
        regexp: bool,
        query: bool,
        region: bool,
    },
    FindFile,
    WriteFile,
    InsertFile,
    SwitchBuffer,
    KillBuffer,
    StringRectangle,
    /// `yes` or `no`.
    YesOrNo(Confirm),
}

/// What a question of `y`, `n`, `yes`, or `no` decides.
#[derive(Clone, Debug)]
pub enum Confirm {
    /// Close the buffer at this path though it has changes.
    KillBuffer(Option<String>),
    /// `C-x C-c`: save the buffer first?
    SaveBeforeQuit,
    /// `C-x C-c`: quit without saving?
    QuitAnyway,
}

// Paths are the host's, as the core gives them: `\` may separate their
// parts and `C:` start them on Windows, which the plugin's own `Path`
// does not know.

fn is_separator(c: char) -> bool {
    c == '/' || c == '\\'
}

fn is_absolute(path: &str) -> bool {
    path.starts_with(is_separator) || path.as_bytes().get(1) == Some(&b':')
}

/// `path` from `dir`, if it is in it.
fn relative_to(path: &str, dir: &str) -> Option<String> {
    if !is_absolute(path) {
        return Some(path.to_string());
    }
    // macOS reaches /var and /tmp through /private too.
    let plain = |p: &'static str, s: &str| s.strip_prefix(p).map_or(s.to_string(), str::to_string);
    let dir = plain("/private", dir);
    let path = plain("/private", path);
    let rest = path.strip_prefix(dir.trim_end_matches(is_separator))?;
    let rest = rest.strip_prefix(is_separator)?;
    Some(rest.to_string())
}

/// `path` from the working directory, if it is in it.
fn relative(path: &str) -> Option<String> {
    relative_to(path, &editor::working_directory())
}

/// The last part of `path`.
fn file_name(path: &str) -> &str {
    path.rsplit(is_separator).next().unwrap_or(path)
}

/// A buffer's name, as the minibuffer shows it: its path from the working
/// directory, or else its file's name, or `*scratch*` without a file.
pub fn buffer_name(path: Option<&str>) -> String {
    let Some(path) = path else {
        return "*scratch*".into();
    };
    relative(path).unwrap_or_else(|| file_name(path).to_string())
}

/// The path of the open buffer named `name`, or `name` as a path.
fn buffer_path(name: &str) -> String {
    editor::buffers()
        .into_iter()
        .filter_map(|b| b.path())
        .find(|p| buffer_name(Some(p)) == name)
        .unwrap_or_else(|| name.to_string())
}

impl Confirm {
    pub fn question(&self) -> String {
        match self {
            Confirm::KillBuffer(path) => format!(
                "Buffer {} modified; kill anyway?",
                buffer_name(path.as_deref())
            ),
            Confirm::SaveBeforeQuit => {
                let path = editor::active_view().buffer().path();
                format!("Save file {}?", path.as_deref().unwrap_or("*scratch*"))
            }
            Confirm::QuitAnyway => "Modified buffers exist; exit anyway?".into(),
        }
    }
}

pub struct Asking {
    pub ask: Ask,
    line: Line,
    /// Used when nothing is typed.
    default: Option<String>,
    /// Walking the history: where, and what was typed before.
    history_at: Option<(usize, String)>,
    completions: Option<Panel>,
    history_key: &'static str,
    /// `C-q`: the next key goes in as it is.
    quoted: bool,
}

/// `ESC ESC` in a question: gives up, as `ESC ESC ESC` does.
pub fn escape_quit() -> KeyEvent {
    KeyEvent {
        code: KeyCode::Escape,
        modifiers: Modifiers::ALT,
    }
}

fn history_key(ask: &Ask) -> &'static str {
    match ask {
        Ask::Command(_) => "command",
        Ask::Number(_) => "number",
        Ask::ReplaceFrom { .. } | Ask::ReplaceWith { .. } => "replace",
        Ask::FindFile | Ask::WriteFile | Ask::InsertFile => "file",
        Ask::SwitchBuffer | Ask::KillBuffer => "buffer",
        Ask::StringRectangle => "rectangle",
        Ask::YesOrNo(_) => "yes-or-no",
    }
}

/// What a minibuffer editing key does to `text` with the cursor at
/// `cursor`: the new text and cursor, or `None` for other keys. `yank` is
/// the newest kill, for `C-y`.
pub fn edit_key(
    text: &str,
    cursor: usize,
    ev: KeyEvent,
    yank: Option<&str>,
) -> Option<(String, usize)> {
    let prev = |at: usize| text[..at].char_indices().next_back().map_or(0, |(i, _)| i);
    let next = |at: usize| text[at..].chars().next().map_or(at, |c| at + c.len_utf8());
    let plain_code = ev.modifiers.is_empty();
    Some(match (ctrl(&ev), meta(&ev), ev.code) {
        (Some('a'), ..) => (text.to_string(), 0),
        (Some('e'), ..) => (text.to_string(), text.len()),
        (Some('b'), ..) => (text.to_string(), prev(cursor)),
        (Some('f'), ..) => (text.to_string(), next(cursor)),
        (Some('d'), ..) => {
            let after = next(cursor);
            (format!("{}{}", &text[..cursor], &text[after..]), cursor)
        }
        (Some('k'), ..) => line_edit::delete_to_end(text, cursor),
        (Some('y'), ..) => {
            let yank = yank?;
            (
                format!("{}{yank}{}", &text[..cursor], &text[cursor..]),
                cursor + yank.len(),
            )
        }
        (_, Some('b'), _) => (text.to_string(), line_edit::word_left(text, cursor)),
        (_, Some('f'), _) => (text.to_string(), line_edit::word_right(text, cursor)),
        (_, Some('d'), _) => {
            let end = line_edit::word_right(text, cursor);
            (format!("{}{}", &text[..cursor], &text[end..]), cursor)
        }
        (_, None, KeyCode::Backspace) if ev.modifiers == Modifiers::ALT => {
            line_edit::delete_word_before(text, cursor)
        }
        (_, _, KeyCode::Backspace) if plain_code => {
            let before = prev(cursor);
            (format!("{}{}", &text[..before], &text[cursor..]), before)
        }
        (_, _, KeyCode::Delete) if plain_code => {
            let after = next(cursor);
            (format!("{}{}", &text[..cursor], &text[after..]), cursor)
        }
        (_, _, KeyCode::Left) if plain_code => (text.to_string(), prev(cursor)),
        (_, _, KeyCode::Right) if plain_code => (text.to_string(), next(cursor)),
        (_, _, KeyCode::Home) if plain_code => (text.to_string(), 0),
        (_, _, KeyCode::End) if plain_code => (text.to_string(), text.len()),
        _ => return None,
    })
}

impl Emacs {
    /// Opens the minibuffer for `ask`, with `text` typed already.
    pub fn ask(&mut self, ask: Ask, label: &str, text: &str) {
        let line = Line::new(label);
        line.set(text, text.len() as u32);
        let history_key = history_key(&ask);
        self.asking = Some(Asking {
            ask,
            line,
            default: None,
            history_at: None,
            completions: None,
            history_key,
            quoted: false,
        });
    }

    fn ask_with_default(&mut self, ask: Ask, label: &str, default: Option<String>) {
        let label = match &default {
            Some(default) => format!("{label} (default {default}): "),
            None => format!("{label}: "),
        };
        self.ask(ask, &label, "");
        if let Some(asking) = self.asking.as_mut() {
            asking.default = default;
        }
    }

    pub fn ask_command(&mut self, arg: Arg) {
        let label = match arg {
            Arg::None => "M-x ".to_string(),
            arg => format!("{} M-x ", arg_label(arg)),
        };
        self.ask(Ask::Command(arg), &label, "");
    }

    pub fn ask_number(&mut self, name: &str, _arg: Arg) {
        let label = match name {
            "goto-line" => "Goto line: ",
            "goto-char" => "Goto char: ",
            _ => "Move to column: ",
        };
        let name = bind::COMMANDS
            .iter()
            .map(|(n, _, _)| *n)
            .find(|n| *n == name)
            .unwrap_or("goto-line");
        self.ask(Ask::Number(name), label, "");
    }

    pub fn remember(&mut self, key: &'static str, text: &str) {
        let list = self.history.entry(key).or_default();
        list.retain(|t| t != text);
        list.push(text.to_string());
    }

    /// A key while the minibuffer is open.
    pub fn asking_key(&mut self, ev: KeyEvent) {
        let Some(asking) = self.asking.as_mut() else {
            return;
        };
        if is_quit(&ev) || ev == escape_quit() {
            self.asking = None;
            self.failed = true;
            ui::show_message("Quit");
            return;
        }
        let text = asking.line.text();
        let cursor = asking.line.cursor() as usize;
        if std::mem::take(&mut asking.quoted) {
            if let Some(c) = crate::command::quoted_char(ev) {
                let mut text = text;
                text.insert(cursor, c);
                asking.line.set(&text, (cursor + c.len_utf8()) as u32);
            }
            return;
        }
        match (ctrl(&ev), meta(&ev), ev.code) {
            (_, _, KeyCode::Enter) if ev.modifiers.is_empty() => return self.accept(),
            (Some('j'), ..) => return self.accept(),
            (_, _, KeyCode::Tab) if ev.modifiers.is_empty() => return self.complete(),
            (_, Some('p'), _) | (_, _, KeyCode::Up) => return self.walk_history(-1),
            (_, Some('n'), _) | (_, _, KeyCode::Down) => return self.walk_history(1),
            (Some('q'), ..) => {
                asking.quoted = true;
                return;
            }
            _ => {}
        }
        let yank = self.kills.first().map(|kill| kill.text.clone());
        let edited = edit_key(&text, cursor, ev, yank.as_deref()).or_else(|| {
            let c = plain(&ev)?;
            let mut text = text.clone();
            text.insert(cursor, c);
            Some((text, cursor + c.len_utf8()))
        });
        let asking = self.asking.as_mut().expect("asking");
        if let Some((text, cursor)) = edited {
            asking.line.set(&text, cursor as u32);
            asking.history_at = None;
            asking.completions = None;
        }
    }

    fn walk_history(&mut self, step: i64) {
        let Some(asking) = self.asking.as_mut() else {
            return;
        };
        let list = self
            .history
            .get(asking.history_key)
            .cloned()
            .unwrap_or_default();
        let (at, typed) = asking
            .history_at
            .clone()
            .unwrap_or((list.len(), asking.line.text()));
        let next = at as i64 + step;
        if next < 0 {
            ui::show_message("Beginning of history; no preceding item");
            return;
        }
        let next = next as usize;
        let text = if next >= list.len() {
            if at >= list.len() {
                ui::show_message("End of history; no default available");
                return;
            }
            typed.clone()
        } else {
            list[next].clone()
        };
        asking.line.set(&text, text.len() as u32);
        asking.history_at = Some((next, typed));
    }

    /// `TAB`: completes as far as the candidates agree, and lists them
    /// when it cannot go further.
    fn complete(&mut self) {
        let Some(asking) = self.asking.as_ref() else {
            return;
        };
        let text = asking.line.text();
        let candidates = candidates(&asking.ask, &text);
        let asking = self.asking.as_mut().expect("asking");
        if candidates.is_empty() {
            ui::show_message("[No match]");
            return;
        }
        let names: Vec<&str> = candidates.iter().map(|(name, _)| name.as_str()).collect();
        let common = common_prefix(&names);
        if common.len() > text.len() {
            asking.line.set(&common, common.len() as u32);
            asking.completions = None;
            if names.len() == 1 {
                ui::show_message("[Sole completion]");
            }
            return;
        }
        if names.len() == 1 {
            ui::show_message("[Sole completion]");
            return;
        }
        let width = names.iter().map(|n| n.len()).max().unwrap_or(0).min(40);
        let mut lines: Vec<Vec<_>> = vec![vec![span(
            &format!("{} possible completions:", candidates.len()),
            "ui.popup.title",
        )]];
        lines.extend(candidates.iter().take(LISTED).map(|(name, what)| {
            vec![
                span(&format!("{name:width$}"), "ui.popup.key"),
                span(&format!("  {what}"), ""),
            ]
        }));
        asking.completions = Some(Panel::new(&lines));
    }

    fn accept(&mut self) {
        let Some(asking) = self.asking.take() else {
            return;
        };
        let typed = asking.line.text();
        let text = match (&asking.default, typed.is_empty()) {
            (Some(default), true) => default.clone(),
            _ => typed,
        };
        let key = asking.history_key;
        drop(asking.line);
        if !text.is_empty() && !matches!(asking.ask, Ask::YesOrNo(_)) {
            self.remember(key, &text);
        }
        let view = editor::active_view();
        let ask = asking.ask;
        if let Ask::Command(arg) = ask {
            return self.run_named(&text, arg);
        }
        // The command that asked goes on here.
        self.step_open = false;
        self.merge = false;
        self.deactivate = false;
        let result = self.answer(&view, ask, text);
        if let Err(err) = result {
            self.failed = true;
            ui::show_message(&err);
        }
        if self.deactivate {
            self.active = false;
            self.rectangle = false;
            self.shift_selected = false;
        }
        if self.isearch.is_none() && self.query.is_none() {
            self.render();
        }
    }

    /// `M-x`: an Emacs command by its name, or nib's by its dotted name
    /// with arguments after it.
    fn run_named(&mut self, text: &str, arg: Arg) {
        let (name, rest) = text.split_once(' ').unwrap_or((text, ""));
        let key = KeyEvent {
            code: KeyCode::Enter,
            modifiers: Modifiers::empty(),
        };
        if bind::is_command(name) {
            if matches!(
                name,
                "universal-argument" | "digit-argument" | "negative-argument"
            ) {
                return ui::show_message(&format!("{name} needs a key"));
            }
            let name = bind::COMMANDS
                .iter()
                .map(|(n, _, _)| *n)
                .find(|n| *n == name)
                .expect("checked above");
            self.run_with(name, arg, key);
            return;
        }
        let result = if name.contains('.') {
            cmdline::args(rest).and_then(|args| commands::call(name, &args).map(|_| ()))
        } else {
            Err(format!("[No match] {name}"))
        };
        if let Err(err) = result {
            self.failed = true;
            ui::show_message(&err);
        }
        self.render();
    }

    fn answer(&mut self, view: &View, ask: Ask, text: String) -> Result<(), String> {
        match ask {
            Ask::Command(_) => unreachable!("handled before"),
            Ask::Number(name) => {
                let n = text
                    .trim()
                    .parse::<i64>()
                    .map_err(|_| format!("Not a number: {text}"))?;
                self.go(view, name, n)
            }
            Ask::ReplaceFrom {
                regexp,
                query,
                region,
            } => {
                if text.is_empty() {
                    let (from, to) = self.last_replace.clone().ok_or("Nothing to replace")?;
                    return self.start_replace(view, from, to, regexp, query, region);
                }
                if regexp {
                    crate::search::translate(&text)?;
                }
                let what = match (query, regexp) {
                    (true, false) => "Query replace",
                    (true, true) => "Query replace regexp",
                    (false, false) => "Replace string",
                    (false, true) => "Replace regexp",
                };
                let region_label = if region { " in region" } else { "" };
                self.ask(
                    Ask::ReplaceWith {
                        from: text.clone(),
                        regexp,
                        query,
                        region,
                    },
                    &format!("{what}{region_label} {text} with: "),
                    "",
                );
                Ok(())
            }
            Ask::ReplaceWith {
                from,
                regexp,
                query,
                region,
            } => self.start_replace(view, from, text, regexp, query, region),
            Ask::FindFile => {
                let path = expand_home(&text);
                let args = format!(r#"{{"path":{}}}"#, json_string(&path));
                commands::call("buffer.open", &args).map(|_| ())
            }
            Ask::WriteFile => {
                let path = expand_home(&text);
                let args = format!(r#"{{"path":{}}}"#, json_string(&path));
                commands::call("buffer.save", &args)?;
                ui::show_message(&format!("Wrote {path}"));
                Ok(())
            }
            Ask::InsertFile => {
                let path = expand_home(&text);
                let inserted =
                    std::fs::read_to_string(&path).map_err(|err| format!("{path}: {err}"))?;
                let here = self.point(view);
                let end = here + inserted.len() as u64;
                self.replace(view, here, here, &inserted, here);
                self.push_mark(view, end);
                Ok(())
            }
            Ask::SwitchBuffer => {
                if text.is_empty() {
                    return Ok(());
                }
                let args = format!(r#"{{"path":{}}}"#, json_string(&buffer_path(&text)));
                commands::call("buffer.open", &args).map(|_| ())
            }
            Ask::KillBuffer => {
                let current = buffer_name(view.buffer().path().as_deref());
                if !text.is_empty() && current != text {
                    let args = format!(r#"{{"path":{}}}"#, json_string(&buffer_path(&text)));
                    commands::call("buffer.open", &args)?;
                }
                let path = editor::active_view().buffer().path();
                if commands::call("buffer.close", r#"{"force":false}"#).is_err() {
                    let confirm = Confirm::KillBuffer(path);
                    let question = format!("{} (yes or no) ", confirm.question());
                    self.ask(Ask::YesOrNo(confirm), &question, "");
                }
                Ok(())
            }
            Ask::StringRectangle => self.string_rectangle(view, &text),
            Ask::YesOrNo(confirm) => match text.trim() {
                "yes" => {
                    self.confirmed(confirm, true);
                    Ok(())
                }
                "no" => {
                    self.confirmed(confirm, false);
                    Ok(())
                }
                _ => {
                    let question = format!(
                        "Please answer yes or no.  {} (yes or no) ",
                        confirm.question()
                    );
                    self.ask(Ask::YesOrNo(confirm), &question, "");
                    Ok(())
                }
            },
        }
    }

    /// What was answered to a question of `y` or `n`, or `yes` or `no`.
    pub fn confirmed(&mut self, confirm: Confirm, yes: bool) {
        let result = match (confirm, yes) {
            (Confirm::KillBuffer(_), true) => {
                commands::call("buffer.close", r#"{"force":true}"#).map(|_| ())
            }
            (Confirm::SaveBeforeQuit, true) => commands::call("buffer.save", "{}")
                .and_then(|_| commands::call("editor.quit", r#"{"force":false}"#))
                .map(|_| ())
                .or_else(|_| {
                    self.ask_yes_or_no(Confirm::QuitAnyway);
                    Ok(())
                }),
            (Confirm::SaveBeforeQuit, false) => {
                self.ask_yes_or_no(Confirm::QuitAnyway);
                Ok(())
            }
            (Confirm::QuitAnyway, true) => {
                commands::call("editor.quit", r#"{"force":true}"#).map(|_| ())
            }
            (_, false) => Ok(()),
        };
        if let Err(err) = result {
            ui::show_message(&err);
        }
        self.render();
    }

    fn ask_yes_or_no(&mut self, confirm: Confirm) {
        let question = format!("{} (yes or no) ", confirm.question());
        self.ask(Ask::YesOrNo(confirm), &question, "");
    }

    /// Commands for files, buffers, and windows.
    pub fn run_file_command(&mut self, view: &View, name: &str, _arg: Arg) -> Result<(), String> {
        let full = view.buffer().path();
        let path = full.as_deref().map(|p| buffer_name(Some(p)));
        let here = full
            .as_deref()
            .map(|p| relative(p).unwrap_or_else(|| p.to_string()));
        match name {
            "save-buffer" | "save-some-buffers" => {
                commands::call("buffer.save", "{}")?;
                let path = editor::active_view().buffer().path().unwrap_or_default();
                ui::show_message(&format!("Wrote {path}"));
                Ok(())
            }
            "write-file" => {
                self.ask(
                    Ask::WriteFile,
                    "Write file: ",
                    &directory_of(here.as_deref()),
                );
                Ok(())
            }
            "find-file" => {
                self.ask(Ask::FindFile, "Find file: ", &directory_of(here.as_deref()));
                Ok(())
            }
            "insert-file" => {
                self.ask(
                    Ask::InsertFile,
                    "Insert file: ",
                    &directory_of(here.as_deref()),
                );
                Ok(())
            }
            "switch-to-buffer" => {
                let other = editor::buffers()
                    .into_iter()
                    .filter_map(|b| b.path())
                    .map(|p| buffer_name(Some(&p)))
                    .find(|p| Some(p) != path.as_ref());
                self.ask_with_default(Ask::SwitchBuffer, "Switch to buffer", other);
                Ok(())
            }
            "kill-buffer" => {
                self.ask_with_default(Ask::KillBuffer, "Kill buffer", path);
                Ok(())
            }
            "next-buffer" => commands::call("buffer.next", "").map(|_| ()),
            "previous-buffer" => commands::call("buffer.previous", "").map(|_| ()),
            "save-buffers-kill-terminal" => {
                if commands::call("editor.quit", r#"{"force":false}"#).is_err() {
                    let confirm = Confirm::SaveBeforeQuit;
                    ui::show_message(&format!("{} (y or n) ", confirm.question()));
                    self.waiting = Some(Waiting::YesOrNo(confirm));
                }
                Ok(())
            }
            "split-window-below" => {
                commands::call("view.split", r#"{"direction":"horizontal"}"#).map(|_| ())
            }
            "split-window-right" => {
                commands::call("view.split", r#"{"direction":"vertical"}"#).map(|_| ())
            }
            "other-window" => commands::call("view.focus", r#"{"to":"next"}"#).map(|_| ()),
            "delete-window" => commands::call("view.close", "").map(|_| ()),
            "delete-other-windows" => commands::call("view.only", "").map(|_| ()),
            "string-rectangle" => {
                self.region(view)?;
                self.ask(Ask::StringRectangle, "String rectangle: ", "");
                Ok(())
            }
            _ => Err(format!("{name} is not a command here")),
        }
    }
}

fn arg_label(arg: Arg) -> String {
    match arg {
        Arg::Universal(1) => "C-u".into(),
        Arg::Universal(k) => format!("{}", 4i64.pow(k)),
        Arg::Minus => "-".into(),
        Arg::Number(n) => n.to_string(),
        Arg::None => String::new(),
    }
}

/// The directory of `path` with a `/` after it, as `C-x C-f` starts with,
/// or nothing for the working directory.
fn directory_of(path: Option<&str>) -> String {
    match path.and_then(|p| p.rfind(is_separator)) {
        Some(i) => path.expect("found in it")[..=i].to_string(),
        None => String::new(),
    }
}

fn expand_home(path: &str) -> String {
    match (path.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(home)) => format!("{home}/{rest}"),
        _ => path.to_string(),
    }
}

/// What `text` may complete to, for what is asked, with what each is.
fn candidates(ask: &Ask, text: &str) -> Vec<(String, String)> {
    let mut all: Vec<(String, String)> = match ask {
        Ask::Command(_) => {
            let mut all: Vec<(String, String)> = bind::COMMANDS
                .iter()
                .map(|(name, _, what)| (name.to_string(), what.to_string()))
                .collect();
            let mut nib = commands::all();
            nib.sort();
            all.extend(nib);
            all
        }
        Ask::FindFile | Ask::WriteFile | Ask::InsertFile => files(text),
        Ask::SwitchBuffer | Ask::KillBuffer => editor::buffers()
            .into_iter()
            .filter_map(|b| b.path())
            .map(|p| (buffer_name(Some(&p)), String::new()))
            .collect(),
        _ => Vec::new(),
    };
    all.retain(|(name, _)| name.starts_with(text));
    all
}

/// The files and directories that start with `text`'s last part, in its
/// directory.
fn files(text: &str) -> Vec<(String, String)> {
    let (dir, _) = match text.rfind(is_separator) {
        Some(i) => (&text[..=i], &text[i + 1..]),
        None => ("", text),
    };
    let read = if dir.is_empty() { "." } else { dir };
    let Ok(entries) = std::fs::read_dir(expand_home(read)) else {
        return Vec::new();
    };
    let mut found: Vec<(String, String)> = entries
        .filter_map(Result::ok)
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
            let slash = if is_dir { "/" } else { "" };
            (format!("{dir}{name}{slash}"), String::new())
        })
        .collect();
    found.sort();
    found
}

fn common_prefix(names: &[&str]) -> String {
    let Some(first) = names.first() else {
        return String::new();
    };
    let mut end = first.len();
    for name in &names[1..] {
        end = first
            .char_indices()
            .zip(name.chars())
            .take_while(|((_, a), b)| a == b)
            .last()
            .map_or(0, |((i, a), _)| i + a.len_utf8())
            .min(end);
    }
    first[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use base_kit::keys::parse_key;

    #[test]
    fn minibuffer_keys_edit_as_emacs_does() {
        let key = |text| parse_key(text).unwrap();
        let text = "find file";
        assert_eq!(edit_key(text, 9, key("C-a"), None), Some((text.into(), 0)));
        assert_eq!(edit_key(text, 0, key("A-f"), None), Some((text.into(), 4)));
        assert_eq!(
            edit_key(text, 4, key("C-k"), None),
            Some(("find".into(), 4))
        );
        assert_eq!(
            edit_key(text, 9, key("A-backspace"), None),
            Some(("find ".into(), 5))
        );
        assert_eq!(
            edit_key(text, 4, key("C-y"), Some("-x")),
            Some(("find-x file".into(), 6))
        );
        assert_eq!(edit_key(text, 4, key("x"), None), None);
    }

    #[test]
    fn completion_goes_as_far_as_the_candidates_agree() {
        assert_eq!(
            common_prefix(&["kill-line", "kill-word", "kill-region"]),
            "kill-"
        );
        assert_eq!(common_prefix(&["yank"]), "yank");
        assert_eq!(common_prefix(&["ab", "cd"]), "");
        assert_eq!(directory_of(Some("src/main.rs")), "src/");
        assert_eq!(directory_of(Some("main.rs")), "");
        assert_eq!(directory_of(Some(r"src\main.rs")), r"src\");
    }

    #[test]
    fn buffers_are_named_from_the_working_directory() {
        assert_eq!(
            relative_to("/w/src/a.rs", "/w").as_deref(),
            Some("src/a.rs")
        );
        assert_eq!(
            relative_to("/private/var/a.rs", "/var/").as_deref(),
            Some("a.rs")
        );
        assert_eq!(relative_to(r"D:\w\a.rs", r"D:\w").as_deref(), Some("a.rs"));
        assert_eq!(relative_to("src/a.rs", "/w").as_deref(), Some("src/a.rs"));
        assert_eq!(relative_to("/other/a.rs", "/w"), None);
        assert_eq!(relative_to("/wx/a.rs", "/w"), None);
        assert_eq!(file_name(r"D:\other\a.rs"), "a.rs");
        assert_eq!(file_name("/other/a.rs"), "a.rs");
    }
}
