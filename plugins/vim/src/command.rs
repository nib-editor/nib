//! The command line: `:` for ex commands, and `/` and `?` for searches.

use base_kit::doc::Doc;
use base_kit::{cmdline, line_edit};
use nib_plugin::exports::nib::plugin::guest::KeyResult;
use nib_plugin::nib::plugin::editor;
use nib_plugin::nib::plugin::prompt::{self as prompts, Action as PromptAction, Line};
use nib_plugin::nib::plugin::types::{Edit, KeyCode, KeyEvent};
use nib_plugin::nib::plugin::ui;
use nib_plugin::nib::plugin::view::{self, Direction, View};

use crate::ex::{self, Base, Ex};
use crate::motion::first_non_blank;
use crate::normal::{deletion, insertion, lines_span};
use crate::parse::{Action, Cmd, Motion, Target};
use crate::pattern;
use crate::register::{Shape, Value, Why};
use crate::{Mode, Vim, ctrl, plain};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Command,
    Search { backward: bool },
}

pub struct Prompting {
    kind: Kind,
    line: Line,
    /// For a search: the command waiting for its pattern, such as `d/`.
    cmd: Option<Cmd>,
    /// Walking the history: where, and what was typed before.
    history_at: Option<(usize, String)>,
    /// Tab cycling through command names: the names, and the one shown.
    completing: Option<(Vec<String>, usize)>,
    /// Ctrl-r waits for a register.
    waiting: bool,
}

/// Ex commands' names, for completion.
const NAMES: &[&str] = &[
    "write",
    "quit",
    "wq",
    "xit",
    "edit",
    "substitute",
    "global",
    "vglobal",
    "delete",
    "yank",
    "join",
    "move",
    "copy",
    "normal",
    "nohlsearch",
    "split",
    "vsplit",
    "only",
    "close",
    "bnext",
    "bprevious",
    "bdelete",
    "qall",
    "wall",
    "wqall",
    "update",
    "config",
    "config-reload",
];

impl Vim {
    pub fn open_command(&mut self, text: &str) {
        let line = Line::new(":");
        line.set(text, text.len() as u32);
        self.prompting = Some(Prompting {
            kind: Kind::Command,
            line,
            cmd: None,
            history_at: None,
            completing: None,
            waiting: false,
        });
    }

    /// `/` and `?`, alone or after an operator (`cmd`).
    pub fn open_search(&mut self, backward: bool, cmd: Option<Cmd>) {
        let label = if backward { "?" } else { "/" };
        let cmd = cmd.unwrap_or(Cmd {
            register: None,
            count: None,
            action: Action::Move(Motion::SearchPrompt { backward }),
        });
        self.prompting = Some(Prompting {
            kind: Kind::Search { backward },
            line: Line::new(label),
            cmd: Some(cmd),
            history_at: None,
            completing: None,
            waiting: false,
        });
    }

