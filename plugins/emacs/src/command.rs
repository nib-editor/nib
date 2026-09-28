//! Emacs's commands by name: moving, editing, killing and yanking, the
//! mark, undo, and the commands that only call others.

use base_kit::doc::{self, Doc};
use base_kit::edit::{deletion, indent_unit, insertion};
use base_kit::text::Text;
use base_kit::{call_or_show, tree};
use nib_plugin::nib::plugin::types::{Edit, KeyCode, KeyEvent, Modifiers};
use nib_plugin::nib::plugin::view::{ScrollAmount, View};
use nib_plugin::nib::plugin::{commands, ui};

use crate::motion::{self, is_word};
use crate::{Arg, Dabbrev, Emacs, Kill, Last, Spacing, Waiting};

impl Emacs {
    pub fn run(&mut self, view: &View, name: &str, arg: Arg, key: KeyEvent) -> Result<(), String> {
        let n = arg.count();
        match name {
            "forward-char" => self.chars(view, n),
            "backward-char" => self.chars(view, -n),
            "next-line" => self.lines(view, n),
            "previous-line" => self.lines(view, -n),
            "move-beginning-of-line" | "move-end-of-line" => {
                let doc = Doc::new(view.buffer());
                let line = doc.line_of(self.point(view)) as i64 + n - 1;
                let line = line.clamp(0, doc.line_count() as i64 - 1) as u64;
                let start = doc.line_start(line);
                let at = if name == "move-beginning-of-line" {
                    start
                } else {
                    doc.line_end(start)
                };
                self.goto(view, at);
                Ok(())
            }
            "back-to-indentation" => {
                let doc = Doc::new(view.buffer());
                let at = doc::first_non_blank(&doc, self.point(view));
                self.goto(view, at);
                Ok(())
            }
            "forward-word" | "backward-word" => {
                let doc = Doc::new(view.buffer());
                let mut t = Text::new(&doc);
                let n = if name == "forward-word" { n } else { -n };
                let at = motion::words(&mut t, self.point(view), n);
                self.goto(view, at);
                Ok(())
            }
            "forward-sentence" | "backward-sentence" => {
                let n = if name == "forward-sentence" { n } else { -n };
                let at = self.sentences(view, n);
                self.goto(view, at);
                Ok(())
            }
            "forward-paragraph" | "backward-paragraph" => {
                let n = if name == "forward-paragraph" { n } else { -n };
                let at = paragraphs(&Doc::new(view.buffer()), self.point(view), n);
                self.goto(view, at);
                Ok(())
            }
            "forward-sexp" | "backward-sexp" => {
                let n = if name == "forward-sexp" { n } else { -n };
                let doc = Doc::new(view.buffer());
                let (at, result) = sexps(&doc, self.point(view), n);
                self.goto(view, at);
                result
            }
            "backward-up-list" | "down-list" => {
                let doc = Doc::new(view.buffer());
                let mut t = Text::new(&doc);
                let mut at = self.point(view);
                for _ in 0..n.max(1) {
                    at = if name == "down-list" {
                        motion::down_list(&mut t, at)
                            .ok_or("Containing expression ends prematurely")?
                    } else {
                        motion::up_list(&mut t, at).ok_or("At top level")?
                    };
                }
                self.goto(view, at);
                Ok(())
            }
            "beginning-of-defun" | "end-of-defun" => {
                let doc = Doc::new(view.buffer());
                let (start, end) = defun(&doc, self.point(view), name == "end-of-defun")
                    .ok_or("No function here")?;
                let at = if name == "end-of-defun" { end } else { start };
                self.goto(view, at);
                Ok(())
            }
            "beginning-of-buffer" | "end-of-buffer" => {
                let doc = Doc::new(view.buffer());
                let here = self.point(view);
                if !self.active {
                    self.push_mark(view, here);
                    ui::show_message("Mark set");
                }
                let tenths = |n: i64| doc.len * n.clamp(0, 10) as u64 / 10;
                let mut at = match (name, arg.given()) {
                    ("beginning-of-buffer", false) => 0,
                    ("end-of-buffer", false) => doc.len,
                    ("beginning-of-buffer", true) => tenths(n),
                    _ => doc.len - tenths(n),
                };
                if let Arg::Number(_) = arg
                    && at > 0
                    && at < doc.len
                {
                    at = doc.line_start(doc.line_of(at) + 1);
                }
                self.goto(view, at);
                Ok(())
            }
            "scroll-up-command" | "scroll-down-command" => {
                let direction = if name == "scroll-up-command" { 1 } else { -1 };
                let amount = if arg.given() {
                    ScrollAmount::Lines((n * direction) as i32)
                } else {
                    ScrollAmount::Page(direction as i32)
                };
                let lines = view.scroll(amount);
                if lines == 0 {
                    return Err(if direction > 0 {
                        "End of buffer".into()
                    } else {
                        "Beginning of buffer".into()
                    });
                }
                self.this = Last::Vertical;
                let column = self.goal_column();
                if let Ok((at, aimed)) = view.move_vertically(self.point(view), lines, column) {
                    self.column = Some(aimed);
                    self.goto(view, at);
                }
                Ok(())
            }
            "recenter-top-bottom" => {
                self.recenter(view, arg);
                Ok(())
            }
            "move-to-window-line-top-bottom" => {
                self.window_line(view, arg);
                Ok(())
            }
            "goto-line" | "goto-char" | "move-to-column" => {
                if let Arg::Number(n) = arg {
                    self.go(view, name, n)
                } else {
                    self.ask_number(name, arg);
                    Ok(())
                }
            }
            "self-insert-command" => self.self_insert(view, arg, key),
            "newline" => self.newline(view, n),
            "electric-newline-and-maybe-indent" | "open-line" => {
                if n < 0 {
                    return Err("Negative repetition argument".into());
                }
                let here = self.point(view);
                let text = "\n".repeat(n as usize);
                let after = if name == "open-line" {
                    here
                } else {
                    here + n as u64
                };
                self.edit(view, vec![insertion(here, text)], after);
                Ok(())
            }
            "indent-for-tab-command" => self.indent_relative(view),
            "indent-rigidly" => {
                self.region(view)?;
                if arg.given() {
                    self.shift_lines(view, n);
                    self.deactivate = true;
                } else {
                    self.waiting = Some(Waiting::IndentRigidly);
                    ui::show_message("Indent region with <left>, <right>, S-<left>, or S-<right>.");
                }
                Ok(())
            }
            "delete-char" => self.delete_chars(view, n, Last::DeleteChar),
            "delete-backward-char" | "delete-forward-char" => {
                if self.active && !arg.given() {
                    if self.rectangle {
                        return self.rectangle_command(view, "delete-rectangle");
                    }
                    let (from, to) = self.region(view)?;
                    self.replace(view, from, to, "", from);
                    return Ok(());
                }
                let n = if name == "delete-backward-char" {
                    -n
                } else {
                    n
                };
                let last = if name == "delete-backward-char" {
                    Last::DeleteBackward
                } else {
                    Last::DeleteChar
                };
                self.delete_chars(view, n, last)
            }
            "kill-line" => self.kill_line(view, arg),
            "kill-whole-line" => self.kill_whole_line(view, n),
            "kill-word" | "backward-kill-word" => {
                let n = if name == "kill-word" { n } else { -n };
                let doc = Doc::new(view.buffer());
                let mut t = Text::new(&doc);
                let here = self.point(view);
                let to = motion::words(&mut t, here, n);
                self.kill_between(view, here, to);
                Ok(())
            }
            "kill-sentence" => {
                let here = self.point(view);
                let to = self.sentences(view, n);
                self.kill_between(view, here, to);
                Ok(())
            }
            "kill-sexp" => {
                let doc = Doc::new(view.buffer());
                let here = self.point(view);
                let (to, result) = sexps(&doc, here, n);
                result?;
                self.kill_between(view, here, to);
                Ok(())
            }
            "zap-to-char" | "zap-up-to-char" => {
                self.waiting = Some(Waiting::Zap {
                    up_to: name == "zap-up-to-char",
                    arg,
                });
                ui::show_message(if name == "zap-to-char" {
                    "Zap to char: "
                } else {
                    "Zap up to char: "
                });
                Ok(())
            }
            "kill-region" => {
                if self.active && self.rectangle {
                    return self.rectangle_command(view, "kill-rectangle-to-ring");
                }
                let (from, to) = self.region(view)?;
                let text = view.buffer().slice(from, to).unwrap_or_default();
                self.kill(text, false);
                self.replace(view, from, to, "", from);
                Ok(())
            }
            "kill-ring-save" => {
                if self.active && self.rectangle {
                    return self.rectangle_command(view, "copy-rectangle-to-ring");
                }
                let (from, to) = self.region(view)?;
                let text = view.buffer().slice(from, to).unwrap_or_default();
                if self.last == Last::Kill
                    && let Some(first) = self.kills.first_mut()
                {
                    first.text.push_str(&text);
                    self.sync_clipboard();
                } else {
                    self.kill_new(Kill { text, rect: None });
                }
                self.deactivate = true;
                Ok(())
            }
            "yank" => self.yank(view, arg),
            "yank-pop" => self.yank_pop(view, n),
            "transpose-chars" => self.transpose_chars(view, arg),
            "transpose-words" => self.transpose_words(view, n),
            "transpose-lines" => self.transpose_lines(view, n),
            "upcase-word" | "downcase-word" | "capitalize-word" => {
                let doc = Doc::new(view.buffer());
                let mut t = Text::new(&doc);
                let here = self.point(view);
                let to = motion::words(&mut t, here, n);
                let case = Case::of(name);
                let after = if n < 0 { here } else { to };
                self.recase(view, here.min(to), here.max(to), case, after);
                Ok(())
            }
            "upcase-region" | "downcase-region" | "capitalize-region" => {
                let (from, to) = self.region(view)?;
                let here = self.point(view);
                self.recase(view, from, to, Case::of(name), here);
                Ok(())
            }
            "delete-horizontal-space" => {
                let (from, to) = self.blanks_around(view);
                let to = if arg.given() { self.point(view) } else { to };
                self.replace(view, from, to, "", from);
                Ok(())
            }
            "just-one-space" => {
                let (from, to) = self.blanks_around(view);
                let spaces = " ".repeat(n.max(0) as usize);
                self.replace(view, from, to, &spaces, from + spaces.len() as u64);
                Ok(())
            }
            "cycle-spacing" => self.cycle_spacing(view, n),
            "delete-indentation" => self.delete_indentation(view, arg.given()),
            "delete-blank-lines" => self.delete_blank_lines(view),
            "delete-trailing-whitespace" => {
                self.delete_trailing_whitespace(view);
                Ok(())
            }
            "quoted-insert" => {
                self.waiting = Some(Waiting::Quoted(arg));
                Ok(())
            }
            "dabbrev-expand" => self.dabbrev_expand(view),
            "completion-at-point" => {
                call_or_show("lsp.complete");
                Ok(())
            }
            "undo" | "undo-redo" => {
                for _ in 0..n.max(1) {
                    let changed = if name == "undo" {
                        view.undo()
                    } else {
                        view.redo()
                    };
                    // The point goes where the text changed, as in Emacs.
                    match changed {
                        Some(changed) => self.goto(view, changed.start),
                        None => {
                            return Err(if name == "undo" {
                                "No further undo information".into()
                            } else {
                                "No further redo information".into()
                            });
                        }
                    }
                }
                self.deactivate = true;
                Ok(())
            }
            "set-mark-command" => self.set_mark_command(view, arg),
            "exchange-point-and-mark" => {
                let buffer = view.buffer();
                let mark = self.mark(&buffer).ok_or("No mark set in this buffer")?;
                let here = self.point(view);
                self.set_mark(view, here);
                self.goto(view, mark);
                if !arg.is_plain_universal() {
                    self.active = true;
                }
                Ok(())
            }
            "mark-whole-buffer" => {
                let here = self.point(view);
                let len = view.buffer().len();
                self.push_mark(view, here);
                self.push_mark(view, len);
                self.goto(view, 0);
                self.active = true;
                ui::show_message("Mark set");
                Ok(())
            }
            "mark-paragraph" => {
                let doc = Doc::new(view.buffer());
                let here = self.point(view);
                let extend = self.last == Last::MarkParagraph && self.active;
                self.this = Last::MarkParagraph;
                if extend && let Some(mark) = self.mark(&view.buffer()) {
                    let end = paragraphs(&doc, mark, n);
                    self.set_mark(view, end);
                    return Ok(());
                }
                let end = paragraphs(&doc, here, n);
                let start = paragraphs(&doc, end, -n);
                self.push_mark(view, end);
                self.goto(view, start);
                self.active = true;
                Ok(())
            }
            "mark-word" | "mark-sexp" => {
                let doc = Doc::new(view.buffer());
                let this = if name == "mark-word" {
                    Last::MarkWord
                } else {
                    Last::MarkSexp
                };
                let buffer = view.buffer();
                let from = match self.mark(&buffer) {
                    Some(mark) if self.last == this && self.active => mark,
                    _ => self.point(view),
                };
                self.this = this;
                let to = if name == "mark-word" {
                    motion::words(&mut Text::new(&doc), from, n)
                } else {
                    let (to, result) = sexps(&doc, from, n);
                    result?;
                    to
                };
                if self.last == this && self.active {
                    self.set_mark(view, to);
                } else {
                    self.push_mark(view, to);
                }
                self.active = true;
                Ok(())
            }
            "mark-defun" => {
                let doc = Doc::new(view.buffer());
                let (start, end) =
                    defun(&doc, self.point(view), false).ok_or("No function here")?;
                self.push_mark(view, end);
                self.goto(view, start);
                self.active = true;
                Ok(())
            }
            "rectangle-mark-mode" => {
                self.rectangle = !self.rectangle;
                if self.rectangle {
                    if !self.active {
                        let here = self.point(view);
                        self.push_mark(view, here);
                        self.active = true;
                    }
                    ui::show_message("Mark set (rectangle mode)");
                }
                Ok(())
            }
            "keyboard-quit" | "keyboard-escape-quit" => {
                self.deactivate = true;
                self.sequence.cancel();
                self.waiting = None;
                Err("Quit".into())
            }
            "isearch-forward"
            | "isearch-backward"
            | "isearch-forward-regexp"
            | "isearch-backward-regexp" => {
                self.start_isearch(
                    view,
                    name.starts_with("isearch-forward"),
                    name.ends_with("regexp"),
                );
                Ok(())
            }
            "query-replace" | "query-replace-regexp" | "replace-string" | "replace-regexp" => {
                self.ask_replace(name);
                Ok(())
            }
            "kill-rectangle"
            | "copy-rectangle-as-kill"
            | "delete-rectangle"
            | "yank-rectangle"
            | "open-rectangle"
            | "clear-rectangle"
            | "string-rectangle"
            | "rectangle-number-lines" => self.rectangle_command(view, name),
            "copy-to-register"
            | "insert-register"
            | "copy-rectangle-to-register"
            | "point-to-register"
            | "jump-to-register"
            | "number-to-register"
            | "increment-register" => {
                let command = crate::bind::COMMANDS
                    .iter()
                    .map(|(n, _, _)| *n)
                    .find(|n| *n == name)
                    .expect("a register command");
                self.waiting = Some(Waiting::Register { command, arg });
                ui::show_message(&format!("{}: ", register_prompt(name)));
                Ok(())
            }
            "kmacro-start-macro" => {
                if self.recording.is_some() {
                    return Err("Already defining keyboard macro".into());
                }
                let keys = if arg.given() {
                    let keys = self.last_macro.clone().unwrap_or_default();
                    self.play(&keys);
                    keys
                } else {
                    Vec::new()
                };
                self.recording = Some(keys);
                self.command_start = 0;
                ui::show_message("Defining kbd macro...");
                Ok(())
            }
            "kmacro-end-macro" => {
                self.end_macro()?;
                if n > 1 {
                    self.call_macro(n - 1)?;
                }
                Ok(())
            }
            "kmacro-end-or-call-macro" => {
                if self.recording.is_some() {
                    self.end_macro()
                } else {
                    self.call_macro(n)
                }
            }
            "kmacro-end-and-call-macro" => {
                if self.recording.is_some() {
                    self.end_macro()?;
                }
                self.call_macro(n)?;
                self.waiting = Some(Waiting::Again('e'));
                Ok(())
            }
            "repeat" => {
                let (last, last_arg, last_key) =
                    self.last_command.clone().ok_or("No repeatable command")?;
                let arg = if arg.given() { arg } else { last_arg };
                self.run_with(&last, arg, last_key);
                self.last_command = Some((last, arg, last_key));
                self.waiting = Some(Waiting::Again('z'));
                Ok(())
            }
            "execute-extended-command" => {
                self.ask_command(arg);
                Ok(())
            }
            "describe-key" => {
                self.waiting = Some(Waiting::Describe(Vec::new()));
                ui::show_message("Describe the following key: ");
                Ok(())
            }
            "what-cursor-position" => {
                what_cursor_position(view, self.point(view));
                Ok(())
            }
            "xref-find-definitions" => {
                let buffer = view.buffer();
                self.xref.push((buffer.path(), self.point(view)));
                commands::call("lsp.definition", "")
                    .map(|_| ())
                    .inspect_err(|_| {
                        self.xref.pop();
                    })
            }
            "xref-go-back" => {
                let (path, at) = self.xref.pop().ok_or("At start of history")?;
                if let Some(path) = path.filter(|p| Some(p) != view.buffer().path().as_ref()) {
                    base_kit::open_file(&path)?;
                }
                let view = nib_plugin::nib::plugin::view::active();
                let at = at.min(view.buffer().len());
                self.goto(&view, at);
                Ok(())
            }
            _ => self.run_file_command(view, name, arg),
        }
    }

