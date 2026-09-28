//! Normal mode: running commands, where motions go, and operators.

use base_kit::doc::{Doc, FindKind};
use base_kit::edit::indent_unit;
use base_kit::{call_or_show, regex_escape};
use nib_plugin::nib::plugin::prompt as prompts;
use nib_plugin::nib::plugin::settings;
use nib_plugin::nib::plugin::types::{Edit, KeyEvent};
use nib_plugin::nib::plugin::ui;
use nib_plugin::nib::plugin::view::{self, Direction, ScrollAmount, Toward, View};

use crate::motion::{self, Kind};
use crate::object::{self, Object};
use crate::parse::{Action, Cmd, InsertAt, Motion, Op, Parsed, Scroll, Target, parse};
use crate::register::{Shape, Value, Why};
use crate::{Change, Mode, Vim, clamp_normal, is_escape, plain};
use base_kit::text::Text;

/// A range of text an operator works on.
#[derive(Clone, Copy, Debug)]
pub struct Span {
    pub start: u64,
    pub end: u64,
    pub linewise: bool,
    /// Where the text is taken from for the cursor afterwards, before it was
    /// grown to whole lines.
    pub from: u64,
}

impl Vim {
    pub fn normal_key(&mut self, ev: KeyEvent) -> bool {
        if self.keys.is_empty() {
            // Other plugins may have left a selection, or the cursor on a
            // line break; normal mode starts from a point on a char.
            let view = view::active();
            let pos = self.cursor(&view);
            let doc = Doc::new(view.buffer());
            let selection = view.selection();
            let point = selection.ranges.len() == 1
                && selection.ranges[0].anchor == selection.ranges[0].head;
            if !point || clamp_normal(&doc, pos) != pos {
                self.place(&view, pos);
            }
            if is_escape(&ev) {
                if self.one_command {
                    self.back_to_insert();
                }
                return true;
            }
            if plain(&ev) == Some('q')
                && let Some((register, mut keys)) = self.recording.take()
            {
                keys.pop();
                self.macros.insert(register, keys);
                return true;
            }
        } else if is_escape(&ev) {
            self.keys.clear();
            return true;
        }
        self.keys.push(ev);
        match parse(&self.keys, false) {
            Parsed::Need => {}
            Parsed::Bad => self.keys.clear(),
            Parsed::Done(cmd) => {
                self.keys.clear();
                self.run(cmd);
            }
        }
        true
    }

    /// Runs a command of normal mode, and keeps it for `.` if it changed
    /// the text.
    pub fn run(&mut self, cmd: Cmd) {
        let view = view::active();
        let version = view.buffer().version();
        self.step_open = false;
        let change = is_change(&cmd.action);
        self.execute(&view, &cmd);
        let view = view::active();
        match self.mode {
            Mode::Insert | Mode::Replace => {
                if change && let Some(session) = &mut self.session {
                    session.cmd = Some(cmd);
                }
                return;
            }
            Mode::Normal => {
                if change && !self.dotting && view.buffer().version() != version {
                    self.last_change = Some(Change {
                        cmd,
                        inserted: Vec::new(),
                    });
                }
                let pos = self.cursor(&view);
                self.place(&view, pos);
            }
            Mode::Visual(_) => {}
        }
        if self.one_command && self.mode == Mode::Normal && prompts::active().is_none() {
            self.back_to_insert();
        }
    }

    pub fn execute(&mut self, view: &View, cmd: &Cmd) {
        let count = cmd.count;
        let n = cmd.count1();
        let pos = self.cursor(view);
        match &cmd.action {
            Action::Move(motion) => self.move_to(view, motion, count),
            Action::Operate(op, target) => {
                let span = match target {
                    Target::Line => self.line_span(view, pos, n),
                    Target::Object { around, c } => self.object_span(view, pos, *c, *around, n),
                    Target::Motion(Motion::SearchPrompt { backward }) => {
                        self.open_search(*backward, Some(cmd.clone()));
                        return;
                    }
                    Target::Motion(motion) => self.motion_span(view, pos, motion, count, *op),
                };
                if let Some(span) = span {
                    self.operate(view, *op, span, cmd);
                }
            }
            Action::DeleteChar => self.run_as(view, cmd, Op::Delete, Motion::Right),
            Action::DeleteCharBack => self.run_as(view, cmd, Op::Delete, Motion::Left),
            Action::Substitute => self.run_as(view, cmd, Op::Change, Motion::Right),
            Action::ChangeToEnd => self.run_as(view, cmd, Op::Change, Motion::LineEnd),
            Action::DeleteToEnd => self.run_as(view, cmd, Op::Delete, Motion::LineEnd),
            Action::YankToEnd => self.run_as(view, cmd, Op::Yank, Motion::LineEnd),
            Action::SubstituteLine => {
                if let Some(span) = self.line_span(view, pos, n) {
                    self.operate(view, Op::Change, span, cmd);
                }
            }
            Action::Put { before } => self.put(view, cmd.register, *before, n),
            Action::Join { spaces } => self.join(view, pos, n.max(2) - 1, *spaces),
            Action::Replace(c) => self.replace_chars(view, pos, *c, n),
            Action::ReplaceMode => {
                self.start_insert(view, &[pos]);
                self.set_mode(Mode::Replace);
            }
            Action::Insert(at) => self.insert_at(view, pos, *at),
            Action::Undo => self.undo(view, n, false),
            Action::Redo => self.undo(view, n, true),
            Action::Repeat => self.repeat(count),
            Action::ToggleCase => self.toggle_case(view, pos, n),
            Action::Visual(kind) => self.start_visual(view, *kind, pos),
            Action::Reselect => self.reselect(view),
            Action::CommandLine => {
                let range = match count {
                    Some(1) => ".".to_string(),
                    Some(n) => format!(".,.+{}", n - 1),
                    None => String::new(),
                };
                self.open_command(&range);
            }
            Action::SetMark(c) => self.set_mark(view, *c, pos),
            Action::Record(c) => {
                if c.is_ascii_alphanumeric() || *c == '"' {
                    self.recording = Some((c.to_ascii_lowercase(), Vec::new()));
                }
            }
            Action::Play(c) => self.play(*c, n),
            Action::JumpBack => self.jump(view, -(n as i64)),
            Action::JumpForward => self.jump(view, n as i64),
            Action::Scroll(scroll) => self.scroll(view, *scroll, count),
            Action::Increment(sign) => self.increment(view, pos, *sign * n as i64),
            Action::Definition => call_or_show("lsp.definition"),
            Action::Hover => call_or_show("lsp.hover"),
            Action::Window(c) => window(*c),
            Action::WriteQuit => self.run_ex_line("x"),
            Action::QuitForce => self.run_ex_line("q!"),
            Action::RepeatSubstitute => self.run_ex_line("&&"),
            Action::Select { .. }
            | Action::OnSelection(_)
            | Action::OnSelectionG(_)
            | Action::ReplaceSelection(_) => {}
        }
    }