    /// A key for the prompt keys go to: vim's command-line keys, and the
    /// core's for the rest.
    pub fn prompt_key(&mut self, ev: KeyEvent) -> KeyResult {
        let Some(active) = prompts::active() else {
            return KeyResult::Pass;
        };
        let mine = active.mine && self.prompting.is_some();
        let (text, cursor) = (active.text.as_str(), active.cursor as usize);
        if mine
            && let Some(prompting) = &mut self.prompting
            && prompting.waiting
        {
            prompting.waiting = false;
            if let Some(c) = plain(&ev)
                && let Ok(Some(value)) = self.registers.get(Some(c))
            {
                let inserted = value.text.trim_end_matches('\n');
                let new = format!("{}{inserted}{}", &text[..cursor], &text[cursor..]);
                prompts::edit(&new, (cursor + inserted.len()) as u32);
            }
            return KeyResult::Handled;
        }
        let history = |vim: &mut Vim, older: bool| {
            if let Some(prompting) = &mut vim.prompting {
                let key = match prompting.kind {
                    Kind::Command => ':',
                    Kind::Search { .. } => '/',
                };
                let past = vim.history.get(&key).cloned().unwrap_or_default();
                let (at, typed) = prompting
                    .history_at
                    .clone()
                    .unwrap_or((past.len(), text.to_string()));
                let at = if older {
                    at.checked_sub(1)
                } else {
                    Some(at + 1).filter(|&a| a <= past.len())
                };
                if let Some(at) = at {
                    let shown = past.get(at).cloned().unwrap_or(typed.clone());
                    prompting.history_at = Some((at, typed));
                    prompting.line.set(&shown, shown.len() as u32);
                }
            }
        };
        let edited = match (ctrl(&ev), ev.code) {
            (Some('c'), _) => {
                prompts::act(PromptAction::Cancel);
                return KeyResult::Handled;
            }
            (Some('w'), _) => line_edit::delete_word_before(text, cursor),
            (Some('u'), _) => line_edit::delete_to_start(text, cursor),
            (Some('b'), _) => (text.to_string(), 0),
            (Some('e'), _) => (text.to_string(), text.len()),
            (Some('h'), _) if text.is_empty() => {
                prompts::act(PromptAction::Cancel);
                return KeyResult::Handled;
            }
            (Some('h'), _) => {
                let before = text[..cursor]
                    .char_indices()
                    .next_back()
                    .map_or(0, |(i, _)| i);
                (format!("{}{}", &text[..before], &text[cursor..]), before)
            }
            (Some('r'), _) if mine => {
                if let Some(prompting) = &mut self.prompting {
                    prompting.waiting = true;
                }
                return KeyResult::Handled;
            }
            (Some('p'), _) | (None, KeyCode::Up) if mine => {
                history(self, true);
                return KeyResult::Handled;
            }
            (Some('n'), _) | (None, KeyCode::Down) if mine => {
                history(self, false);
                return KeyResult::Handled;
            }
            (Some('n'), _) => {
                prompts::act(PromptAction::Next);
                return KeyResult::Handled;
            }
            (Some('p'), _) => {
                prompts::act(PromptAction::Previous);
                return KeyResult::Handled;
            }
            _ => return KeyResult::Pass,
        };
        prompts::edit(&edited.0, edited.1 as u32);
        KeyResult::Handled
    }

    /// A key for this plugin's prompt while keys are replayed, as by a
    /// macro: the core's keys are done here, as they never reach it.
    pub fn prompt_key_replayed(&mut self, ev: KeyEvent) {
        let Some(prompting) = &self.prompting else {
            return;
        };
        let id = prompting.line.id();
        let text = prompting.line.text();
        let cursor = prompting.line.cursor() as usize;
        let (text, cursor) = match (ctrl(&ev), ev.code) {
            (Some('c'), _) | (None, KeyCode::Escape) => {
                return self.prompt_action(id, PromptAction::Cancel);
            }
            (None, KeyCode::Enter) => return self.prompt_action(id, PromptAction::Accept),
            (None, KeyCode::Backspace) | (Some('h'), _) => {
                if text.is_empty() {
                    return self.prompt_action(id, PromptAction::Cancel);
                }
                let before = text[..cursor]
                    .char_indices()
                    .next_back()
                    .map_or(0, |(i, _)| i);
                (format!("{}{}", &text[..before], &text[cursor..]), before)
            }
            (Some('w'), _) => line_edit::delete_word_before(&text, cursor),
            (Some('u'), _) => line_edit::delete_to_start(&text, cursor),
            (None, KeyCode::Char(c)) => {
                let mut text = text;
                text.insert(cursor, c);
                (text, cursor + c.len_utf8())
            }
            _ => return,
        };
        if let Some(prompting) = &self.prompting {
            prompting.line.set(&text, cursor as u32);
        }
    }

    pub fn prompt_changed(&mut self, id: u64) {
        if let Some(prompting) = self.prompting.as_mut().filter(|p| p.line.id() == id) {
            prompting.completing = None;
            prompting.history_at = None;
        }
    }