    fn goal_column(&self) -> Option<u32> {
        if self.last == Last::Vertical {
            self.column
        } else {
            None
        }
    }

    /// `C-f` and `C-b`: to the end or start of the buffer, then an error.
    fn chars(&mut self, view: &View, n: i64) -> Result<(), String> {
        let doc = Doc::new(view.buffer());
        let mut at = self.point(view);
        for _ in 0..n.unsigned_abs() {
            let next = if n > 0 {
                doc.next_grapheme(at)
            } else {
                doc.prev_grapheme(at)
            };
            if next == at {
                self.goto(view, at);
                return Err(edge(n > 0));
            }
            at = next;
        }
        self.goto(view, at);
        Ok(())
    }

    /// `C-n` and `C-p`, keeping the column; past the last line, to the end
    /// of the buffer, then an error.
    fn lines(&mut self, view: &View, n: i64) -> Result<(), String> {
        self.this = Last::Vertical;
        let mut column = self.goal_column();
        let mut at = self.point(view);
        let step = if n > 0 { 1 } else { -1 };
        for _ in 0..n.unsigned_abs() {
            let moved = view.move_vertically(at, step, column).ok();
            if let Some((_, aimed)) = moved {
                column = Some(aimed);
            }
            // At the first or last line, the core stays on it.
            let Some((next, _)) =
                moved.filter(|&(next, _)| (next > at) == (step > 0) && next != at)
            else {
                let len = view.buffer().len();
                self.column = column;
                self.goto(view, if n > 0 { len } else { 0 });
                return Err(edge(n > 0));
            };
            at = next;
        }
        self.column = column;
        self.goto(view, at);
        Ok(())
    }