    /// `x`, `D`, and the like: an operator with a motion, and the count.
    fn run_as(&mut self, view: &View, cmd: &Cmd, op: Op, motion: Motion) {
        let pos = self.cursor(view);
        if let Some(span) = self.motion_span(view, pos, &motion, cmd.count, op) {
            self.operate(view, op, span, cmd);
        }
    }

    /// Moves the cursor as `motion` says, remembering where it jumped from.
    pub fn move_to(&mut self, view: &View, motion: &Motion, count: Option<u64>) {
        if let Motion::SearchPrompt { backward } = motion {
            self.open_search(*backward, None);
            return;
        }
        let pos = self.cursor(view);
        let Some((target, _)) = self.target(view, pos, motion, count, false) else {
            return;
        };
        if is_jump(motion) {
            self.push_jump(view, pos);
        }
        self.go(view, target);
    }

    /// Puts the cursor at `pos`, or in visual mode, its moving end.
    pub fn go(&mut self, view: &View, pos: u64) {
        match self.mode {
            Mode::Visual(_) => self.move_visual(view, pos),
            _ => self.place(view, pos),
        }
    }

    /// Where `motion` goes from `pos`, and how an operator takes it. `None`
    /// when it cannot move, which also cancels an operator and stops a
    /// macro.
    pub fn target(
        &mut self,
        view: &View,
        pos: u64,
        motion: &Motion,
        count: Option<u64>,
        for_op: bool,
    ) -> Option<(u64, Kind)> {
        let found = self.find_target(view, pos, motion, count, for_op);
        if found.is_none() {
            self.failed = true;
        }
        found
    }

    fn find_target(
        &mut self,
        view: &View,
        pos: u64,
        motion: &Motion,
        count: Option<u64>,
        for_op: bool,
    ) -> Option<(u64, Kind)> {
        let doc = Doc::new(view.buffer());
        let mut t = Text::new(&doc);
        let n = count.unwrap_or(1);
        let line = doc.line_of(pos);
        let line_start = doc.line_start(line);
        let line_end = doc.line_end(pos);
        let vertical = matches!(motion, Motion::Up | Motion::Down)
            || matches!(motion, Motion::FileStart | Motion::FileEnd);
        let keeps_column = vertical || matches!(motion, Motion::LineEnd);
        if !keeps_column {
            self.column = None;
        }
        match motion {
            Motion::Left => {
                let mut p = pos;
                for _ in 0..n {
                    if p <= line_start {
                        break;
                    }
                    p = doc.prev_grapheme(p);
                }
                (p != pos || for_op).then_some((p, Kind::Exclusive))
            }
            Motion::Right => {
                // Normal mode stays on the text; an operator may reach the
                // line's end.
                let limit = if for_op || matches!(self.mode, Mode::Visual(_)) {
                    line_end
                } else {
                    clamp_normal(&doc, line_end)
                };
                let mut p = pos;
                for _ in 0..n {
                    if p >= limit {
                        break;
                    }
                    p = doc.next_grapheme(p).min(limit);
                }
                (p != pos || for_op).then_some((p, Kind::Exclusive))
            }
            Motion::Down | Motion::Up | Motion::NextLine | Motion::PrevLine => {
                let down = matches!(motion, Motion::Down | Motion::NextLine);
                let last = doc.last_line();
                let to = if down {
                    (line + n).min(last)
                } else {
                    line.saturating_sub(n)
                };
                if to == line {
                    return None;
                }
                let p = if matches!(motion, Motion::Down | Motion::Up) {
                    self.at_column(view, &doc, pos, to)
                } else {
                    motion::first_non_blank(&doc, doc.line_start(to))
                };
                Some((p, Kind::Linewise))
            }
            Motion::CurrentLine => {
                let to = (line + n - 1).min(doc.last_line());
                Some((
                    motion::first_non_blank(&doc, doc.line_start(to)),
                    Kind::Linewise,
                ))
            }
            Motion::WordStart(big) => {
                let p = match motion::word_start(&mut t, pos, *big, n, for_op) {
                    Ok(p) | Err(p) => p,
                };
                (p != pos).then_some((p, Kind::Exclusive))
            }
            Motion::WordEnd(big) => {
                motion::word_end(&mut t, pos, *big, n, false).map(|p| (p, Kind::Inclusive))
            }
            Motion::WordBack(big) => {
                motion::word_back(&mut t, pos, *big, n).map(|p| (p, Kind::Exclusive))
            }
            Motion::WordEndBack(big) => {
                motion::word_end_back(&mut t, pos, *big, n).map(|p| (p, Kind::Inclusive))
            }
            Motion::LineStart => Some((line_start, Kind::Exclusive)),
            Motion::FirstNonBlank => Some((motion::first_non_blank(&doc, pos), Kind::Exclusive)),
            Motion::LineEnd => {
                self.column = Some(u32::MAX);
                let to = (line + n - 1).min(doc.last_line());
                let end = doc.line_end(doc.line_start(to));
                // Through the last char: to the line's end, left out.
                Some((end, Kind::Exclusive))
            }
            Motion::LastNonBlank => {
                let to = (line + n - 1).min(doc.last_line());
                let p = motion::last_non_blank(&mut t, doc.line_start(to));
                Some((p, Kind::Inclusive))
            }
            Motion::FileStart | Motion::FileEnd => {
                let to = match (count, motion) {
                    (Some(n), _) => (n.max(1) - 1).min(doc.last_line()),
                    (None, Motion::FileStart) => 0,
                    (None, _) => doc.last_line(),
                };
                Some((self.at_column(view, &doc, pos, to), Kind::Linewise))
            }
            Motion::Column => {
                let (p, _) = view
                    .move_vertically(line_start, 0, Some((n - 1) as u32))
                    .ok()?;
                self.column = Some((n - 1) as u32);
                Some((p.min(line_end), Kind::Exclusive))
            }
            Motion::Find(kind, c) => {
                self.last_find = Some((*kind, *c));
                find(&mut t, pos, *kind, *c, n, false)
            }
            Motion::RepeatFind(reverse) => {
                let (kind, c) = self.last_find?;
                let kind = if *reverse { reversed(kind) } else { kind };
                find(&mut t, pos, kind, c, n, true)
            }
            Motion::Bracket => match count {
                Some(percent) => {
                    if percent > 100 {
                        return None;
                    }
                    let lines = doc.last_line() + 1;
                    let to = ((percent * lines).div_ceil(100)).max(1) - 1;
                    Some((
                        motion::first_non_blank(&doc, doc.line_start(to)),
                        Kind::Linewise,
                    ))
                }
                None => motion::bracket(&doc, &mut t, pos).map(|p| (p, Kind::Inclusive)),
            },
            Motion::ParagraphForward | Motion::ParagraphBack => {
                let forward = *motion == Motion::ParagraphForward;
                let mut p = pos;
                for _ in 0..n {
                    p = motion::paragraph(&doc, p, forward);
                }
                (p != pos).then_some((p, Kind::Exclusive))
            }
            Motion::ScreenTop | Motion::ScreenMiddle | Motion::ScreenBottom => {
                let shown = view.visible_range();
                let (start, end) = (shown.start, shown.end);
                let top = doc.line_of(start);
                let bottom = doc
                    .line_of(end.saturating_sub(1).max(start))
                    .min(doc.last_line());
                let to = match motion {
                    Motion::ScreenTop => (top + n - 1).min(bottom),
                    Motion::ScreenBottom => bottom.saturating_sub(n - 1).max(top),
                    _ => (top + bottom) / 2,
                };
                Some((
                    motion::first_non_blank(&doc, doc.line_start(to)),
                    Kind::Linewise,
                ))
            }
            Motion::SearchNext(reverse) => {
                let Some((pattern, backward)) = self.search.clone() else {
                    ui::show_message("E35: No previous regular expression");
                    return None;
                };
                self.search_from(view, pos, &pattern, backward != *reverse, n)
                    .map(|p| (p, Kind::Exclusive))
            }
            Motion::Star { forward, whole } => {
                let word = word_under(&mut t, pos)?;
                let escaped = regex_escape(&word).replace('/', "\\/");
                let pattern = if *whole && word.chars().all(crate::text::is_keyword) {
                    format!("\\<{escaped}\\>")
                } else {
                    escaped
                };
                self.search = Some((pattern.clone(), !forward));
                self.remember('/', &pattern);
                self.search_from(view, pos, &pattern, !forward, n)
                    .map(|p| (p, Kind::Exclusive))
            }
            Motion::Search { backward, pattern } => self
                .search_from(view, pos, pattern, *backward, n)
                .map(|p| (p, Kind::Exclusive)),
            Motion::SearchPrompt { .. } => None,
            Motion::Mark { name, exact } => {
                let p = self.mark(view, *name)?;
                let view = view::active();
                let doc = Doc::new(view.buffer());
                if *exact {
                    Some((p.min(doc.len), Kind::Exclusive))
                } else {
                    Some((motion::first_non_blank(&doc, p), Kind::Linewise))
                }
            }
        }
    }