    pub fn prompt_action(&mut self, id: u64, action: PromptAction) {
        let Some(prompting) = self.prompting.as_mut().filter(|p| p.line.id() == id) else {
            return;
        };
        match action {
            PromptAction::Accept => {
                let prompting = self.prompting.take().expect("checked above");
                let text = prompting.line.text();
                drop(prompting.line);
                match prompting.kind {
                    Kind::Command => {
                        self.remember(':', &text);
                        self.last_ex = Some(text.clone());
                        self.step_open = false;
                        let view = view::active();
                        let version = view.buffer().version();
                        self.run_ex_line(&text);
                        let view = view::active();
                        if self.mode == Mode::Normal && view.buffer().version() != version {
                            let pos = self.cursor(&view);
                            self.place(&view, pos);
                        }
                    }
                    Kind::Search { backward } => {
                        let pattern = if text.is_empty() {
                            match &self.search {
                                Some((pattern, _)) => pattern.clone(),
                                None => {
                                    ui::show_message("E35: No previous regular expression");
                                    return;
                                }
                            }
                        } else {
                            text
                        };
                        self.remember('/', &pattern);
                        self.search = Some((pattern.clone(), backward));
                        if let Some(mut cmd) = prompting.cmd {
                            let found = Motion::Search { backward, pattern };
                            cmd.action = match cmd.action {
                                Action::Operate(op, _) => {
                                    Action::Operate(op, Target::Motion(found))
                                }
                                _ => Action::Move(found),
                            };
                            self.run(cmd);
                        }
                    }
                }
                if self.one_command && self.mode == Mode::Normal {
                    self.back_to_insert();
                }
            }
            PromptAction::Cancel => {
                self.prompting = None;
                if self.one_command {
                    self.back_to_insert();
                }
            }
            PromptAction::Complete | PromptAction::CompleteBack
                if prompting.kind == Kind::Command =>
            {
                let back = action == PromptAction::CompleteBack;
                let (names, at) = match prompting.completing.take() {
                    Some((names, at)) => {
                        let n = names.len();
                        (names, if back { (at + n - 1) % n } else { (at + 1) % n })
                    }
                    None => {
                        let typed = prompting.line.text();
                        if typed.contains(' ') {
                            return;
                        }
                        let mut names: Vec<String> = NAMES
                            .iter()
                            .map(|n| n.to_string())
                            .chain(cmdline::candidates(&typed).into_iter().map(|(n, _)| n))
                            .filter(|n| n.starts_with(&typed))
                            .collect();
                        names.dedup();
                        if names.is_empty() {
                            return;
                        }
                        let at = if back { names.len() - 1 } else { 0 };
                        (names, at)
                    }
                };
                let name = names[at].clone();
                prompting.line.set(&name, name.len() as u32);
                prompting.completing = Some((names, at));
            }
            _ => {}
        }
    }

    /// Keeps a line in the history of `:` or `/`.
    pub fn remember(&mut self, key: char, text: &str) {
        if text.is_empty() {
            return;
        }
        let past = self.history.entry(key).or_default();
        past.retain(|t| t != text);
        past.push(text.to_string());
        if past.len() > 100 {
            past.remove(0);
        }
    }

    /// Runs an ex command line, saying what went wrong.
    pub fn run_ex_line(&mut self, line: &str) {
        if let Err(err) = self.run_ex(line) {
            ui::show_message(&err);
        }
    }