    fn sentences(&self, view: &View, n: i64) -> u64 {
        let doc = Doc::new(view.buffer());
        let mut at = self.point(view);
        for _ in 0..n.unsigned_abs() {
            at = if n > 0 {
                motion::forward_sentence(&doc, at).unwrap_or(doc.len)
            } else {
                motion::backward_sentence(&doc, at).unwrap_or(0)
            };
        }
        at
    }

    /// `C-l`: the point's line to the middle, the top, then the bottom.
    fn recenter(&mut self, view: &View, arg: Arg) {
        let doc = Doc::new(view.buffer());
        let shown = view.visible_range();
        let (top, bottom) = (shown.start, shown.end);
        let first = doc.line_of(top) as i64;
        let height = (doc.line_of(bottom) as i64 - first + 1).max(1);
        let line = doc.line_of(self.point(view)) as i64;
        let stage = if self.last == Last::Recenter {
            self.recenter_stage(first, height, line) + 1
        } else {
            0
        };
        self.this = Last::Recenter;
        let want = match (arg, stage % 3) {
            (Arg::Number(n), _) if n >= 0 => line - n,
            (Arg::Number(n), _) => line - (height + n),
            (_, 0) => line - height / 2,
            (_, 1) => line,
            _ => line - height + 1,
        };
        view.scroll(ScrollAmount::Lines((want.max(0) - first) as i32));
    }

