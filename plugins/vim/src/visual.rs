//! Visual mode: by chars, lines, or blocks. Its two ends are marks, so
//! they follow edits; the selection is drawn from them after each key.

use base_kit::doc::Doc;
use nib_plugin::nib::plugin::editor::{self, View};
use nib_plugin::nib::plugin::types::{Edit, KeyEvent, SelRange, Selection};
use nib_plugin::nib::plugin::ui;

use crate::motion;
use crate::normal::{Span, deletion, lines_span, text_object};
use crate::parse::{Action, Cmd, Op, Parsed, VisualKind, parse};
use crate::register::{Shape, Value, Why};
use crate::{Mode, Vim, is_escape, set_points};

impl Vim {
    pub fn start_visual(&mut self, view: &View, kind: VisualKind, pos: u64) {
        match self.mode {
            Mode::Visual(current) if current == kind => {
                self.leave_visual(view);
                return;
            }
            Mode::Visual(_) => {}
            _ => view.buffer().set_marks("visual", &[pos, pos]),
        }
        self.set_mode(Mode::Visual(kind));
        self.draw_visual(view);
    }

    /// The visual area's two ends: where it started, and the cursor.
    fn ends(&self, view: &View) -> (u64, u64) {
        match view.buffer().marks("visual")[..] {
            [anchor, cursor] => (anchor, cursor),
            _ => {
                let pos = self.cursor(view);
                (pos, pos)
            }
        }
    }

    pub fn move_visual(&mut self, view: &View, pos: u64) {
        let (anchor, _) = self.ends(view);
        let doc = Doc::new(view.buffer());
        let pos = crate::clamp_normal(&doc, pos);
        view.buffer().set_marks("visual", &[anchor, pos]);
        self.draw_visual(view);
    }

    /// Selects what is between the ends.
    pub fn draw_visual(&self, view: &View) {
        let Mode::Visual(kind) = self.mode else {
            return;
        };
        let doc = Doc::new(view.buffer());
        let (anchor, cursor) = self.ends(view);
        let forward = cursor >= anchor;
        let (ranges, primary) = match kind {
            VisualKind::Chars => {
                let (from, to) = (anchor.min(cursor), anchor.max(cursor));
                let end = doc.next_grapheme(to).max(to + 1).min(doc.len.max(to + 1));
                let end = end.min(doc.len);
                let r = if forward {
                    SelRange {
                        anchor: from,
                        head: end,
                    }
                } else {
                    SelRange {
                        anchor: end,
                        head: from,
                    }
                };
                (vec![r], 0)
            }
            VisualKind::Lines => {
                let span = lines_span(&doc, anchor.min(cursor), anchor.max(cursor));
                let r = if forward {
                    SelRange {
                        anchor: span.start,
                        head: span.end,
                    }
                } else {
                    SelRange {
                        anchor: span.end,
                        head: span.start,
                    }
                };
                (vec![r], 0)
            }
            VisualKind::Block => {
                let rows = self.block_rows(view, &doc);
                let cursor_line = doc.line_of(cursor);
                let first = doc.line_of(anchor.min(cursor));
                let ranges = rows
                    .iter()
                    .map(|&(start, end)| SelRange {
                        anchor: start,
                        head: end,
                    })
                    .collect();
                (ranges, (cursor_line - first) as u32)
            }
        };
        let _ = view.set_selection(&Selection { ranges, primary });
    }

    /// The block's part of each line it covers, `start..end`; empty where a
    /// line is too short to reach it.
    fn block_rows(&self, view: &View, doc: &Doc) -> Vec<(u64, u64)> {
        let (anchor, cursor) = self.ends(view);
        let column = |pos: u64| view.move_vertically(pos, 0, None).map_or(0, |(_, c)| c);
        let (a, c) = (column(anchor), column(cursor));
        let left = a.min(c);
        let right = if self.column == Some(u32::MAX) {
            u32::MAX
        } else {
            a.max(c)
        };
        let (first, last) = (
            doc.line_of(anchor.min(cursor)),
            doc.line_of(anchor.max(cursor)),
        );
        (first..=last)
            .map(|line| {
                let start = doc.line_start(line);
                let end_of_line = doc.line_end(start);
                let at = |col: u32| {
                    view.move_vertically(start, 0, Some(col))
                        .map_or(start, |(p, _)| p)
                };
                let from = at(left);
                let to = at(right);
                let to = if to < end_of_line {
                    doc.next_grapheme(to)
                } else {
                    end_of_line
                };
                (from.min(end_of_line), to.max(from).min(end_of_line))
            })
            .collect()
    }