    fn run_ex(&mut self, line: &str) -> Result<(), String> {
        let ex = ex::parse(line)?;
        let arg = ex.arg.trim_end();
        let view = view::active();
        let doc = Doc::new(view.buffer());
        let cursor = self.cursor(&view);
        let current = doc.line_of(cursor);
        let given = !ex.range.is_empty();
        let whole = matches!(
            ex.name.as_str(),
            "g" | "global" | "v" | "vglobal" | "w" | "write"
        );
        let (first, last) = match ex.range.len() {
            0 if whole => (0, doc.last_line()),
            0 => (current, current),
            _ => {
                let mut lines: Vec<u64> = Vec::new();
                for a in &ex.range {
                    let from = match lines.last() {
                        Some(&previous) if a.from_previous => previous,
                        _ => current,
                    };
                    lines.push(self.address(&view, &doc, a, from)?);
                }
                let a = lines[lines.len().saturating_sub(2)];
                let b = lines[lines.len() - 1];
                (a.min(b), a.max(b))
            }
        };
        let is = |full: &str, min: usize| ex.name.len() >= min && full.starts_with(&ex.name);
        let name = ex.name.as_str();
        match name {
            "" => {
                if given {
                    self.push_jump(&view, cursor);
                    let pos = doc.line_start(last);
                    let column = self.column.unwrap_or(0);
                    let p = view
                        .move_vertically(pos, 0, Some(column))
                        .map_or(pos, |(p, _)| p);
                    let p = first_non_blank_if_blank(&doc, p);
                    self.place(&view, p);
                }
                Ok(())
            }
            "&" | "&&" => {
                let Some((pattern, replacement, flags)) = self.substitute.clone() else {
                    return Err("E35: No previous regular expression".into());
                };
                let flags = if name == "&&" || arg.starts_with('&') {
                    flags
                } else {
                    arg.to_string()
                };
                self.substitute_lines(&view, first, last, &pattern, &replacement, &flags)
            }
            _ if is("substitute", 1) => {
                let (pattern, replacement, flags) = if arg.is_empty() {
                    self.substitute
                        .clone()
                        .ok_or("E35: No previous regular expression")?
                } else {
                    let (pattern, replacement, flags) = ex::substitute_parts(arg)
                        .ok_or("E146: Regular expressions can't be delimited by letters")?;
                    let pattern = if pattern.is_empty() {
                        self.search
                            .clone()
                            .map(|(p, _)| p)
                            .ok_or("E35: No previous regular expression")?
                    } else {
                        pattern
                    };
                    (pattern, replacement.unwrap_or_default(), flags)
                };
                self.substitute = Some((pattern.clone(), replacement.clone(), flags.clone()));
                self.search = Some((pattern.clone(), false));
                self.substitute_lines(&view, first, last, &pattern, &replacement, &flags)
            }
            _ if is("delete", 1) || is("yank", 1) => {
                let (register, count) = register_and_count(arg);
                let (first, last) = match count {
                    Some(n) => (last, (last + n - 1).min(doc.last_line())),
                    None => (first, last),
                };
                let span = lines_span(&doc, doc.line_start(first), doc.line_start(last));
                let value = Value {
                    text: doc.slice(span.start, span.end),
                    shape: Shape::Lines,
                };
                if name.starts_with('d') {
                    self.registers
                        .store(register, value, Why::Delete { big: true })?;
                    self.delete(&view, span);
                    let doc = Doc::new(view.buffer());
                    let pos = self.cursor(&view);
                    self.place(&view, first_non_blank(&doc, pos));
                } else {
                    self.registers.store(register, value, Why::Yank)?;
                }
                Ok(())
            }
            _ if is("join", 1) => {
                let joins = if last > first { last - first } else { 1 };
                self.place(&view, doc.line_start(first));
                let cmd = Cmd {
                    register: None,
                    count: Some(joins + 1),
                    action: Action::Join { spaces: !ex.bang },
                };
                self.execute(&view, &cmd);
                let doc = Doc::new(view.buffer());
                self.place(&view, first_non_blank(&doc, doc.line_start(first)));
                Ok(())
            }
            ">" | "<" => {
                let times = 1 + arg.chars().filter(|&c| c.to_string() == name).count() as u64;
                self.shift_lines(&view, first, last, name == ">", times);
                Ok(())
            }
            _ if is("move", 1) || name == "t" || is("copy", 2) => {
                let to = self.target_line(&view, &doc, &ex, current)?;
                self.move_lines(&view, first, last, to, name.starts_with('m'))
            }
            _ if is("normal", 4) => {
                self.normal_on_lines(&view, first, last, given, &ex.arg);
                Ok(())
            }
            _ if is("global", 1) || is("vglobal", 1) => {
                let keep = !ex.bang && name.starts_with('g');
                self.global(&view, first, last, &ex.arg, keep)
            }
            _ if is("nohlsearch", 3) => Ok(()),
            _ if is("write", 1) || is("update", 2) => self.write(arg).map(|_| ()),
            "wq" | "x" | "xit" | "exit" | "wqa" | "wqall" | "xa" | "xall" => {
                self.write(arg)?;
                quit(ex.bang)
            }
            _ if is("quit", 1) || is("qall", 2) || name == "quitall" => quit(ex.bang),
            "wa" | "wall" => self.write("").map(|_| ()),
            _ if is("edit", 1) => {
                if arg.is_empty() {
                    return Ok(());
                }
                base_kit::open_file(arg)
            }
            _ if is("split", 2) || name == "new" || is("vsplit", 2) || name == "vnew" => {
                view::split(if name.starts_with('v') {
                    Direction::Vertical
                } else {
                    Direction::Horizontal
                });
                if !arg.is_empty() {
                    base_kit::open_file(arg)?;
                }
                Ok(())
            }
            _ if is("only", 2) => {
                view::only();
                Ok(())
            }
            _ if is("close", 3) => view::close(),
            _ if is("bnext", 2) => {
                view::active().show_next();
                Ok(())
            }
            _ if is("bprevious", 2) || is("bNext", 2) => {
                view::active().show_previous();
                Ok(())
            }
            _ if is("bdelete", 2) => view::active().buffer().close(ex.bang),
            _ if is("set", 2) => Err("nib's settings are in config.toml; :config opens it".into()),
            _ if is("registers", 3) || is("display", 2) => {
                base_kit::show_listing(
                    "*registers*",
                    &self.registers_listing(),
                    &[("q", "vim.close-listing")],
                );
                Ok(())
            }
            _ => cmdline::run(line.trim()),
        }
    }