    /// Which of the middle, top, and bottom the point's line is at.
    fn recenter_stage(&self, first: i64, height: i64, line: i64) -> i64 {
        if line == first {
            1
        } else if line == first + height - 1 {
            2
        } else {
            0
        }
    }

    /// `M-r`: the point to the middle line on screen, the top, then the
    /// bottom.
    fn window_line(&mut self, view: &View, arg: Arg) {
        let doc = Doc::new(view.buffer());
        let shown = view.visible_range();
        let (top, bottom) = (shown.start, shown.end);
        let first = doc.line_of(top) as i64;
        let last = doc.line_of(bottom) as i64;
        let line = doc.line_of(self.point(view)) as i64;
        let stage = if self.last == Last::WindowLine {
            if line == first {
                2
            } else if line == last {
                0
            } else {
                1
            }
        } else {
            0
        };
        self.this = Last::WindowLine;
        let target = match (arg, stage) {
            (Arg::Number(n), _) if n >= 0 => first + n,
            (Arg::Number(n), _) => last + 1 + n,
            (_, 0) => (first + last) / 2,
            (_, 1) => first,
            _ => last,
        };
        let target = target.clamp(first, last) as u64;
        let at = doc::first_non_blank(&doc, doc.line_start(target));
        self.goto(view, at);
    }

    /// Goes to line `n`, char `n`, or column `n`, as `name` says.
    pub fn go(&mut self, view: &View, name: &str, n: i64) -> Result<(), String> {
        let doc = Doc::new(view.buffer());
        let here = self.point(view);
        match name {
            "goto-line" => {
                if !self.active {
                    self.push_mark(view, here);
                }
                let line = (n - 1).clamp(0, doc.line_count() as i64 - 1) as u64;
                self.goto(view, doc.line_start(line));
            }
            "goto-char" => {
                if !self.active {
                    self.push_mark(view, here);
                }
                let mut t = Text::new(&doc);
                let mut at = 0;
                for _ in 1..n.max(1) {
                    match t.next(at) {
                        Some(next) => at = next,
                        None => break,
                    }
                }
                self.goto(view, at);
            }
            _ => {
                let start = doc.line_start(doc.line_of(here));
                let (at, _) = motion::at_column(&doc, start, n.max(0) as u32);
                self.goto(view, at);
            }
        }
        Ok(())
    }

    fn self_insert(&mut self, view: &View, arg: Arg, key: KeyEvent) -> Result<(), String> {
        let c = match key.code {
            KeyCode::Char(c) => c,
            KeyCode::Tab => '\t',
            KeyCode::Enter => '\n',
            _ => return Err(format!("{} cannot be inserted", crate::key_label(&key))),
        };
        let n = arg.count();
        if n < 0 {
            return Err("Negative repetition argument".into());
        }
        self.insert_typed(view, &c.to_string().repeat(n as usize), Last::SelfInsert);
        Ok(())
    }

    /// Inserts typed text at the point, joining the undo step of the chars
    /// typed just before.
    fn insert_typed(&mut self, view: &View, text: &str, this: Last) {
        self.merge = self.last == this && self.amalgamated < crate::AMALGAMATE;
        self.amalgamated = if self.merge { self.amalgamated + 1 } else { 1 };
        self.this = this;
        let here = self.point(view);
        self.edit(
            view,
            vec![insertion(here, text.to_string())],
            here + text.len() as u64,
        );
    }

    /// `C-q`: the key typed after it, as it is.
    pub fn insert_quoted(&mut self, view: &View, arg: Arg, ev: KeyEvent) -> Result<(), String> {
        let c = quoted_char(ev)
            .ok_or_else(|| format!("{} cannot be inserted", crate::key_label(&ev)))?;
        let n = arg.count().max(0) as usize;
        let here = self.point(view);
        let text = c.to_string().repeat(n);
        self.edit(
            view,
            vec![insertion(here, text.clone())],
            here + text.len() as u64,
        );
        Ok(())
    }

    /// `RET`, as `electric-indent-mode` does it: the line left loses its
    /// trailing whitespace, and the new one is indented like the nearest
    /// line above with text.
    fn newline(&mut self, view: &View, n: i64) -> Result<(), String> {
        if n < 0 {
            return Err("Negative repetition argument".into());
        }
        let doc = Doc::new(view.buffer());
        let here = self.point(view);
        let line = doc.line_of(here);
        let start = doc.line_start(line);
        let end = doc.line_end(here);
        let before = doc.slice(start, here);
        let after = doc.slice(here, end);
        let from = start + before.trim_end_matches([' ', '\t']).len() as u64;
        let to = here + (after.len() - after.trim_start_matches([' ', '\t']).len()) as u64;
        let left = &before[..(from - start) as usize];
        // The new line is indented as the line above it, where there is
        // one with text.
        let indent = if n > 1 || left.trim().is_empty() {
            String::new()
        } else {
            doc::indentation(&doc, start)
        };
        let text = format!("{}{indent}", "\n".repeat(n as usize));
        let after = from + text.len() as u64;
        self.replace(view, from, to, &text, after);
        Ok(())
    }

    /// `TAB`: whitespace up to the next indent point of the line above
    /// with text, or else to the next tab stop.
    fn indent_relative(&mut self, view: &View) -> Result<(), String> {
        let doc = Doc::new(view.buffer());
        let here = self.point(view);
        let line = doc.line_of(here);
        let col = motion::column(&doc, here);
        let tab = motion::tab_width();
        let unit = indent_unit();
        let tabs = unit == "\t";
        let step = if tabs { tab } else { unit.len() as u32 };
        let above = (0..line)
            .rev()
            .find(|&l| !motion::blank_line(&doc, l))
            .map(|l| {
                let start = doc.line_start(l);
                doc.slice(start, doc.line_end(start))
            });
        let target = above
            .and_then(|text| motion::indent_point(&text, col, tab))
            .unwrap_or((col / step + 1) * step);
        let text = motion::spaces(col, target, tabs, tab);
        self.edit(
            view,
            vec![insertion(here, text.clone())],
            here + text.len() as u64,
        );
        Ok(())
    }