    /// The position on line `to` at the column the cursor keeps.
    fn at_column(&mut self, view: &View, doc: &Doc, pos: u64, to: u64) -> u64 {
        let line = doc.line_of(pos) as i64;
        match view.move_vertically(pos, (to as i64 - line) as i32, self.column) {
            Ok((p, column)) => {
                self.column = Some(column);
                clamp_normal(doc, p)
            }
            Err(_) => pos,
        }
    }

    /// The text `motion` from `pos` covers for `op`, with vim's rules for
    /// where exclusive motions end.
    fn motion_span(
        &mut self,
        view: &View,
        pos: u64,
        motion: &Motion,
        count: Option<u64>,
        op: Op,
    ) -> Option<Span> {
        let doc = Doc::new(view.buffer());
        // `cw` on a word changes to its end, as `ce` would, but not past it.
        let cw = op == Op::Change && matches!(motion, Motion::WordStart(_));
        if cw {
            let mut t = Text::new(&doc);
            let c = t.char_at(pos);
            if c.is_some_and(|c| c != '\n' && c != ' ' && c != '\t') {
                let big = matches!(motion, Motion::WordStart(true));
                let end = motion::word_end(&mut t, pos, big, count.unwrap_or(1), true)?;
                return Some(Span {
                    start: pos,
                    end: doc.next_grapheme(end),
                    linewise: false,
                    from: pos,
                });
            }
        }
        let (target, kind) = self.target(view, pos, motion, count, true)?;
        let view = view::active();
        let doc = Doc::new(view.buffer());
        let (from, to) = (pos.min(target), pos.max(target));
        let span = match kind {
            Kind::Linewise => lines_span(&doc, from, to),
            Kind::Inclusive => Span {
                start: from,
                end: doc
                    .next_grapheme(to)
                    .min(doc.line_end(to).max(to + 1))
                    .min(doc.len),
                linewise: false,
                from,
            },
            Kind::Exclusive => {
                let (start_line, end_line) = (doc.line_of(from), doc.line_of(to));
                let at_line_start = to == doc.line_start(end_line);
                if at_line_start && end_line > start_line {
                    // Ending at the start of a line, it ends at the end of
                    // the line before; from the indent, it takes lines.
                    let in_indent = from <= motion::first_non_blank(&doc, from);
                    if in_indent {
                        let mut span = lines_span(&doc, from, doc.line_start(end_line - 1));
                        span.from = from;
                        span
                    } else {
                        Span {
                            start: from,
                            end: doc.line_end(doc.line_start(end_line - 1)),
                            linewise: false,
                            from,
                        }
                    }
                } else {
                    Span {
                        start: from,
                        end: to,
                        linewise: false,
                        from,
                    }
                }
            }
        };
        // A deletion from the indent to the end of a line takes the lines.
        let mut span = span;
        if op == Op::Delete && !span.linewise && doc.line_of(span.start) != doc.line_of(span.end) {
            let rest = doc.slice(span.end, doc.line_end(span.end));
            let in_indent = span.start <= motion::first_non_blank(&doc, span.start);
            if rest.trim().is_empty() && in_indent {
                span = lines_span(&doc, span.start, span.end);
                span.from = from;
            }
        }
        Some(span)
    }

    /// `n` lines from the line of `pos`, as `dd` takes them. Fails when
    /// there are not as many lines, unless it can take some.
    fn line_span(&mut self, view: &View, pos: u64, n: u64) -> Option<Span> {
        let doc = Doc::new(view.buffer());
        let line = doc.line_of(pos);
        let last = doc.last_line();
        if n > 1 && line >= last {
            return None;
        }
        let end_line = (line + n - 1).min(last);
        let mut span = lines_span(&doc, pos, doc.line_start(end_line));
        span.from = pos;
        Some(span)
    }