    /// The line an address points to, from 0.
    fn address(
        &mut self,
        view: &View,
        doc: &Doc,
        address: &ex::Address,
        current: u64,
    ) -> Result<u64, String> {
        let base = match &address.base {
            Base::Line(n) => n.saturating_sub(1),
            Base::Current => current,
            Base::Last => doc.last_line(),
            Base::Mark(c) => {
                let name = match c {
                    '\'' | '`' => "'".to_string(),
                    c => c.to_string(),
                };
                let pos = view
                    .buffer()
                    .marks(&name)
                    .first()
                    .copied()
                    .ok_or("E20: Mark not set")?;
                doc.line_of(pos.min(doc.len))
            }
            Base::Pattern { pattern, backward } => {
                let pattern = if pattern.is_empty() {
                    self.search.clone().map(|(p, _)| p).unwrap_or_default()
                } else {
                    pattern.clone()
                };
                let from = if *backward {
                    doc.line_start(current)
                } else {
                    doc.line_end(doc.line_start(current))
                };
                let found = self
                    .search_from(view, from, &pattern, *backward, 1)
                    .ok_or_else(|| format!("E486: Pattern not found: {pattern}"))?;
                doc.line_of(found)
            }
        };
        let line = base as i64 + address.offset;
        if line < 0 || line > doc.last_line() as i64 + 1 {
            return Err("E16: Invalid range".into());
        }
        Ok((line as u64).min(doc.last_line()))
    }

    /// The line after which `:m` and `:t` put the lines; `None` for above
    /// the first.
    fn target_line(
        &mut self,
        view: &View,
        doc: &Doc,
        ex: &Ex,
        current: u64,
    ) -> Result<Option<u64>, String> {
        let arg = ex.arg.trim();
        if arg == "0" {
            return Ok(None);
        }
        let parsed = ex::parse(arg)?;
        let address = parsed.range.first().ok_or("E14: Invalid address")?;
        self.address(view, doc, address, current).map(Some)
    }

    fn substitute_lines(
        &mut self,
        view: &View,
        first: u64,
        last: u64,
        vim_pattern: &str,
        replacement: &str,
        flags: &str,
    ) -> Result<(), String> {
        let mut regex = pattern::translate(vim_pattern)?;
        if flags.contains('i') && !regex.starts_with("(?i)") {
            regex = format!("(?i){regex}");
        }
        let all = flags.contains('g');
        let doc = Doc::new(view.buffer());
        let start = doc.line_start(first);
        let end = doc.line_end(doc.line_start(last));
        let found = view
            .buffer()
            .find_all(&regex, start, end)
            .map_err(base_kit::error_message)?;
        let mut edits: Vec<Edit> = Vec::new();
        let mut lines = Vec::new();
        for found in found {
            let (s, e) = (found.start, found.end);
            let line = doc.line_of(s);
            if !all && lines.last() == Some(&line) {
                continue;
            }
            if edits
                .last()
                .is_some_and(|prev| prev.end > s || (prev.start == s && s == e))
            {
                continue;
            }
            let groups = if base_kit::refers_to_groups(replacement) {
                base_kit::match_groups(&view.buffer(), &regex, s)?
            } else {
                vec![doc.slice(s, e)]
            };
            edits.push(Edit {
                start: s,
                end: e,
                text: pattern::replacement(replacement, &groups)?,
            });
            if lines.last() != Some(&line) {
                lines.push(line);
            }
        }
        if edits.is_empty() {
            if flags.contains('e') {
                return Ok(());
            }
            return Err(format!("E486: Pattern not found: {vim_pattern}"));
        }
        if flags.contains('n') {
            ui::show_message(&format!("{} matches on {} lines", edits.len(), lines.len()));
            return Ok(());
        }
        let count = edits.len();
        // The cursor goes to the line where the last replacement ends.
        let shift: i64 = edits[..count - 1]
            .iter()
            .map(|e| e.text.len() as i64 - (e.end - e.start) as i64)
            .sum();
        let last = &edits[count - 1];
        let end = (last.start as i64 + shift) as u64 + last.text.len() as u64;
        self.edit(view, edits, None);
        let doc = Doc::new(view.buffer());
        let line = doc.line_of(end.min(doc.len)).min(doc.last_line());
        let pos = first_non_blank(&doc, doc.line_start(line));
        self.place(view, pos);
        if lines.len() > 2 {
            ui::show_message(&format!(
                "{count} substitution{} on {} lines",
                if count == 1 { "" } else { "s" },
                lines.len()
            ));
        }
        Ok(())
    }