    /// Moves the region's lines right by `n` columns, or left.
    pub fn shift_lines(&mut self, view: &View, n: i64) {
        let Ok((from, to)) = self.region(view) else {
            return;
        };
        let doc = Doc::new(view.buffer());
        let tab = motion::tab_width();
        let tabs = indent_unit() == "\t";
        let first = doc.line_of(from);
        let mut last = doc.line_of(to);
        if last > first && doc.line_start(last) == to {
            last -= 1;
        }
        let edits: Vec<Edit> = (first..=last)
            .filter_map(|line| {
                let start = doc.line_start(line);
                let end = doc.line_end(start);
                let text = doc.slice(start, end);
                let rest = text.trim_start_matches([' ', '\t']);
                let lead = (text.len() - rest.len()) as u64;
                let new = if rest.is_empty() {
                    String::new()
                } else {
                    let width = motion::indent_width(&text, tab) as i64;
                    motion::spaces(0, (width + n).max(0) as u32, tabs, tab)
                };
                (new != text[..lead as usize]).then(|| Edit {
                    start,
                    end: start + lead,
                    text: new,
                })
            })
            .collect();
        let deactivate = self.deactivate;
        self.edit_around(view, edits);
        self.deactivate = deactivate;
    }

    /// `C-x TAB` without an argument: arrows move the lines, and any other
    /// key ends it and does what it does.
    pub fn indent_rigidly_key(&mut self, ev: KeyEvent) -> bool {
        let tab = motion::tab_width() as i64;
        let n = match (ev.code, ev.modifiers) {
            (KeyCode::Left, m) if m.is_empty() => -1,
            (KeyCode::Right, m) if m.is_empty() => 1,
            (KeyCode::Left, Modifiers::SHIFT) => -tab,
            (KeyCode::Right, Modifiers::SHIFT) => tab,
            _ => {
                self.active = false;
                self.render();
                return false;
            }
        };
        let view = nib_plugin::nib::plugin::view::active();
        self.shift_lines(&view, n);
        self.waiting = Some(Waiting::IndentRigidly);
        self.render();
        true
    }

    fn delete_chars(&mut self, view: &View, n: i64, this: Last) -> Result<(), String> {
        let doc = Doc::new(view.buffer());
        let here = self.point(view);
        let mut at = here;
        for _ in 0..n.unsigned_abs() {
            let next = if n > 0 {
                doc.next_grapheme(at)
            } else {
                doc.prev_grapheme(at)
            };
            if next == at {
                return Err(edge(n > 0));
            }
            at = next;
        }
        self.merge = self.last == this && self.amalgamated < crate::AMALGAMATE;
        self.amalgamated = if self.merge { self.amalgamated + 1 } else { 1 };
        self.this = this;
        let (from, to) = (here.min(at), here.max(at));
        self.edit(view, vec![deletion(from, to)], from);
        Ok(())
    }

    /// Kills from the point to `to`, which may be before it.
    fn kill_between(&mut self, view: &View, here: u64, to: u64) {
        let (from, end) = (here.min(to), here.max(to));
        let text = view.buffer().slice(from, end).unwrap_or_default();
        self.kill(text, to < here);
        self.replace(view, from, end, "", from);
    }

    /// `C-k`: the rest of the line, or its line break too when only
    /// whitespace is left; lines by the argument.
    fn kill_line(&mut self, view: &View, arg: Arg) -> Result<(), String> {
        let doc = Doc::new(view.buffer());
        let here = self.point(view);
        let line = doc.line_of(here);
        let to = match arg {
            Arg::None => {
                if here >= doc.len {
                    return Err("End of buffer".into());
                }
                let end = doc.line_end(here);
                let rest = doc.slice(here, end);
                if rest.chars().all(|c| c == ' ' || c == '\t') && end < doc.len {
                    end + 1
                } else {
                    end
                }
            }
            _ => {
                let n = arg.count();
                if n > 0 {
                    let target = line + n as u64;
                    if target >= doc.line_count() {
                        doc.len
                    } else {
                        doc.line_start(target)
                    }
                } else {
                    let target = line.saturating_sub(n.unsigned_abs());
                    doc.line_start(target)
                }
            }
        };
        if to > here && here >= doc.len {
            return Err("End of buffer".into());
        }
        self.kill_between(view, here, to);
        Ok(())
    }

    fn kill_whole_line(&mut self, view: &View, n: i64) -> Result<(), String> {
        let doc = Doc::new(view.buffer());
        let here = self.point(view);
        let line = doc.line_of(here);
        let start = doc.line_start(line);
        let (from, to) = if n == 0 {
            (start, doc.line_end(here))
        } else if n > 0 {
            let target = line + n as u64;
            let to = if target >= doc.line_count() {
                doc.len
            } else {
                doc.line_start(target)
            };
            (start, to)
        } else {
            let first = line.saturating_sub(n.unsigned_abs() - 1);
            let from = doc.line_start(first);
            let from = if from > 0 { from - 1 } else { from };
            (from, doc.line_end(here))
        };
        let text = doc.slice(from, to);
        self.kill(text, n < 0);
        self.replace(view, from, to, "", from);
        Ok(())
    }

    /// `C-y`, with `C-u` leaving the point before the text, and a number
    /// taking that kill.
    fn yank(&mut self, view: &View, arg: Arg) -> Result<(), String> {
        self.from_clipboard();
        if self.kills.is_empty() {
            return Err("Kill ring is empty".into());
        }
        let len = self.kills.len() as i64;
        let rotate = match arg {
            Arg::None | Arg::Universal(_) => 0,
            Arg::Minus => -2,
            Arg::Number(n) => n - 1,
        };
        self.yank_at = (self.yank_at as i64 + rotate).rem_euclid(len) as usize;
        let kill = self.kills[self.yank_at].clone();
        let here = self.point(view);
        let (start, end) = self.insert_kill(view, here, &kill)?;
        self.push_mark(view, start);
        view.buffer().set_marks("yank", &[start, end]);
        if matches!(arg, Arg::Universal(_)) {
            self.set_mark(view, end);
            self.goto(view, start);
        }
        self.this = Last::Yank;
        Ok(())
    }

    /// Inserts a kill at `at`, as text or as a rectangle, and returns where
    /// it went.
    fn insert_kill(&mut self, view: &View, at: u64, kill: &Kill) -> Result<(u64, u64), String> {
        match &kill.rect {
            Some(lines) => {
                let end = self.insert_rectangle(view, at, lines);
                Ok((at, end))
            }
            None => {
                let end = at + kill.text.len() as u64;
                self.edit(view, vec![insertion(at, kill.text.clone())], end);
                Ok((at, end))
            }
        }
    }