    fn object_span(
        &mut self,
        view: &View,
        pos: u64,
        c: char,
        around: bool,
        n: u64,
    ) -> Option<Span> {
        let doc = Doc::new(view.buffer());
        let obj = text_object(&doc, pos, c, around, n)?;
        Some(if obj.linewise {
            let mut span = lines_span(&doc, obj.start, obj.end.saturating_sub(1).max(obj.start));
            span.from = obj.start;
            span
        } else {
            Span {
                start: obj.start,
                end: obj.end,
                linewise: false,
                from: obj.start,
            }
        })
    }

    /// Does `op` to `span`.
    pub fn operate(&mut self, view: &View, op: Op, span: Span, cmd: &Cmd) {
        let doc = Doc::new(view.buffer());
        let text = doc.slice(span.start, span.end);
        let shape = if span.linewise {
            Shape::Lines
        } else {
            Shape::Chars
        };
        let value = Value {
            text: text.clone(),
            shape,
        };
        let big = span.linewise
            || text.contains('\n')
            || matches!(
                &cmd.action,
                Action::Operate(
                    _,
                    Target::Motion(
                        Motion::Bracket
                            | Motion::ParagraphBack
                            | Motion::ParagraphForward
                            | Motion::SearchNext(_)
                            | Motion::Search { .. }
                            | Motion::Mark { exact: true, .. }
                    )
                )
            );
        match op {
            Op::Yank => {
                if let Err(err) = self.registers.store(cmd.register, value, Why::Yank) {
                    ui::show_message(&err);
                }
                let lines = text.matches('\n').count();
                if span.linewise && lines > 2 {
                    ui::show_message(&format!("{lines} lines yanked"));
                }
                let cursor = self.cursor(view);
                self.place(view, span.from.min(cursor));
            }
            Op::Delete | Op::Change => {
                if span.start == span.end && !span.linewise {
                    if op == Op::Change {
                        self.start_insert(view, &[span.start]);
                    }
                    return;
                }
                if let Err(err) = self
                    .registers
                    .store(cmd.register, value, Why::Delete { big })
                {
                    ui::show_message(&err);
                    return;
                }
                if op == Op::Change {
                    self.change(view, span);
                } else {
                    self.delete(view, span);
                }
            }
            Op::Indent | Op::Unindent => {
                let first = doc.line_of(span.start);
                let last = doc.line_of(span.end.saturating_sub(1).max(span.start));
                let times = match &cmd.action {
                    Action::OnSelection(_) => cmd.count1(),
                    _ => 1,
                };
                self.shift_lines(view, first, last, op == Op::Indent, times);
            }
            Op::Lower | Op::Upper | Op::Toggle => {
                let changed: String = text
                    .chars()
                    .map(|c| match op {
                        Op::Lower => c.to_lowercase().collect::<String>(),
                        Op::Upper => c.to_uppercase().collect(),
                        _ => toggle(c),
                    })
                    .collect();
                if changed != text {
                    let edit = Edit {
                        start: span.start,
                        end: span.end,
                        text: changed,
                    };
                    self.edit(view, vec![edit], None);
                }
                self.place(view, span.start);
            }
        }
    }

    /// Deletes `span`, leaving the cursor where vim does.
    pub fn delete(&mut self, view: &View, span: Span) {
        let doc = Doc::new(view.buffer());
        let cursor = self.cursor(view);
        if !span.linewise {
            self.edit(
                view,
                vec![deletion(span.start, span.end)],
                Some(vec![span.start]),
            );
            let pos = self.cursor(view);
            self.place(view, pos);
            return;
        }
        let (mut start, mut end) = (span.start, span.end);
        // The last line has no line break after it to delete: the one
        // before it goes instead, so no empty line is left.
        if end >= doc.len && start > 0 && !doc.slice(start, end).ends_with('\n') {
            start -= 1;
        }
        // Deleting every line leaves one, empty.
        if start == 0 && end >= doc.len && doc.slice(0, doc.len).ends_with('\n') {
            end = doc.len - 1;
        }
        let column = self.column_of(view, cursor);
        self.edit(view, vec![deletion(start, end)], Some(vec![start]));
        let doc = Doc::new(view.buffer());
        let line = doc.line_of(span.start.min(doc.len)).min(doc.last_line());
        let pos = self.pos_at_column(view, &doc, doc.line_start(line), column);
        self.place(view, pos);
    }

    /// Deletes `span` and starts insert mode there. Lines keep one empty
    /// line, indented as the first was.
    fn change(&mut self, view: &View, span: Span) {
        let doc = Doc::new(view.buffer());
        if !span.linewise {
            self.edit(
                view,
                vec![deletion(span.start, span.end)],
                Some(vec![span.start]),
            );
            self.start_insert(view, &[span.start]);
            return;
        }
        let indent = base_kit::doc::indentation(&doc, span.start);
        let end = if span.end >= doc.len && !doc.slice(span.start, span.end).ends_with('\n') {
            span.end
        } else {
            span.end - 1
        };
        let at = span.start + indent.len() as u64;
        let edit = Edit {
            start: span.start,
            end,
            text: indent.clone(),
        };
        self.edit(view, vec![edit], Some(vec![at]));
        self.start_insert(view, &[at]);
        if let Some(session) = &mut self.session
            && !indent.is_empty()
        {
            session.autoindent = Some(span.start);
        }
    }

    /// Indents or unindents lines `first..=last`, `times` times.
    pub fn shift_lines(&mut self, view: &View, first: u64, last: u64, more: bool, times: u64) {
        let doc = Doc::new(view.buffer());
        let unit = indent_unit();
        let width = if unit == "\t" {
            settings::tab_width(&view.buffer()) as usize
        } else {
            unit.len()
        };
        let mut edits = Vec::new();
        for line in first..=last {
            let start = doc.line_start(line);
            let end = doc.line_end(start);
            if start == end {
                continue;
            }
            let leading = base_kit::doc::indentation(&doc, start);
            let columns: usize = leading
                .chars()
                .map(|c| if c == '\t' { width } else { 1 })
                .sum();
            let wanted = if more {
                columns + width * times as usize
            } else {
                columns.saturating_sub(width * times as usize)
            };
            let text = if unit == "\t" {
                "\t".repeat(wanted / width) + &" ".repeat(wanted % width)
            } else {
                " ".repeat(wanted)
            };
            if text != leading {
                edits.push(Edit {
                    start,
                    end: start + leading.len() as u64,
                    text,
                });
            }
        }
        self.edit(view, edits, None);
        let doc = Doc::new(view.buffer());
        let pos = motion::first_non_blank(&doc, doc.line_start(first));
        self.place(view, pos);
    }