    /// `:m` and `:t`: moves or copies lines `first..=last` after line `to`.
    fn move_lines(
        &mut self,
        view: &View,
        first: u64,
        last: u64,
        to: Option<u64>,
        moving: bool,
    ) -> Result<(), String> {
        let doc = Doc::new(view.buffer());
        if moving && to.is_some_and(|to| to >= first && to < last) {
            return Err("E134: Cannot move a range of lines into itself".into());
        }
        let span = lines_span(&doc, doc.line_start(first), doc.line_start(last));
        let mut text = doc.slice(span.start, span.end);
        if !text.ends_with('\n') {
            text.push('\n');
        }
        let (at, lead) = match to {
            None => (0, false),
            Some(to) => {
                let end = doc.line_end(doc.line_start(to));
                if end >= doc.len {
                    (doc.len, !doc.slice(0, doc.len).ends_with('\n'))
                } else {
                    (end + 1, false)
                }
            }
        };
        let insert = if lead {
            format!("\n{}", text.trim_end_matches('\n'))
        } else {
            text.clone()
        };
        let mut edits = vec![insertion(at, insert)];
        if moving && at != span.start && at != span.end {
            let (start, end) =
                if span.end >= doc.len && !doc.slice(span.start, span.end).ends_with('\n') {
                    (span.start.saturating_sub(1), span.end)
                } else {
                    (span.start, span.end)
                };
            edits.push(deletion(start, end));
        } else if moving {
            return Ok(());
        }
        edits.sort_by_key(|e| (e.start, e.end));
        self.edit(view, edits, None);
        let doc = Doc::new(view.buffer());
        let count = last - first + 1;
        let last_new = match to {
            None => count - 1,
            Some(to) if moving && to > last => to,
            Some(to) => to + count,
        };
        let pos = first_non_blank(&doc, doc.line_start(last_new.min(doc.last_line())));
        self.place(view, pos);
        Ok(())
    }

    /// `:normal`: runs `keys` as typed in normal mode, on each line of the
    /// range, or where the cursor is.
    fn normal_on_lines(&mut self, view: &View, first: u64, last: u64, given: bool, keys: &str) {
        let keys: Vec<KeyEvent> = keys
            .chars()
            .map(|c| KeyEvent {
                code: KeyCode::Char(c),
                modifiers: nib_plugin::nib::plugin::types::Modifiers::empty(),
            })
            .collect();
        let doc = Doc::new(view.buffer());
        let starts: Vec<u64> = if given {
            (first..=last).map(|l| doc.line_start(l)).collect()
        } else {
            vec![self.cursor(view)]
        };
        view.buffer().set_marks("normal", &starts);
        for i in 0..starts.len() {
            let view = view::active();
            let Some(&at) = view.buffer().marks("normal").get(i) else {
                break;
            };
            self.place(&view, at);
            self.replay(&keys);
            self.finish_keys();
        }
        view::active().buffer().set_marks("normal", &[]);
    }

    /// Ends what replayed keys left unfinished, as `:normal` does.
    fn finish_keys(&mut self) {
        self.keys.clear();
        if self.prompting.is_some() {
            self.prompting = None;
        }
        let escape = KeyEvent {
            code: KeyCode::Escape,
            modifiers: nib_plugin::nib::plugin::types::Modifiers::empty(),
        };
        if self.mode != Mode::Normal {
            self.replay(&[escape]);
        }
    }