    /// `M-y`: the text the yank before put in becomes the kill before it.
    fn yank_pop(&mut self, view: &View, n: i64) -> Result<(), String> {
        if self.last != Last::Yank {
            return Err("Previous command was not a yank".into());
        }
        let buffer = view.buffer();
        let [start, end] = buffer.marks("yank")[..] else {
            return Err("Previous command was not a yank".into());
        };
        let len = self.kills.len() as i64;
        self.yank_at = (self.yank_at as i64 + n).rem_euclid(len.max(1)) as usize;
        let kill = self.kills[self.yank_at].clone();
        let point_first = self.point(view) == start && start != end;
        self.edit(view, vec![deletion(start, end)], start);
        let (start, end) = self.insert_kill(view, start, &kill)?;
        buffer.set_marks("yank", &[start, end]);
        if point_first {
            self.set_mark(view, end);
            self.goto(view, start);
        } else {
            self.set_mark(view, start);
            self.goto(view, end);
        }
        self.this = Last::Yank;
        Ok(())
    }

    fn transpose_chars(&mut self, view: &View, arg: Arg) -> Result<(), String> {
        let doc = Doc::new(view.buffer());
        let mut here = self.point(view);
        if !arg.given() && here == doc.line_end(here) && here > doc.line_start(doc.line_of(here)) {
            here = doc.prev_grapheme(here);
        }
        let n = arg.count();
        for _ in 0..n.unsigned_abs() {
            let doc = Doc::new(view.buffer());
            if here == 0 {
                return Err("Beginning of buffer".into());
            }
            let before = doc.prev_grapheme(here);
            let after = doc.next_grapheme(here);
            if after == here {
                return Err("End of buffer".into());
            }
            let (a, b) = (doc.slice(before, here), doc.slice(here, after));
            let text = format!("{b}{a}");
            let at = if n > 0 {
                after
            } else {
                before + b.len() as u64
            };
            self.replace(view, before, after, &text, at);
            here = at;
            if n < 0 {
                here = Doc::new(view.buffer()).prev_grapheme(here);
            }
        }
        Ok(())
    }

    fn transpose_words(&mut self, view: &View, n: i64) -> Result<(), String> {
        for _ in 0..n.unsigned_abs() {
            let doc = Doc::new(view.buffer());
            let mut t = Text::new(&doc);
            let here = self.point(view);
            let none = || "Don’t have two things to transpose".to_string();
            // The word at or before the point, and the one after it.
            let s1 = motion::backward_word(&mut t, here).ok_or_else(none)?;
            let e1 = motion::forward_word(&mut t, s1).ok_or_else(none)?;
            let e2 = motion::forward_word(&mut t, e1).filter(|&e2| {
                e2 > e1 && t.prev(e2).and_then(|p| t.char_at(p)).is_some_and(is_word)
            });
            let Some(e2) = e2 else {
                self.goto(view, s1);
                return Err(none());
            };
            let s2 = motion::backward_word(&mut t, e2).ok_or_else(none)?;
            let (w1, mid, w2) = (doc.slice(s1, e1), doc.slice(e1, s2), doc.slice(s2, e2));
            self.replace(view, s1, e2, &format!("{w2}{mid}{w1}"), e2);
        }
        Ok(())
    }

    fn transpose_lines(&mut self, view: &View, n: i64) -> Result<(), String> {
        for _ in 0..n.unsigned_abs().max(1) {
            let doc = Doc::new(view.buffer());
            let here = self.point(view);
            let line = doc.line_of(here);
            if line == 0 {
                return Err("Don’t have two things to transpose".into());
            }
            let prev = doc.line_start(line - 1);
            let start = doc.line_start(line);
            let end = doc.line_end(start);
            let upper = doc.slice(prev, start - 1);
            let lower = doc.slice(start, end);
            let (text, to) = if end >= doc.len {
                (format!("{lower}\n{upper}\n"), end)
            } else {
                (format!("{lower}\n{upper}"), end)
            };
            let after = prev + text.len() as u64 + u64::from(end < doc.len);
            self.replace(
                view,
                prev,
                to,
                &text,
                after.min(prev + text.len() as u64 + 1),
            );
            let doc = Doc::new(view.buffer());
            let at = doc.line_start(line + 1).min(doc.len);
            self.goto(view, at);
        }
        Ok(())
    }

    fn recase(&mut self, view: &View, from: u64, to: u64, case: Case, after: u64) {
        let text = view.buffer().slice(from, to).unwrap_or_default();
        let new = case.apply(&text);
        if new != text {
            self.replace(view, from, to, &new, after);
        } else {
            self.goto(view, after);
        }
    }

    /// The spaces and tabs around the point.
    fn blanks_around(&self, view: &View) -> (u64, u64) {
        let doc = Doc::new(view.buffer());
        let here = self.point(view);
        let start = doc.line_start(doc.line_of(here));
        let end = doc.line_end(here);
        let before = doc.slice(start, here);
        let after = doc.slice(here, end);
        let from = start + before.trim_end_matches([' ', '\t']).len() as u64;
        let to = here + (after.len() - after.trim_start_matches([' ', '\t']).len()) as u64;
        (from, to)
    }

    /// `M-SPC`: one space, then none, then what there was.
    fn cycle_spacing(&mut self, view: &View, n: i64) -> Result<(), String> {
        self.this = Last::CycleSpacing;
        let spacing = if self.last == Last::CycleSpacing {
            self.spacing.clone()
        } else {
            None
        };
        match spacing {
            None => {
                let (from, to) = self.blanks_around(view);
                let original = view.buffer().slice(from, to).unwrap_or_default();
                let spaces = " ".repeat(n.max(0) as usize);
                let offset = self.point(view) - from;
                self.replace(view, from, to, &spaces, from + spaces.len() as u64);
                self.spacing = Some(Spacing {
                    start: from,
                    original,
                    offset,
                    stage: 1,
                });
            }
            Some(mut spacing) if spacing.stage == 1 => {
                let (from, to) = self.blanks_around(view);
                self.replace(view, from, to, "", from);
                spacing.stage = 2;
                spacing.start = from;
                self.spacing = Some(spacing);
            }
            Some(spacing) => {
                let at = spacing.start;
                let end = at + spacing.offset;
                self.replace(view, at, at, &spacing.original, end);
                self.spacing = None;
                self.this = Last::Other;
            }
        }
        Ok(())
    }