    fn column_of(&self, view: &View, pos: u64) -> u32 {
        match self.column {
            Some(column) => column,
            None => view.move_vertically(pos, 0, None).map_or(0, |(_, c)| c),
        }
    }

    fn pos_at_column(&self, view: &View, doc: &Doc, line_start: u64, column: u32) -> u64 {
        let pos = view
            .move_vertically(line_start, 0, Some(column))
            .map_or(line_start, |(p, _)| p);
        clamp_normal(doc, pos)
    }

    /// `p` and `P`: puts the register's text after or before the cursor,
    /// `n` times.
    pub fn put(&mut self, view: &View, register: Option<char>, before: bool, n: u64) {
        let value = match self.registers.get(register) {
            Ok(Some(value)) => value,
            Ok(None) => {
                let name = register.unwrap_or('"');
                ui::show_message(&format!("E353: Nothing in register {name}"));
                return;
            }
            Err(err) => {
                ui::show_message(&err);
                return;
            }
        };
        let doc = Doc::new(view.buffer());
        let pos = self.cursor(view);
        match value.shape {
            Shape::Lines => {
                let text = value.text.repeat(n as usize);
                let line = doc.line_of(pos);
                let (at, insert, first) = if before {
                    let at = doc.line_start(line);
                    (at, text, at)
                } else {
                    let end = doc.line_end(pos);
                    if end < doc.len {
                        (end + 1, text, end + 1)
                    } else {
                        // After the last line, which has no line break.
                        let body = text.strip_suffix('\n').unwrap_or(&text);
                        (end, format!("\n{body}"), end + 1)
                    }
                };
                self.edit(view, vec![insertion(at, insert)], Some(vec![first]));
                let doc = Doc::new(view.buffer());
                let pos = motion::first_non_blank(&doc, first);
                self.place(view, pos);
            }
            Shape::Chars => {
                let text = value.text.repeat(n as usize);
                let at = if before || pos >= doc.line_end(pos) {
                    pos
                } else {
                    doc.next_grapheme(pos)
                };
                let len = text.len() as u64;
                let multiline = text.contains('\n');
                self.edit(view, vec![insertion(at, text)], Some(vec![at]));
                let doc = Doc::new(view.buffer());
                let pos = if multiline {
                    at
                } else {
                    doc.prev_grapheme(at + len).max(at)
                };
                self.place(view, pos);
            }
            Shape::Block => self.put_block(view, &value.text, before, n),
        }
    }

    /// Puts a block: each of its lines at the same column of the lines
    /// from the cursor's down, adding lines and spaces as needed.
    fn put_block(&mut self, view: &View, text: &str, before: bool, n: u64) {
        let doc = Doc::new(view.buffer());
        let pos = self.cursor(view);
        let at = if before || pos >= doc.line_end(pos) {
            pos
        } else {
            doc.next_grapheme(pos)
        };
        let column = view.move_vertically(at, 0, None).map_or(0, |(_, c)| c);
        let line = doc.line_of(at);
        let rows: Vec<&str> = text.split('\n').collect();
        let width = rows.iter().map(|r| r.chars().count()).max().unwrap_or(0);
        let mut edits = Vec::new();
        for (i, row) in rows.iter().enumerate() {
            let piece: String = (0..n)
                .map(|_| format!("{row:width$}"))
                .collect::<String>()
                .trim_end()
                .to_string();
            let target = line + i as u64;
            if target > doc.last_line() {
                let end = doc.len;
                let pad = " ".repeat(column as usize);
                let lead = if doc.slice(0, end).ends_with('\n') || end == 0 {
                    ""
                } else {
                    "\n"
                };
                edits.push(insertion(end, format!("{lead}{pad}{piece}\n")));
                continue;
            }
            let start = doc.line_start(target);
            let (p, reached) = view
                .move_vertically(start, 0, Some(column))
                .unwrap_or((start, 0));
            let pad = " ".repeat(column.saturating_sub(reached) as usize);
            edits.push(insertion(p, format!("{pad}{piece}")));
        }
        edits.sort_by_key(|e| e.start);
        self.edit(view, edits, None);
        self.place(view, at);
    }

    /// `J`: joins `joins` lines after the cursor's to it.
    fn join(&mut self, view: &View, pos: u64, joins: u64, spaces: bool) {
        let doc = Doc::new(view.buffer());
        let line = doc.line_of(pos);
        if line + 1 > doc.last_line() {
            return;
        }
        let last = (line + joins).min(doc.last_line());
        let mut edits = Vec::new();
        let mut cursor = pos;
        let mut shift: i64 = 0;
        for l in line..last {
            let newline = doc.line_end(doc.line_start(l));
            let next = newline + 1;
            let (end, text) = if spaces {
                let text_start = motion::first_non_blank(&doc, next);
                let next_text = doc.slice(text_start, doc.line_end(next));
                let before = doc.slice(doc.line_start(l), newline);
                let text = if next_text.is_empty()
                    || next_text.starts_with(')')
                    || before.ends_with([' ', '\t'])
                    || before.is_empty()
                {
                    ""
                } else {
                    " "
                };
                (text_start, text)
            } else {
                (next, "")
            };
            cursor = (newline as i64 + shift) as u64;
            shift += text.len() as i64 - (end - newline) as i64;
            edits.push(Edit {
                start: newline,
                end,
                text: text.to_string(),
            });
        }
        self.edit(view, edits, None);
        let doc = Doc::new(view.buffer());
        self.place(view, cursor.min(doc.len));
    }

    /// `r`: replaces `n` chars with `c`; a line break replaces them all.
    fn replace_chars(&mut self, view: &View, pos: u64, c: char, n: u64) {
        let doc = Doc::new(view.buffer());
        let end_of_line = doc.line_end(pos);
        let mut end = pos;
        for _ in 0..n {
            if end >= end_of_line {
                return;
            }
            end = doc.next_grapheme(end);
        }
        if c == '\r' || c == '\n' {
            // As Enter: the indent of the line, without the blanks after.
            let indent = base_kit::doc::indentation(&doc, pos);
            let mut after = end;
            while matches!(
                doc.slice(after, doc.next_grapheme(after)).as_str(),
                " " | "\t"
            ) {
                after = doc.next_grapheme(after);
            }
            let at = pos + 1 + indent.len() as u64;
            self.edit(
                view,
                vec![Edit {
                    start: pos,
                    end: after,
                    text: format!("\n{indent}"),
                }],
                Some(vec![at]),
            );
            return;
        }
        let text = c.to_string().repeat(n as usize);
        let last = pos + (text.len() - c.len_utf8()) as u64;
        self.edit(
            view,
            vec![Edit {
                start: pos,
                end,
                text,
            }],
            Some(vec![last]),
        );
    }