    /// Back to normal mode, remembering the area for `gv` and `'<`.
    pub fn leave_visual(&mut self, view: &View) {
        let Mode::Visual(kind) = self.mode else {
            return;
        };
        let (anchor, cursor) = self.ends(view);
        let buffer = view.buffer();
        buffer.set_marks("<", &[anchor.min(cursor)]);
        buffer.set_marks(">", &[anchor.max(cursor)]);
        buffer.set_marks("last-visual", &[anchor, cursor]);
        buffer.set_marks("visual", &[]);
        self.last_visual = Some(kind);
        self.set_mode(Mode::Normal);
        self.place(view, cursor);
    }

    /// `gv`: the last visual area again.
    pub fn reselect(&mut self, view: &View) {
        let buffer = view.buffer();
        let (Some(kind), [anchor, cursor]) = (self.last_visual, &buffer.marks("last-visual")[..])
        else {
            return;
        };
        let (anchor, cursor) = (*anchor, *cursor);
        if let Mode::Visual(_) = self.mode {
            self.leave_visual(view);
        }
        buffer.set_marks("visual", &[anchor, cursor]);
        self.set_mode(Mode::Visual(kind));
        self.draw_visual(view);
    }

    pub fn visual_key(&mut self, ev: KeyEvent, kind: VisualKind) -> bool {
        let view = editor::active_view();
        if is_escape(&ev) {
            self.keys.clear();
            self.leave_visual(&view);
            return true;
        }
        self.keys.push(ev);
        match parse(&self.keys, true) {
            Parsed::Need => {}
            Parsed::Bad => self.keys.clear(),
            Parsed::Done(cmd) => {
                self.keys.clear();
                self.step_open = false;
                self.run_visual(&view, &cmd, kind);
            }
        }
        true
    }

    fn run_visual(&mut self, view: &View, cmd: &Cmd, kind: VisualKind) {
        let n = cmd.count1();
        match &cmd.action {
            Action::Move(motion) => self.move_to(view, motion, cmd.count),
            Action::Visual(new) => {
                let pos = self.cursor(view);
                self.start_visual(view, *new, pos);
            }
            Action::Reselect => self.reselect(view),
            Action::Select { around, c } => {
                let doc = Doc::new(view.buffer());
                let pos = self.cursor(view);
                if let Some(obj) = text_object(&doc, pos, *c, *around, n) {
                    let last = doc.prev_grapheme(obj.end).max(obj.start);
                    view.buffer().set_marks("visual", &[obj.start, last]);
                    if obj.linewise && kind == VisualKind::Chars {
                        self.set_mode(Mode::Visual(VisualKind::Lines));
                    }
                    self.draw_visual(view);
                }
            }
            Action::OnSelection(c) => self.on_selection(view, *c, kind, cmd),
            Action::OnSelectionG(c) => {
                let op = match c {
                    '~' => Op::Toggle,
                    'u' => Op::Lower,
                    'U' => Op::Upper,
                    'J' => {
                        self.join_selection(view, false);
                        return;
                    }
                    _ => return,
                };
                self.operate_selection(view, op, kind, cmd);
            }
            Action::ReplaceSelection(c) => self.replace_selection(view, *c, kind),
            Action::Scroll(_) => self.execute(view, cmd),
            Action::SetMark(c) => {
                let pos = self.cursor(view);
                self.set_mark(view, *c, pos);
            }
            _ => {}
        }
    }

    /// The area as one span, taking whole lines in line mode.
    fn span(&self, view: &View, kind: VisualKind) -> Span {
        let doc = Doc::new(view.buffer());
        let (anchor, cursor) = self.ends(view);
        let (from, to) = (anchor.min(cursor), anchor.max(cursor));
        match kind {
            VisualKind::Lines => lines_span(&doc, from, to),
            _ => Span {
                start: from,
                end: doc.next_grapheme(to).min(doc.len).max(from),
                linewise: false,
                from,
            },
        }
    }