    /// `M-^`: joins the line to the one above, or with an argument, the
    /// next line to this one, leaving one space where Emacs would.
    fn delete_indentation(&mut self, view: &View, next: bool) -> Result<(), String> {
        let doc = Doc::new(view.buffer());
        let here = self.point(view);
        let line = doc.line_of(here) + u64::from(next);
        if line == 0 || line >= doc.line_count() {
            return Ok(());
        }
        let start = doc.line_start(line);
        let upper_start = doc.line_start(line - 1);
        let upper = doc.slice(upper_start, start - 1);
        let from = upper_start + upper.trim_end_matches([' ', '\t']).len() as u64;
        let rest_end = doc.line_end(start);
        let rest = doc.slice(start, rest_end);
        let to = start + (rest.len() - rest.trim_start_matches([' ', '\t']).len()) as u64;
        let prev = upper.trim_end_matches([' ', '\t']).chars().next_back();
        let next_char = rest.trim_start_matches([' ', '\t']).chars().next();
        let space = !(from == upper_start
            || next_char.is_none()
            || next_char.is_some_and(|c| ")]}".contains(c))
            || prev.is_some_and(|c| "([{'".contains(c)));
        self.replace(view, from, to, if space { " " } else { "" }, from);
        Ok(())
    }

    /// `C-x C-o`: on a blank line, leaves one of the blank lines around; on
    /// a lone blank line, deletes it; on a line with text, deletes the
    /// blank lines after.
    fn delete_blank_lines(&mut self, view: &View) -> Result<(), String> {
        let doc = Doc::new(view.buffer());
        let here = self.point(view);
        let line = doc.line_of(here);
        let count = doc.line_count();
        let blank = |l: u64| l < count && motion::blank_line(&doc, l);
        // The phantom line after a final line break is not a line to keep.
        let last = doc.last_line();
        let blank = |l: u64| l <= last && blank(l);
        if blank(line) {
            let mut first = line;
            while first > 0 && blank(first - 1) {
                first -= 1;
            }
            let mut end = line;
            while blank(end + 1) {
                end += 1;
            }
            let from = doc.line_start(first);
            let to = doc.line_start(end + 1).min(doc.len);
            if first == end {
                self.replace(view, from, to, "", from);
            } else {
                self.replace(view, from, to, "\n", from);
            }
        } else {
            let mut end = line;
            while blank(end + 1) {
                end += 1;
            }
            if end > line {
                let from = doc.line_start(line + 1);
                let to = doc.line_start(end + 1).min(doc.len);
                self.replace(view, from, to, "", here);
            }
        }
        Ok(())
    }

    fn delete_trailing_whitespace(&mut self, view: &View) {
        let buffer = view.buffer();
        let Ok(found) = buffer.find_all("[ \\t]+$", 0, buffer.len()) else {
            return;
        };
        let edits: Vec<Edit> = found
            .into_iter()
            .filter(|r| r.start < r.end)
            .map(|r| deletion(r.start, r.end))
            .collect();
        self.edit_around(view, edits);
    }

    /// `M-z`: kills through the `count`th of the char typed, or up to it.
    pub fn zap(&mut self, view: &View, up_to: bool, arg: Arg, ev: KeyEvent) -> Result<(), String> {
        let c = match ev.code {
            KeyCode::Char(c) => c,
            KeyCode::Enter => '\n',
            KeyCode::Tab => '\t',
            _ => return Err("Wrong type argument: characterp".into()),
        };
        let n = arg.count();
        let doc = Doc::new(view.buffer());
        let mut t = Text::new(&doc);
        let here = self.point(view);
        let mut at = here;
        let mut found = 0;
        while found < n.unsigned_abs() {
            let next = if n > 0 { t.next(at) } else { t.prev(at) };
            let Some(next) = next else {
                return Err(format!("Search failed: \"{c}\""));
            };
            let pos = if n > 0 { at } else { next };
            if t.char_at(pos) == Some(c) {
                found += 1;
                if found == n.unsigned_abs() {
                    at = if n > 0 { next } else { pos };
                    break;
                }
            }
            at = next;
        }
        let to = if up_to {
            if n > 0 {
                t.prev(at).unwrap_or(at)
            } else {
                t.next(at).unwrap_or(at)
            }
        } else {
            at
        };
        self.kill_between(view, here, to);
        Ok(())
    }

    fn set_mark_command(&mut self, view: &View, arg: Arg) -> Result<(), String> {
        let here = self.point(view);
        let buffer = view.buffer();
        if arg.given() {
            let mark = self.mark(&buffer).ok_or("No mark set in this buffer")?;
            self.goto(view, mark);
            let mut ring = buffer.marks("mark-ring");
            if !ring.is_empty() {
                ring.push(mark);
                let next = ring.remove(0);
                buffer.set_marks("mark-ring", &ring);
                self.set_mark(view, next);
            }
            self.deactivate = true;
            return Ok(());
        }
        if self.last == Last::SetMark && self.active && self.mark(&buffer) == Some(here) {
            self.active = false;
            self.this = Last::Other;
            ui::show_message("Mark deactivated");
            return Ok(());
        }
        self.push_mark(view, here);
        self.active = true;
        self.rectangle = false;
        self.this = Last::SetMark;
        ui::show_message("Mark set");
        Ok(())
    }

    /// `M-/`: completes the word before the point from the words in the
    /// buffer that start with it, the nearest before it first, then after.
    fn dabbrev_expand(&mut self, view: &View) -> Result<(), String> {
        self.this = Last::Dabbrev;
        let here = self.point(view);
        if self.last == Last::Dabbrev
            && let Some(mut state) = self.dabbrev.take()
        {
            let end = state.start + (state.prefix.len() + state.inserted.len()) as u64;
            if state.left.is_empty() {
                let back = state.start + state.prefix.len() as u64;
                self.replace(view, back, end, "", back);
                return Err(format!(
                    "No further dynamic expansions for ‘{}’ found",
                    state.prefix
                ));
            }
            let next = state.left.remove(0);
            let tail = next[state.prefix.len()..].to_string();
            let at = state.start + state.prefix.len() as u64;
            self.replace(view, at, end, &tail, at + tail.len() as u64);
            state.inserted = tail;
            self.dabbrev = Some(state);
            return Ok(());
        }
        let doc = Doc::new(view.buffer());
        let mut t = Text::new(&doc);
        let mut start = here;
        while let Some(prev) = t.prev(start)
            && t.char_at(prev).is_some_and(|c| is_word(c) || c == '_')
        {
            start = prev;
        }
        let prefix = doc.slice(start, here);
        if prefix.is_empty() {
            return Err("No dynamic expansion for ‘’ found".into());
        }
        let buffer = view.buffer();
        let pattern = format!("{}[\\w$%]+", base_kit::regex_escape(&prefix));
        let found = buffer
            .find_all(&pattern, 0, buffer.len())
            .map_err(base_kit::error_message)?;
        let word_start = |s: u64, t: &mut Text| {
            t.prev(s)
                .is_none_or(|p| !t.char_at(p).is_some_and(|c| is_word(c) || c == '_'))
        };
        let mut before = Vec::new();
        let mut after = Vec::new();
        for found in found {
            let (s, e) = (found.start, found.end);
            if s == start || !word_start(s, &mut t) {
                continue;
            }
            let word = doc.slice(s, e);
            if s < start {
                before.push(word);
            } else {
                after.push(word);
            }
        }
        before.reverse();
        let mut left: Vec<String> = Vec::new();
        for word in before.into_iter().chain(after) {
            if word != prefix && !left.contains(&word) {
                left.push(word);
            }
        }
        if left.is_empty() {
            return Err(format!("No dynamic expansion for ‘{prefix}’ found"));
        }
        let first = left.remove(0);
        let tail = first[prefix.len()..].to_string();
        self.replace(view, here, here, &tail, here + tail.len() as u64);
        self.dabbrev = Some(Dabbrev {
            start,
            prefix,
            left,
            inserted: tail,
        });
        Ok(())
    }