    /// `~`: toggles the case of `n` chars and moves past them.
    fn toggle_case(&mut self, view: &View, pos: u64, n: u64) {
        let doc = Doc::new(view.buffer());
        let line_end = doc.line_end(pos);
        let mut end = pos;
        for _ in 0..n {
            if end >= line_end {
                break;
            }
            end = doc.next_grapheme(end);
        }
        if end == pos {
            return;
        }
        let text = doc.slice(pos, end);
        let changed: String = text.chars().map(toggle).collect();
        let after = pos + changed.len() as u64;
        if changed != text {
            self.edit(
                view,
                vec![Edit {
                    start: pos,
                    end,
                    text: changed,
                }],
                Some(vec![after]),
            );
        }
        self.place(view, after);
    }

    /// Ctrl-a and Ctrl-x: adds `delta` to the number at or after the cursor.
    fn increment(&mut self, view: &View, pos: u64, delta: i64) {
        let doc = Doc::new(view.buffer());
        let start = doc.line_start(doc.line_of(pos));
        let line = doc.slice(start, doc.line_end(pos));
        let at = (pos - start) as usize;
        let Some((from, to, value, radix, width)) = number_at(&line, at) else {
            return;
        };
        let new = value.wrapping_add(delta);
        let text = match radix {
            16 => {
                let digits = &line[from + 2..to];
                let upper = digits.chars().any(|c| c.is_ascii_uppercase());
                let hex = format!("{:0width$x}", new as u64, width = width);
                let hex = if upper { hex.to_uppercase() } else { hex };
                format!("{}{hex}", &line[from..from + 2])
            }
            2 => format!(
                "{}{:0width$b}",
                &line[from..from + 2],
                new as u64,
                width = width
            ),
            _ => new.to_string(),
        };
        let edit = Edit {
            start: start + from as u64,
            end: start + to as u64,
            text: text.clone(),
        };
        let last = start + from as u64 + text.len() as u64 - 1;
        self.edit(view, vec![edit], Some(vec![last]));
    }

    fn insert_at(&mut self, view: &View, pos: u64, at: InsertAt) {
        let doc = Doc::new(view.buffer());
        let line_end = doc.line_end(pos);
        match at {
            InsertAt::Before => self.start_insert(view, &[pos]),
            InsertAt::After => {
                let p = if pos >= line_end {
                    pos
                } else {
                    doc.next_grapheme(pos)
                };
                self.start_insert(view, &[p]);
            }
            InsertAt::FirstNonBlank => {
                self.start_insert(view, &[motion::first_non_blank(&doc, pos)])
            }
            InsertAt::LineStart => self.start_insert(view, &[doc.line_start(doc.line_of(pos))]),
            InsertAt::LineEnd => self.start_insert(view, &[line_end]),
            InsertAt::Below | InsertAt::Above => {
                self.open_line(view, pos, at == InsertAt::Below);
            }
            InsertAt::Last => {
                let buffer = view.buffer();
                let p = buffer.marks("^").first().copied().unwrap_or(pos);
                self.start_insert(view, &[p.min(buffer.len())]);
            }
        }
    }

    /// `o` and `O`: a new line below or above, indented as the cursor's
    /// line, in insert mode.
    pub fn open_line(&mut self, view: &View, pos: u64, below: bool) {
        let doc = Doc::new(view.buffer());
        let indent = base_kit::doc::indentation(&doc, pos);
        let (edit, at, line_start) = if below {
            let end = doc.line_end(pos);
            let text = format!("\n{indent}");
            (insertion(end, text), end + 1 + indent.len() as u64, end + 1)
        } else {
            let start = doc.line_start(doc.line_of(pos));
            let text = format!("{indent}\n");
            (insertion(start, text), start + indent.len() as u64, start)
        };
        self.edit(view, vec![edit], Some(vec![at]));
        if self.mode != Mode::Insert {
            self.start_insert(view, &[at]);
        }
        if let Some(session) = &mut self.session {
            session.autoindent = (!indent.is_empty()).then_some(line_start);
        }
    }

    fn undo(&mut self, view: &View, n: u64, redo: bool) {
        for i in 0..n {
            let done = if redo { view.redo() } else { view.undo() };
            if done.is_none() {
                if i == 0 {
                    ui::show_message(if redo {
                        "Already at newest change"
                    } else {
                        "Already at oldest change"
                    });
                }
                break;
            }
        }
        let selection = view.selection();
        let r = selection.ranges[selection.primary as usize];
        self.place(view, r.anchor.min(r.head));
    }

    /// `.`: does the last change again, with `count` in place of its own.
    fn repeat(&mut self, count: Option<u64>) {
        let Some(change) = self.last_change.clone() else {
            return;
        };
        let mut cmd = change.cmd.clone();
        if count.is_some() {
            cmd.count = count;
            if let Some(change) = &mut self.last_change {
                change.cmd.count = count;
            }
        }
        self.dotting = true;
        self.run(cmd);
        if let Some(session) = &mut self.session {
            let typed = change.inserted.len().saturating_sub(1);
            session.keys = change.inserted[..typed].to_vec();
        }
        if matches!(self.mode, Mode::Insert | Mode::Replace) {
            self.replay(&change.inserted);
        }
        self.dotting = false;
    }

    /// `@c`: plays the keys recorded in `c`, `n` times. `@@` plays the last
    /// one again, and `@:` the last command line.
    fn play(&mut self, c: char, n: u64) {
        if c == ':' {
            if let Some(line) = self.last_ex.clone() {
                for _ in 0..n {
                    self.run_ex_line(&line);
                }
            }
            return;
        }
        let c = if c == '@' {
            match self.last_macro {
                Some(c) => c,
                None => return,
            }
        } else {
            c.to_ascii_lowercase()
        };
        let keys = match self.macros.get(&c) {
            Some(keys) => keys.clone(),
            None => match self.registers.get(Some(c)) {
                Ok(Some(value)) => crate::vim_keys(&value.text).unwrap_or_default(),
                _ => return,
            },
        };
        self.last_macro = Some(c);
        self.failed = false;
        for _ in 0..n {
            self.replay(&keys);
            if self.failed {
                break;
            }
        }
    }

    pub fn set_mark(&mut self, view: &View, c: char, pos: u64) {
        let buffer = view.buffer();
        match c {
            'a'..='z' | '<' | '>' | '[' | ']' | '^' | '.' => {
                buffer.set_marks(&c.to_string(), &[pos]);
            }
            'A'..='Z' => {
                buffer.set_marks(&c.to_string(), &[pos]);
                self.file_marks.insert(c, buffer.path().unwrap_or_default());
            }
            '\'' | '`' => buffer.set_marks("'", &[pos]),
            _ => {}
        }
    }

