//! Insert and replace modes.

use base_kit::call_or_show;
use base_kit::doc::{self, Doc};
use base_kit::edit::indent_unit;
use nib_plugin::nib::plugin::types::{Edit, KeyCode, KeyEvent, Modifiers};
use nib_plugin::nib::plugin::view::{self, View};

use crate::normal::{deletion, insertion};
use crate::parse::{Action, InsertAt};
use crate::{Change, Mode, Session, Vim, ctrl, is_escape, set_points};

impl Vim {
    /// Starts insert mode with the cursors at `points`.
    pub fn start_insert(&mut self, view: &View, points: &[u64]) {
        set_points(view, points);
        let start = points.first().copied().unwrap_or(0);
        view.buffer().set_marks("insert-start", &[start]);
        self.session = Some(Session {
            cmd: None,
            keys: Vec::new(),
            autoindent: None,
            replaced: Vec::new(),
            waiting: None,
            to_corner: false,
        });
        self.set_mode(Mode::Insert);
    }

    /// Back from one command after Ctrl-o.
    pub fn back_to_insert(&mut self) {
        self.one_command = false;
        if self.session.is_some() {
            self.set_mode(Mode::Insert);
        }
    }

    pub fn insert_key(&mut self, ev: KeyEvent) {
        let view = view::active();
        if !self.dotting
            && let Some(session) = &mut self.session
        {
            session.keys.push(ev);
        }
        if let Some(waiting) = self.session.as_mut().and_then(|s| s.waiting.take()) {
            self.waited_key(&view, waiting, ev);
            return;
        }
        let replace = self.mode == Mode::Replace;
        if is_escape(&ev) {
            self.leave_insert(&view);
            return;
        }
        if let Some(c) = ctrl(&ev) {
            match c {
                'h' => self.backspace(&view),
                'w' => self.delete_before(&view, false),
                'u' => self.delete_before(&view, true),
                'j' | 'm' => self.newline(&view),
                't' | 'd' => {
                    let doc = Doc::new(view.buffer());
                    let line = doc.line_of(self.cursor(&view));
                    let before = view.buffer().len();
                    let pos = self.cursor(&view);
                    let fnb = doc::first_non_blank(&doc, pos);
                    self.shift_lines(&view, line, line, c == 't', 1);
                    // The cursor stays on the same text.
                    let grown = view.buffer().len() as i64 - before as i64;
                    let at = if pos >= fnb {
                        (pos as i64 + grown).max(0) as u64
                    } else {
                        let doc = Doc::new(view.buffer());
                        doc::first_non_blank(&doc, pos)
                    };
                    set_points(&view, &[at]);
                }
                'r' | 'v' | 'q' => {
                    if let Some(session) = &mut self.session {
                        session.waiting = Some(if c == 'q' { 'v' } else { c });
                    }
                }
                'o' => {
                    self.one_command = true;
                    self.set_mode(Mode::Normal);
                }
                'n' | 'p' => call_or_show("lsp.complete"),
                _ => {}
            }
            return;
        }
        if ev.modifiers.contains(Modifiers::ALT) {
            // Alt with a key is Escape then the key in a terminal.
            self.leave_insert(&view);
            let plain = KeyEvent {
                code: ev.code,
                modifiers: ev.modifiers - Modifiers::ALT,
            };
            self.dispatch(plain);
            return;
        }
        match ev.code {
            KeyCode::Char(c) => {
                if replace {
                    self.overwrite(&view, c);
                } else {
                    self.type_text(&view, &c.to_string());
                }
            }
            KeyCode::Enter => self.newline(&view),
            KeyCode::Tab => self.type_text(&view, &indent_unit()),
            KeyCode::Backspace => self.backspace(&view),
            KeyCode::Delete => {
                let doc = Doc::new(view.buffer());
                let edits = points(&view)
                    .into_iter()
                    .filter(|&p| p < doc.len)
                    .map(|p| deletion(p, doc.next_grapheme(p)))
                    .collect();
                self.edit(&view, edits, None);
            }
            KeyCode::Left
            | KeyCode::Right
            | KeyCode::Up
            | KeyCode::Down
            | KeyCode::Home
            | KeyCode::End => {
                // Moving starts a new undo step, as in vim.
                self.step_open = false;
                self.drop_autoindent(&view);
                let doc = Doc::new(view.buffer());
                let moved: Vec<u64> = points(&view)
                    .into_iter()
                    .map(|p| match ev.code {
                        KeyCode::Left if p > doc.line_start(doc.line_of(p)) => doc.prev_grapheme(p),
                        KeyCode::Right if p < doc.line_end(p) => doc.next_grapheme(p),
                        KeyCode::Up | KeyCode::Down => {
                            let lines = if ev.code == KeyCode::Up { -1 } else { 1 };
                            view.move_vertically(p, lines, None).map_or(p, |(p, _)| p)
                        }
                        KeyCode::Home => doc.line_start(doc.line_of(p)),
                        KeyCode::End => doc.line_end(p),
                        _ => p,
                    })
                    .collect();
                set_points(&view, &moved);
            }
            _ => {}
        }
    }