    fn on_selection(&mut self, view: &View, c: char, kind: VisualKind, cmd: &Cmd) {
        match c {
            'd' | 'x' => self.operate_selection(view, Op::Delete, kind, cmd),
            'X' | 'D' => self.operate_selection(view, Op::Delete, VisualKind::Lines, cmd),
            'y' => self.operate_selection(view, Op::Yank, kind, cmd),
            'Y' => self.operate_selection(view, Op::Yank, VisualKind::Lines, cmd),
            'c' | 's' => self.operate_selection(view, Op::Change, kind, cmd),
            'C' | 'S' | 'R' => self.operate_selection(view, Op::Change, VisualKind::Lines, cmd),
            '>' => self.operate_selection(view, Op::Indent, kind, cmd),
            '<' => self.operate_selection(view, Op::Unindent, kind, cmd),
            '~' => self.operate_selection(view, Op::Toggle, kind, cmd),
            'u' => self.operate_selection(view, Op::Lower, kind, cmd),
            'U' => self.operate_selection(view, Op::Upper, kind, cmd),
            'J' => self.join_selection(view, true),
            'o' | 'O' => {
                let (anchor, cursor) = self.ends(view);
                view.buffer().set_marks("visual", &[cursor, anchor]);
                self.draw_visual(view);
            }
            'p' | 'P' => self.put_over_selection(view, kind, cmd, c == 'p'),
            'I' | 'A' => self.insert_on_selection(view, kind, c == 'A'),
            ':' => {
                self.leave_visual(view);
                self.open_command("'<,'>");
            }
            _ => {}
        }
    }

    fn operate_selection(&mut self, view: &View, op: Op, kind: VisualKind, cmd: &Cmd) {
        if kind == VisualKind::Block && !matches!(op, Op::Indent | Op::Unindent) {
            self.operate_block(view, op, cmd);
            return;
        }
        let span = self.span(view, kind);
        self.leave_visual(view);
        self.place(view, span.start);
        self.operate(view, op, span, cmd);
    }

    /// An operator on a block: each line's part of it.
    fn operate_block(&mut self, view: &View, op: Op, cmd: &Cmd) {
        let doc = Doc::new(view.buffer());
        let rows = self.block_rows(view, &doc);
        let top_left = rows.first().map_or(0, |r| r.0);
        let text: Vec<String> = rows.iter().map(|&(s, e)| doc.slice(s, e)).collect();
        self.leave_visual(view);
        let value = Value {
            text: text.join("\n"),
            shape: Shape::Block,
        };
        match op {
            Op::Yank => {
                if let Err(err) = self.registers.store(cmd.register, value, Why::Yank) {
                    ui::show_message(&err);
                }
                self.place(view, top_left);
            }
            Op::Delete | Op::Change => {
                if let Err(err) =
                    self.registers
                        .store(cmd.register, value, Why::Delete { big: true })
                {
                    ui::show_message(&err);
                    return;
                }
                let edits: Vec<Edit> = rows
                    .iter()
                    .filter(|(s, e)| s < e)
                    .map(|&(s, e)| deletion(s, e))
                    .collect();
                self.edit(view, edits, None);
                if op == Op::Change {
                    // The same place on each line, after the deletions.
                    let mut shift = 0u64;
                    let points: Vec<u64> = rows
                        .iter()
                        .map(|&(s, e)| {
                            let at = s - shift;
                            shift += e - s;
                            at
                        })
                        .collect();
                    self.start_insert(view, &points);
                } else {
                    self.place(view, top_left);
                }
            }
            Op::Lower | Op::Upper | Op::Toggle => {
                let edits: Vec<Edit> = rows
                    .iter()
                    .zip(&text)
                    .filter(|(r, _)| r.0 < r.1)
                    .map(|(&(s, e), t)| Edit {
                        start: s,
                        end: e,
                        text: t
                            .chars()
                            .map(|c| match op {
                                Op::Lower => c.to_lowercase().collect::<String>(),
                                Op::Upper => c.to_uppercase().collect(),
                                _ if c.is_lowercase() => c.to_uppercase().collect(),
                                _ => c.to_lowercase().collect(),
                            })
                            .collect(),
                    })
                    .collect();
                self.edit(view, edits, None);
                self.place(view, top_left);
            }
            Op::Indent | Op::Unindent => {}
        }
    }

    fn replace_selection(&mut self, view: &View, c: char, kind: VisualKind) {
        let doc = Doc::new(view.buffer());
        let rows = match kind {
            VisualKind::Block => self.block_rows(view, &doc),
            _ => {
                let span = self.span(view, kind);
                vec![(span.start, span.end)]
            }
        };
        let start = rows.first().map_or(0, |r| r.0);
        self.leave_visual(view);
        let edits: Vec<Edit> = rows
            .iter()
            .filter(|(s, e)| s < e)
            .map(|&(s, e)| Edit {
                start: s,
                end: e,
                text: doc
                    .slice(s, e)
                    .chars()
                    .map(|ch| if ch == '\n' { '\n' } else { c })
                    .collect(),
            })
            .collect();
        self.edit(view, edits, None);
        self.place(view, start);
    }