    /// Where mark `c` is, going to its file for `A` to `Z`.
    fn mark(&mut self, view: &View, c: char) -> Option<u64> {
        let name = match c {
            '`' | '\'' => "'".to_string(),
            c => c.to_string(),
        };
        if c.is_ascii_uppercase()
            && let Some(path) = self.file_marks.get(&c).cloned()
            && view.buffer().path().unwrap_or_default() != path
            && let Err(err) = base_kit::open_file(&path)
        {
            ui::show_message(&err);
            return None;
        }
        let view = view::active();
        let found = view.buffer().marks(&name).first().copied();
        if found.is_none() {
            ui::show_message("E20: Mark not set");
        }
        found
    }

    /// Remembers `pos` before a jump, for `''` and Ctrl-o.
    pub fn push_jump(&mut self, view: &View, pos: u64) {
        let buffer = view.buffer();
        buffer.set_marks("'", &[pos]);
        let doc = Doc::new(view.buffer());
        let line = doc.line_of(pos);
        let mut jumps: Vec<u64> = buffer
            .marks("jumps")
            .into_iter()
            .filter(|&p| doc.line_of(p.min(doc.len)) != line)
            .collect();
        jumps.push(pos);
        if jumps.len() > 100 {
            jumps.remove(0);
        }
        buffer.set_marks("jumps", &jumps);
        let key = buffer.path().unwrap_or_default();
        self.jump_at.insert(key, jumps.len());
    }

    /// Ctrl-o and Ctrl-i: `by` places back or forward in the jump list.
    fn jump(&mut self, view: &View, by: i64) {
        let buffer = view.buffer();
        let key = buffer.path().unwrap_or_default();
        let mut jumps = buffer.marks("jumps");
        let mut at = self.jump_at.get(&key).copied().unwrap_or(jumps.len());
        if at >= jumps.len() && by < 0 {
            // Coming back needs the place left.
            let pos = self.cursor(view);
            self.push_jump(view, pos);
            jumps = buffer.marks("jumps");
            at = jumps.len() - 1;
        }
        let target = at as i64 + by;
        if target < 0 || target >= jumps.len() as i64 {
            return;
        }
        self.jump_at.insert(key, target as usize);
        let pos = jumps[target as usize];
        buffer.set_marks("'", &[self.cursor(view)]);
        self.place(view, pos);
    }

    fn scroll(&mut self, view: &View, scroll: Scroll, count: Option<u64>) {
        let doc = Doc::new(view.buffer());
        let pos = self.cursor(view);
        let line = doc.line_of(pos);
        let shown = view.visible_range();
        let (start, end) = (shown.start, shown.end);
        let top = doc.line_of(start);
        let bottom = doc.line_of(end.saturating_sub(1).max(start));
        let amount = match scroll {
            Scroll::Center => {
                let middle = (top + bottom) / 2;
                ScrollAmount::Lines((line as i64 - middle as i64) as i32)
            }
            Scroll::Top => ScrollAmount::Lines((line as i64 - top as i64) as i32),
            Scroll::Bottom => ScrollAmount::Lines((line as i64 - bottom as i64) as i32),
            Scroll::Lines(n) => ScrollAmount::Lines(n * count.unwrap_or(1) as i32),
            Scroll::HalfPage(n) => ScrollAmount::HalfPage(n),
            Scroll::Page(n) => ScrollAmount::Page(n * count.unwrap_or(1) as i32),
        };
        let lines = view.scroll(amount);
        let moves_cursor = matches!(scroll, Scroll::HalfPage(_) | Scroll::Page(_));
        let shown = view.visible_range();
        let (start, end) = (shown.start, shown.end);
        let top = doc.line_of(start);
        let bottom = doc
            .line_of(end.saturating_sub(1).max(start))
            .min(doc.last_line());
        let wanted = if moves_cursor {
            (line as i64 + i64::from(lines)).clamp(top as i64, bottom as i64) as u64
        } else {
            line.clamp(top, bottom)
        };
        if wanted != line {
            let p = self.at_column(view, &doc, pos, wanted);
            self.go(view, p);
        }
    }

    /// Searches for vim's `pattern` from `pos`, `n` times, wrapping around.
    pub fn search_from(
        &mut self,
        view: &View,
        pos: u64,
        pattern: &str,
        backward: bool,
        n: u64,
    ) -> Option<u64> {
        let regex = match crate::pattern::translate(pattern) {
            Ok(regex) => regex,
            Err(err) => {
                ui::show_message(&err);
                return None;
            }
        };
        let buffer = view.buffer();
        let doc = Doc::new(view.buffer());
        let mut at = pos;
        let mut wrapped = false;
        for _ in 0..n {
            let found = if backward {
                let all = buffer.find_all(&regex, 0, buffer.len()).unwrap_or_default();
                match all.iter().rev().find(|r| r.start < at) {
                    Some(r) => Some(r.start),
                    None => {
                        wrapped = true;
                        all.last().map(|r| r.start)
                    }
                }
            } else {
                let from = doc.next_grapheme(at).max(at + 1).min(doc.len);
                let hit = match buffer.find(&regex, from, false) {
                    Ok(hit) => hit,
                    Err(err) => {
                        ui::show_message(&base_kit::error_message(err));
                        return None;
                    }
                };
                match hit {
                    Some(r) => Some(r.start),
                    None => {
                        wrapped = true;
                        buffer
                            .find(&regex, 0, false)
                            .ok()
                            .flatten()
                            .map(|r| r.start)
                    }
                }
            };
            match found {
                Some(p) => at = p,
                None => {
                    ui::show_message(&format!("E486: Pattern not found: {pattern}"));
                    self.failed = true;
                    return None;
                }
            }
        }
        if wrapped {
            ui::show_message(if backward {
                "search hit TOP, continuing at BOTTOM"
            } else {
                "search hit BOTTOM, continuing at TOP"
            });
        }
        Some(at)
    }
}

/// Whether a command changes the text, so `.` can do it again.
fn is_change(action: &Action) -> bool {
    match action {
        Action::Operate(op, _) => *op != Op::Yank,
        Action::DeleteChar
        | Action::DeleteCharBack
        | Action::Substitute
        | Action::SubstituteLine
        | Action::ChangeToEnd
        | Action::DeleteToEnd
        | Action::Put { .. }
        | Action::Join { .. }
        | Action::Replace(_)
        | Action::ReplaceMode
        | Action::Insert(_)
        | Action::ToggleCase
        | Action::Increment(_)
        | Action::RepeatSubstitute => true,
        _ => false,
    }
}