    /// The key after Ctrl-r (a register to put in) or Ctrl-v (a key to put
    /// in as it is).
    fn waited_key(&mut self, view: &View, waiting: char, ev: KeyEvent) {
        let text = if waiting == 'r' {
            let name = match ev.code {
                KeyCode::Char(c) => c,
                _ => return,
            };
            match self.registers.get(Some(name)) {
                Ok(Some(value)) => value.text,
                _ => return,
            }
        } else {
            match ev.code {
                KeyCode::Char(c) => c.to_string(),
                KeyCode::Tab => "\t".into(),
                KeyCode::Enter => "\r".into(),
                KeyCode::Escape => "\u{1b}".into(),
                _ => return,
            }
        };
        self.type_text(view, &text);
    }

    /// Types `text` at every cursor.
    pub fn type_text(&mut self, view: &View, text: &str) {
        let edits = points(view)
            .into_iter()
            .map(|p| insertion(p, text.to_string()))
            .collect();
        if let Some(session) = &mut self.session {
            session.autoindent = None;
        }
        self.edit(view, edits, None);
    }

    /// Replace mode: puts `c` over the char at each cursor, or after the
    /// end of its line.
    fn overwrite(&mut self, view: &View, c: char) {
        let doc = Doc::new(view.buffer());
        let mut replaced = Vec::new();
        let mut edits = Vec::new();
        for p in points(view) {
            let old = doc.slice(p, doc.next_grapheme(p));
            let (end, was) = if old.is_empty() || old == "\n" {
                (p, None)
            } else {
                (doc.next_grapheme(p), old.chars().next())
            };
            replaced.push((p, was));
            edits.push(Edit {
                start: p,
                end,
                text: c.to_string(),
            });
        }
        // Edits keep the cursor before the text replaced at it; it goes
        // after.
        let len = c.len_utf8() as u64;
        let mut shift = 0i64;
        let after: Vec<u64> = edits
            .iter()
            .map(|e| {
                let at = (e.start as i64 + shift) as u64 + len;
                shift += len as i64 - (e.end - e.start) as i64;
                at
            })
            .collect();
        self.edit(view, edits, Some(after));
        if let Some(session) = &mut self.session {
            session.replaced.extend(replaced);
        }
    }

    fn backspace(&mut self, view: &View) {
        if self.mode == Mode::Replace {
            // Backspace puts back what was replaced, and only moves over
            // what was before replace mode started.
            let restored = self.session.as_mut().and_then(|s| s.replaced.pop());
            let doc = Doc::new(view.buffer());
            let p = points(view)[0];
            let before = doc.prev_grapheme(p);
            match restored {
                Some((at, Some(c))) if at == before => {
                    let edit = Edit {
                        start: before,
                        end: p,
                        text: c.to_string(),
                    };
                    self.edit(view, vec![edit], Some(vec![before]));
                }
                Some((at, None)) if at == before => {
                    self.edit(view, vec![deletion(before, p)], Some(vec![before]));
                }
                _ => set_points(view, &[before]),
            }
            return;
        }
        let doc = Doc::new(view.buffer());
        let autoindent = self.session.as_ref().and_then(|s| s.autoindent);
        let edits = points(view)
            .into_iter()
            .filter(|&p| p > 0)
            .map(|p| {
                let start = doc.line_start(doc.line_of(p));
                // In autoindent, Backspace takes a whole indent unit.
                if autoindent == Some(start) && p > start {
                    let unit = indent_unit().len() as u64;
                    let from = start + (p - start - 1) / unit * unit;
                    deletion(from, p)
                } else {
                    deletion(doc.prev_grapheme(p), p)
                }
            })
            .collect();
        self.edit(view, edits, None);
    }

    /// Ctrl-w (a word) and Ctrl-u (the line) before the cursor. Both stop
    /// once where the insert started, and at the indent.
    fn delete_before(&mut self, view: &View, line: bool) {
        let doc = Doc::new(view.buffer());
        let start = view.buffer().marks("insert-start").first().copied();
        let edits = points(view)
            .into_iter()
            .filter_map(|p| {
                let line_start = doc.line_start(doc.line_of(p));
                if p == line_start {
                    return (p > 0).then(|| deletion(p - 1, p));
                }
                let fnb = doc::first_non_blank(&doc, p);
                let mut from = if line {
                    if fnb < p { fnb } else { line_start }
                } else {
                    word_start_before(&doc, p, line_start)
                };
                if let Some(start) = start
                    && start > from
                    && start < p
                {
                    from = start;
                }
                Some(deletion(from, p))
            })
            .collect();
        self.edit(view, edits, None);
    }