    /// `:g/pat/cmd` and `:v/pat/cmd`: runs `cmd` on each line that has a
    /// match, or with `!keep`, has none.
    fn global(
        &mut self,
        view: &View,
        first: u64,
        last: u64,
        arg: &str,
        keep: bool,
    ) -> Result<(), String> {
        let (pattern, rest) = match ex::substitute_parts(arg) {
            Some((pattern, Some(rest), flags)) if flags.is_empty() => (pattern, rest),
            Some((pattern, Some(rest), flags)) => (pattern, format!("{rest}/{flags}")),
            Some((pattern, None, _)) => (pattern, String::new()),
            None => return Err("E492: Not an editor command".into()),
        };
        // The separator may be part of the command, as in `:g/a/s//b/`.
        let command = split_command(arg, &pattern).unwrap_or(rest);
        let pattern = if pattern.is_empty() {
            self.search.clone().map(|(p, _)| p).unwrap_or_default()
        } else {
            pattern
        };
        self.search = Some((pattern.clone(), false));
        let regex = pattern::translate(&pattern)?;
        let doc = Doc::new(view.buffer());
        let buffer = view.buffer();
        let found = buffer
            .find_all(
                &regex,
                doc.line_start(first),
                doc.line_end(doc.line_start(last)),
            )
            .map_err(base_kit::error_message)?;
        let mut matching: Vec<u64> = found.iter().map(|r| doc.line_of(r.start)).collect();
        matching.dedup();
        let lines: Vec<u64> = if keep {
            matching
        } else {
            (first..=last).filter(|l| !matching.contains(l)).collect()
        };
        if lines.is_empty() {
            return Err(format!("E486: Pattern not found: {pattern}"));
        }
        let starts: Vec<u64> = lines.iter().map(|&l| doc.line_start(l)).collect();
        buffer.set_marks("global", &starts);
        let command = if command.trim().is_empty() {
            "p".to_string()
        } else {
            command
        };
        let mut done = None;
        for i in 0..starts.len() {
            let view = view::active();
            let Some(&at) = view.buffer().marks("global").get(i) else {
                break;
            };
            if done == Some(at) {
                continue;
            }
            done = Some(at);
            self.place(&view, at);
            if command.trim() == "p" {
                continue;
            }
            self.run_ex(&command)?;
        }
        view::active().buffer().set_marks("global", &[]);
        Ok(())
    }

    /// Saves, under `path` if one is given.
    fn write(&mut self, path: &str) -> Result<(), String> {
        let buffer = view::active().buffer();
        buffer.save((!path.is_empty()).then_some(path))?;
        let shown = buffer.path().unwrap_or_default();
        let lines = buffer.line_count().saturating_sub(1).max(1);
        ui::show_message(&format!("\"{shown}\" {lines}L, {}B written", buffer.len()));
        Ok(())
    }

    /// `:registers`: a line each, as vim lists them.
    fn registers_listing(&self) -> String {
        let mut listing = String::from("Type Name Content\n");
        for c in "\"0123456789-abcdefghijklmnopqrstuvwxyz".chars() {
            if let Ok(Some(value)) = self.registers.get(Some(c)) {
                let shape = match value.shape {
                    Shape::Chars => 'c',
                    Shape::Lines => 'l',
                    Shape::Block => 'b',
                };
                let text = value.text.replace('\n', "^J");
                listing.push_str(&format!("  {shape}  \"{c}   {text}\n"));
            }
        }
        listing
    }
}

/// On an empty line or past its text, the first non-blank; else `pos`.
fn first_non_blank_if_blank(doc: &Doc, pos: u64) -> u64 {
    let fnb = first_non_blank(doc, pos);
    if pos < fnb { fnb } else { pos }
}

/// `:d x 3`: a register and a count after a command.
fn register_and_count(arg: &str) -> (Option<char>, Option<u64>) {
    let mut rest = arg.trim();
    let register = rest
        .chars()
        .next()
        .filter(|c| !c.is_ascii_digit())
        .inspect(|c| rest = rest[c.len_utf8()..].trim());
    (register, rest.parse().ok())
}

/// The command after `:g`'s pattern, as written.
fn split_command(arg: &str, _pattern: &str) -> Option<String> {
    let mut chars = arg.chars();
    let sep = chars.next()?;
    let body = &arg[sep.len_utf8()..];
    let mut escaped = false;
    for (i, c) in body.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' {
            escaped = true;
        } else if c == sep {
            return Some(body[i + c.len_utf8()..].to_string());
        }
    }
    None
}

fn quit(force: bool) -> Result<(), String> {
    editor::quit(force).map_err(|_| "E37: No write since last change (add ! to override)".into())
}