/// Motions that leave a place in the jump list.
fn is_jump(motion: &Motion) -> bool {
    matches!(
        motion,
        Motion::FileStart
            | Motion::FileEnd
            | Motion::Bracket
            | Motion::ParagraphForward
            | Motion::ParagraphBack
            | Motion::ScreenTop
            | Motion::ScreenMiddle
            | Motion::ScreenBottom
            | Motion::SearchNext(_)
            | Motion::Star { .. }
            | Motion::Search { .. }
            | Motion::Mark { .. }
    )
}

fn reversed(kind: FindKind) -> FindKind {
    match kind {
        FindKind::Forward => FindKind::Backward,
        FindKind::Backward => FindKind::Forward,
        FindKind::Till => FindKind::TillBackward,
        FindKind::TillBackward => FindKind::Till,
    }
}

fn find(
    t: &mut Text,
    pos: u64,
    kind: FindKind,
    c: char,
    n: u64,
    repeating: bool,
) -> Option<(u64, Kind)> {
    let p = motion::find_char(t, pos, c, kind, n, repeating)?;
    let inclusive = matches!(kind, FindKind::Forward | FindKind::Till);
    Some((
        p,
        if inclusive {
            Kind::Inclusive
        } else {
            Kind::Exclusive
        },
    ))
}

/// Whole lines from the line of `from` to the line of `to`.
pub fn lines_span(doc: &Doc, from: u64, to: u64) -> Span {
    let start = doc.line_start(doc.line_of(from));
    let end = (doc.line_end(to) + 1).min(doc.len);
    Span {
        start,
        end,
        linewise: true,
        from,
    }
}

/// The text object `c` around `pos`: `w`, `W`, `p`, `s`, brackets, and
/// quotes.
pub fn text_object(doc: &Doc, pos: u64, c: char, around: bool, n: u64) -> Option<Object> {
    let mut t = Text::new(doc);
    match c {
        'w' | 'W' => object::word(&mut t, pos, c == 'W', around, n),
        'p' => Some(object::paragraph(doc, pos, around, n)),
        '(' | ')' | 'b' => object::pair(&mut t, pos, '(', ')', around, n),
        '{' | '}' | 'B' => object::pair(&mut t, pos, '{', '}', around, n),
        '[' | ']' => object::pair(&mut t, pos, '[', ']', around, n),
        '<' | '>' => object::pair(&mut t, pos, '<', '>', around, n),
        '"' | '\'' | '`' => object::quote(&mut t, pos, c, around),
        _ => None,
    }
}

/// The keyword under or after the cursor on its line, for `*`.
fn word_under(t: &mut Text, pos: u64) -> Option<String> {
    let mut p = pos;
    let end = t.line_end(pos);
    // The first keyword char at or after the cursor, or else any non-blank.
    let mut start = None;
    while p < end {
        let c = t.char_at(p)?;
        if crate::text::is_keyword(c) {
            start = Some(p);
            break;
        }
        p = t.next(p)?;
    }
    let mut start = start?;
    while let Some(prev) = t.prev(start) {
        if !t.char_at(prev).is_some_and(crate::text::is_keyword) {
            break;
        }
        start = prev;
    }
    let mut e = start;
    while e < end && t.char_at(e).is_some_and(crate::text::is_keyword) {
        e = t.next(e)?;
    }
    Some(t.doc.slice(start, e))
}

fn toggle(c: char) -> String {
    if c.is_lowercase() {
        c.to_uppercase().collect()
    } else if c.is_uppercase() {
        c.to_lowercase().collect()
    } else {
        c.to_string()
    }
}

pub fn insertion(pos: u64, text: String) -> Edit {
    Edit {
        start: pos,
        end: pos,
        text,
    }
}

pub fn deletion(start: u64, end: u64) -> Edit {
    Edit {
        start,
        end,
        text: String::new(),
    }
}

/// The number at or after byte `at` of `line`: its range, value, radix,
/// and digits (for keeping leading zeros in hex and binary).
fn number_at(line: &str, at: usize) -> Option<(usize, usize, i64, u32, usize)> {
    let bytes = line.as_bytes();
    // Hex and binary: `0x` or `0b` with the cursor on or in them.
    let mut i = at.min(bytes.len());
    // Back to the start of a word of digits and letters the cursor is in.
    while i > 0 && bytes[i - 1].is_ascii_alphanumeric() {
        i -= 1;
    }
    let mut start = i;
    loop {
        if start >= bytes.len() {
            return None;
        }
        if bytes[start].is_ascii_digit() {
            break;
        }
        start += 1;
    }
    // `start` is the first digit at or after the cursor's word.
    if start + 1 < bytes.len()
        && bytes[start] == b'0'
        && matches!(bytes[start + 1], b'x' | b'X' | b'b' | b'B')
    {
        let radix = if matches!(bytes[start + 1], b'x' | b'X') {
            16
        } else {
            2
        };
        let digits_start = start + 2;
        let mut end = digits_start;
        while end < bytes.len() && (bytes[end] as char).is_digit(radix) {
            end += 1;
        }
        if end > digits_start {
            let value = i64::from_str_radix(&line[digits_start..end], radix).ok()?;
            return Some((start, end, value, radix, end - digits_start));
        }
    }
    let mut end = start;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    let negative = start > 0
        && bytes[start - 1] == b'-'
        && (start < 2 || !bytes[start - 2].is_ascii_alphanumeric());
    let from = if negative { start - 1 } else { start };
    let value: i64 = line[from..end].parse().ok()?;
    Some((from, end, value, 10, end - start))
}

/// Ctrl-w and a key: splitting and moving between views.
fn window(c: char) {
    match c {
        'v' => view::split(Direction::Vertical),
        's' => view::split(Direction::Horizontal),
        'w' => view::focus(Toward::Next),
        'h' => view::focus(Toward::Left),
        'j' => view::focus(Toward::Down),
        'k' => view::focus(Toward::Up),
        'l' => view::focus(Toward::Right),
        'q' | 'c' => {
            if let Err(err) = view::close() {
                ui::show_message(&err);
            }
        }
        'o' => view::only(),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::number_at;

    #[test]
    fn numbers_are_found_at_or_after_the_cursor() {
        assert_eq!(number_at("x = 41;", 0), Some((4, 6, 41, 10, 2)));
        assert_eq!(number_at("x = -5", 0), Some((4, 6, -5, 10, 1)));
        assert_eq!(number_at("a-5", 0), Some((2, 3, 5, 10, 1)));
        assert_eq!(number_at("0x0f", 1), Some((0, 4, 15, 16, 2)));
        assert_eq!(number_at("none", 0), None);
    }
}