    fn join_selection(&mut self, view: &View, spaces: bool) {
        let doc = Doc::new(view.buffer());
        let (anchor, cursor) = self.ends(view);
        let (first, last) = (
            doc.line_of(anchor.min(cursor)),
            doc.line_of(anchor.max(cursor)),
        );
        self.leave_visual(view);
        let start = doc.line_start(first);
        let cmd = Cmd {
            register: None,
            count: Some((last - first + 1).max(2)),
            action: Action::Join { spaces },
        };
        self.place(view, start);
        self.run(cmd);
    }

    /// `p` and `P`: the register's text in place of the area. `p` keeps
    /// what was there in the unnamed register.
    fn put_over_selection(&mut self, view: &View, kind: VisualKind, cmd: &Cmd, keep: bool) {
        let value = match self.registers.get(cmd.register) {
            Ok(Some(value)) => value,
            _ => return,
        };
        let span = self.span(view, kind);
        let doc = Doc::new(view.buffer());
        let old = Value {
            text: doc.slice(span.start, span.end),
            shape: if span.linewise {
                Shape::Lines
            } else {
                Shape::Chars
            },
        };
        self.leave_visual(view);
        let text = match (span.linewise, value.shape) {
            (true, Shape::Lines) | (false, Shape::Chars) => value.text.clone(),
            (true, _) => format!("{}\n", value.text),
            (false, _) => format!("\n{}", value.text),
        };
        let edit = Edit {
            start: span.start,
            end: span.end,
            text: text.clone(),
        };
        self.edit(view, vec![edit], Some(vec![span.start]));
        if keep {
            let big = old.shape == Shape::Lines || old.text.contains('\n');
            let _ = self.registers.store(None, old, Why::Delete { big });
        }
        let doc = Doc::new(view.buffer());
        let pos = if value.shape == Shape::Lines {
            motion::first_non_blank(&doc, span.start + (!span.linewise) as u64)
        } else if text.contains('\n') {
            span.start
        } else {
            doc.prev_grapheme(span.start + text.len() as u64)
                .max(span.start)
        };
        self.place(view, pos);
    }

    /// `I` and `A`: insert mode before or after the area, or on each line
    /// of a block, where `A` pads short lines to reach it. Leaving insert
    /// mode goes back to the block's top-left corner.
    fn insert_on_selection(&mut self, view: &View, kind: VisualKind, after: bool) {
        let doc = Doc::new(view.buffer());
        let span = self.span(view, kind);
        let (points, corner) = match kind {
            VisualKind::Block => {
                let rows = self.block_rows(view, &doc);
                let corner = rows.first().map_or(0, |r| r.0);
                let points = if after {
                    self.pad_block(view, &doc, &rows)
                } else {
                    rows.iter().map(|&(s, _)| s).collect()
                };
                (points, Some(corner))
            }
            // Other areas insert at the start of a line, as vim does.
            VisualKind::Lines if after => (
                vec![doc.line_end(span.end.saturating_sub(1).max(span.start))],
                None,
            ),
            VisualKind::Lines => (vec![span.start], None),
            VisualKind::Chars if after => (
                vec![span.end.min(doc.line_end(span.end.saturating_sub(1)))],
                None,
            ),
            VisualKind::Chars => (vec![doc.line_start(doc.line_of(span.start))], None),
        };
        self.leave_visual(view);
        if let Some(corner) = corner {
            view.buffer().set_marks("block-corner", &[corner]);
        }
        set_points(view, &points);
        self.start_insert(view, &points);
        if let Some(session) = &mut self.session {
            session.to_corner = corner.is_some();
        }
    }

    /// Adds spaces to the lines too short to reach the block's end, and
    /// returns where `A` inserts on each line.
    fn pad_block(&mut self, view: &View, doc: &Doc, rows: &[(u64, u64)]) -> Vec<u64> {
        let (anchor, cursor) = self.ends(view);
        let column = |pos: u64| view.move_vertically(pos, 0, None).map_or(0, |(_, c)| c);
        let right = column(anchor).max(column(cursor)) + 1;
        let mut edits = Vec::new();
        for &(start, _) in rows {
            let line_start = doc.line_start(doc.line_of(start));
            let end = doc.line_end(start);
            let width = column(end);
            if width < right {
                edits.push(Edit {
                    start: end,
                    end,
                    text: " ".repeat((right - width) as usize),
                });
            }
            let _ = line_start;
        }
        self.edit(view, edits, None);
        let doc = Doc::new(view.buffer());
        let first = doc.line_of(rows.first().map_or(0, |r| r.0));
        (0..rows.len() as u64)
            .map(|i| {
                let line_start = doc.line_start(first + i);
                view.move_vertically(line_start, 0, Some(right))
                    .map_or(line_start, |(p, _)| p)
            })
            .collect()
    }
}