    fn end_macro(&mut self) -> Result<(), String> {
        let mut keys = self.recording.take().ok_or("Not defining kbd macro")?;
        keys.truncate(self.command_start);
        self.last_macro = Some(keys);
        ui::show_message("Keyboard macro defined");
        Ok(())
    }
}

/// The char `C-q` inserts for a key: control keys as control chars.
pub fn quoted_char(ev: KeyEvent) -> Option<char> {
    Some(match ev.code {
        KeyCode::Char(c) if ev.modifiers.contains(Modifiers::CTRL) => {
            char::from_u32((c.to_ascii_uppercase() as u32) ^ 0x40).unwrap_or(c)
        }
        KeyCode::Char(c) => c,
        KeyCode::Enter => '\r',
        KeyCode::Tab => '\t',
        KeyCode::Escape => '\u{1b}',
        KeyCode::Backspace => '\u{7f}',
        _ => return None,
    })
}

fn edge(forward: bool) -> String {
    if forward {
        "End of buffer".into()
    } else {
        "Beginning of buffer".into()
    }
}

fn paragraphs(doc: &Doc, pos: u64, n: i64) -> u64 {
    let mut at = pos;
    for _ in 0..n.unsigned_abs() {
        at = if n > 0 {
            motion::forward_paragraph(doc, at)
        } else {
            motion::backward_paragraph(doc, at)
        };
    }
    at
}

/// Moves over `n` expressions, and says if it could not go on.
fn sexps(doc: &Doc, pos: u64, n: i64) -> (u64, Result<(), String>) {
    let mut t = Text::new(doc);
    let mut at = pos;
    for _ in 0..n.unsigned_abs() {
        let next = if n > 0 {
            motion::forward_sexp(&mut t, at)
        } else {
            motion::backward_sexp(&mut t, at)
        };
        match next {
            Ok(Some(next)) => at = next,
            Ok(None) => break,
            Err(err) => return (at, Err(err)),
        }
    }
    (at, Ok(()))
}

/// The function around `pos` from the syntax tree, or the one before it;
/// with `after`, the one after it if `pos` is at its end. Without a tree,
/// from one `(` in column 0 to the next.
fn defun(doc: &Doc, pos: u64, after: bool) -> Option<(u64, u64)> {
    let around = tree::object_around(&doc.buffer, "function.around", pos)
        .filter(|&(start, end)| (start < pos || !after) && (end > pos || after));
    if let Some(found) =
        around.or_else(|| tree::next_object(&doc.buffer, "function.around", pos, after, 1))
    {
        return Some(found);
    }
    let buffer = &doc.buffer;
    let before = buffer.find("^\\(", pos, true).ok().flatten();
    let start = match before {
        Some(r) if !after || r.start < pos => r.start,
        _ => buffer.find("^\\(", pos, false).ok().flatten()?.start,
    };
    let mut t = Text::new(doc);
    let end = motion::forward_sexp(&mut t, start).ok().flatten()?;
    Some((start, end))
}

#[derive(Clone, Copy)]
pub enum Case {
    Upper,
    Lower,
    Capital,
}

impl Case {
    fn of(name: &str) -> Self {
        if name.starts_with("upcase") {
            Case::Upper
        } else if name.starts_with("downcase") {
            Case::Lower
        } else {
            Case::Capital
        }
    }

    /// The text in this case. Capitalizing starts each word, counting the
    /// start of the text as a word's start.
    pub fn apply(self, text: &str) -> String {
        match self {
            Case::Upper => text.to_uppercase(),
            Case::Lower => text.to_lowercase(),
            Case::Capital => {
                let mut out = String::new();
                let mut in_word = false;
                for c in text.chars() {
                    if is_word(c) {
                        if in_word {
                            out.extend(c.to_lowercase());
                        } else {
                            out.extend(c.to_uppercase());
                        }
                        in_word = true;
                    } else {
                        out.push(c);
                        in_word = false;
                    }
                }
                out
            }
        }
    }
}

fn register_prompt(name: &str) -> &'static str {
    match name {
        "copy-to-register" => "Copy to register",
        "insert-register" => "Insert register",
        "copy-rectangle-to-register" => "Copy rectangle to register",
        "point-to-register" => "Point to register",
        "jump-to-register" => "Jump to register",
        "number-to-register" => "Number to register",
        _ => "Increment register",
    }
}

/// `C-x =`: the char at `pos`, and where it is.
fn what_cursor_position(view: &View, pos: u64) {
    let doc = Doc::new(view.buffer());
    let chars_before = doc.slice(0, pos).chars().count() + 1;
    let total = doc.slice(0, doc.len).chars().count();
    let percent = (chars_before - 1) * 100 / total.max(1);
    let column = motion::column(&doc, pos);
    let here = doc.slice(pos, doc.next_grapheme(pos)).chars().next();
    let message = match here {
        Some(c) => {
            let code = c as u32;
            let shown = match c {
                '\n' => "C-j".to_string(),
                '\t' => "TAB".to_string(),
                ' ' => "SPC".to_string(),
                c => c.to_string(),
            };
            format!(
                "Char: {shown} ({code}, #o{code:o}, #x{code:x}) point={chars_before} of {total} ({percent}%) column={column}"
            )
        }
        None => format!("point={chars_before} of {total} (EOB) column={column}"),
    };
    ui::show_message(&message);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capitalizing_starts_each_word() {
        assert_eq!(
            Case::Capital.apply("hello wORLD foo-bar"),
            "Hello World Foo-Bar"
        );
        assert_eq!(Case::Capital.apply("lo"), "Lo");
        assert_eq!(Case::Upper.apply("abc"), "ABC");
    }
}