    /// Enter: a line break, and the indent of the line it breaks.
    fn newline(&mut self, view: &View) {
        let doc = Doc::new(view.buffer());
        let autoindent = self.session.as_ref().and_then(|s| s.autoindent);
        let cursors = points(view);
        let mut edits = Vec::new();
        let mut new_start = None;
        for &p in &cursors {
            let line_start = doc.line_start(doc.line_of(p));
            let indent = doc::indentation(&doc, p);
            let indent = &indent[..indent.len().min((p - line_start) as usize)];
            let only_indent = doc.slice(line_start, doc.line_end(p)).trim().is_empty();
            if autoindent == Some(line_start) && only_indent {
                // The indent of an empty line goes; the next line keeps it.
                edits.push(Edit {
                    start: line_start,
                    end: doc.line_end(p),
                    text: format!("\n{indent}"),
                });
                new_start = Some(line_start + 1);
            } else {
                // The blanks after the cursor go, as autoindent does.
                let mut to = p;
                while matches!(doc.slice(to, doc.next_grapheme(to)).as_str(), " " | "\t") {
                    to = doc.next_grapheme(to);
                }
                edits.push(Edit {
                    start: p,
                    end: to,
                    text: format!("\n{indent}"),
                });
                let from = p;
                new_start = Some(from + 1);
            }
        }
        let indent = doc::indentation(&doc, cursors[0]);
        self.edit(view, edits, None);
        if let Some(session) = &mut self.session {
            session.autoindent = new_start.filter(|_| !indent.is_empty() && cursors.len() == 1);
        }
    }

    /// Takes away the indent Enter, `o`, or `O` added, if nothing was
    /// typed after it.
    fn drop_autoindent(&mut self, view: &View) {
        let Some(start) = self.session.as_mut().and_then(|s| s.autoindent.take()) else {
            return;
        };
        let doc = Doc::new(view.buffer());
        if start > doc.len || doc.line_start(doc.line_of(start)) != start {
            return;
        }
        let end = doc.line_end(start);
        if end > start && doc.slice(start, end).trim().is_empty() {
            self.edit(view, vec![deletion(start, end)], Some(vec![start]));
        }
    }

    /// Escape: back to normal mode, doing what was typed again for a count,
    /// and moving the cursor back onto the text.
    fn leave_insert(&mut self, view: &View) {
        self.drop_autoindent(view);
        let (cmd, typed) = match &self.session {
            Some(session) => {
                let mut typed = session.keys.clone();
                if typed.last().is_some_and(is_escape) {
                    typed.pop();
                }
                (session.cmd.clone(), typed)
            }
            None => (None, Vec::new()),
        };
        let times = match &cmd {
            Some(cmd) if matches!(cmd.action, Action::Insert(_) | Action::ReplaceMode) => {
                cmd.count1()
            }
            _ => 1,
        };
        if times > 1 && !typed.is_empty() {
            let dotting = std::mem::replace(&mut self.dotting, true);
            for _ in 1..times {
                if let Some(Action::Insert(at @ (InsertAt::Below | InsertAt::Above))) =
                    cmd.as_ref().map(|c| &c.action)
                {
                    self.drop_autoindent(view);
                    let pos = points(view)[0];
                    self.open_line(view, pos, *at == InsertAt::Below);
                }
                self.replaying += 1;
                for &key in &typed {
                    self.insert_key(key);
                }
                self.replaying -= 1;
            }
            self.dotting = dotting;
            self.drop_autoindent(view);
        }
        let to_corner = self.session.as_ref().is_some_and(|s| s.to_corner);
        self.session = None;
        self.one_command = false;
        let doc = Doc::new(view.buffer());
        let p = points(view)[0];
        view.buffer().set_marks("^", &[p]);
        let corner = view.buffer().marks("block-corner").first().copied();
        let back = match corner {
            Some(corner) if to_corner => corner,
            _ if p > doc.line_start(doc.line_of(p)) => doc.prev_grapheme(p),
            _ => p,
        };
        self.set_mode(Mode::Normal);
        self.place(view, back);
        self.column = None;
        if !self.dotting
            && let Some(cmd) = cmd
        {
            let mut inserted = typed;
            inserted.push(KeyEvent {
                code: KeyCode::Escape,
                modifiers: Modifiers::empty(),
            });
            self.last_change = Some(Change { cmd, inserted });
        }
    }
}

/// The cursors, as points.
fn points(view: &View) -> Vec<u64> {
    let selection = view.selection();
    let mut points: Vec<u64> = selection.ranges.iter().map(|r| r.head).collect();
    // The primary first, as the cursor.
    let primary = selection.primary as usize;
    if primary < points.len() {
        points.swap(0, primary);
    }
    points
}

/// Where Ctrl-w deletes back to: past blanks, then a run of one class.
fn word_start_before(doc: &Doc, pos: u64, line_start: u64) -> u64 {
    let text = doc.slice(line_start, pos);
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut i = chars.len();
    while i > 0 && matches!(chars[i - 1].1, ' ' | '\t') {
        i -= 1;
    }
    if i > 0 {
        let class = crate::motion::char_class(chars[i - 1].1, false);
        while i > 0 && crate::motion::char_class(chars[i - 1].1, false) == class {
            i -= 1;
        }
    }
    line_start + chars.get(i).map_or(text.len(), |&(at, _)| at) as u64
}
