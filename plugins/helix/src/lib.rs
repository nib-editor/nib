//! Helix-style modal keymap: normal, select, and insert modes, and a `:`
//! command line.
//!
//! In normal mode every range is at least a block over one grapheme, as in
//! Helix, and motions select what they pass over. The core knows nothing
//! about modes; they live here.

mod doc;
mod hints;
mod keys;
mod tree;

use std::cell::RefCell;
use std::collections::BTreeMap;

use doc::{Doc, FindKind};
use keys::{Binding, Keymap, Keymaps};
use nib_plugin::exports::nib::plugin::guest::{Guest, KeyResult};
use nib_plugin::nib::plugin::editor::{ScrollAmount, View};
use nib_plugin::nib::plugin::events::{self, Event};
use nib_plugin::nib::plugin::types::{
    CursorShape, Edit, KeyCode, KeyEvent, Modifiers, SelRange, Selection, Span, UndoMode,
};
use nib_plugin::nib::plugin::ui::{Decoration, Panel, Popup, PopupAnchor, Side};
use nib_plugin::nib::plugin::{clipboard, commands, editor, input, settings, ui};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Normal,
    /// Motions extend the selections instead of replacing them.
    Select,
    Insert,
}

/// A key that waits for the next one.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pending {
    /// `g`, for goto.
    Goto,
    /// `f`, `t`, `F`, `T`, waiting for the char.
    Find(FindKind),
    /// `r`, waiting for the replacement.
    Replace,
    /// `m`, for matching pairs.
    Match,
    /// `mi` and `ma`, waiting for the pair's char or the object's key.
    MatchPair { around: bool },
    /// `]` and `[`, waiting for the object's key.
    Object { forward: bool },
    /// `Space`, for commands of other plugins, such as the file picker.
    Space,
    /// `"`, waiting for a register's name.
    Register,
    /// `Ctrl-w` and `Space w`, for split views.
    Window,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Prompt {
    /// `:`
    Command,
    /// `/` and `?`
    Search { backward: bool },
    /// `s`
    Select,
}

impl Prompt {
    fn label(self) -> &'static str {
        match self {
            Prompt::Command => ":",
            Prompt::Search { backward: false } => "/",
            Prompt::Search { backward: true } => "?",
            Prompt::Select => "select: ",
        }
    }
}

struct CommandLine {
    prompt: Prompt,
    panel: Panel,
    input: String,
}

struct Helix {
    mode: Mode,
    pending: Option<Pending>,
    /// A count typed before a command, as in `3w`.
    count: Option<u64>,
    /// The display column `j` and `k` aim for, kept across short lines.
    column: Option<u32>,
    /// Whether the current insert session has edited yet, so later edits
    /// join the same undo step.
    inserted: bool,
    /// Insert mode was entered with `a`: leaving it moves the cursor back
    /// onto the last inserted grapheme, as Helix does.
    appending: bool,
    command_line: Option<CommandLine>,
    /// What `y`, `d`, and `c` took, one value per selection, by register.
    registers: BTreeMap<char, Vec<String>>,
    /// The register chosen with `"` for the next command.
    register: Option<char>,
    /// The register was chosen by the key just handled, so it stays for
    /// the next one.
    register_fresh: bool,
    /// The keys of the insert in progress, from the key that started it.
    recording: Option<Vec<KeyEvent>>,
    /// The keys of the last insert, for `.`.
    last_insert: Option<Vec<KeyEvent>>,
    /// Keys are being replayed by `.`, so they are not recorded.
    replaying: bool,
    /// The last search pattern, for `n` and `N`.
    search: Option<String>,
    /// Selections before and after each `Alt-o`, so `Alt-i` can go back.
    expansions: Vec<(Selection, Selection)>,
    /// The key hints shown for a pending key.
    hints: Option<(Pending, Popup)>,
    /// Keys from `[settings.keys.*]`, looked at before the built-in ones.
    keymaps: Keymaps,
    /// Inside a table of `keymaps`: its keys, and the keys typed to get
    /// there.
    remap_prefix: Option<(Keymap, Vec<KeyEvent>)>,
    /// What the table's keys do, while in it.
    remap_hints: Option<Popup>,
}

thread_local! {
    static HELIX: RefCell<Helix> = const {
        RefCell::new(Helix {
            mode: Mode::Normal,
            pending: None,
            count: None,
            column: None,
            inserted: false,
            appending: false,
            command_line: None,
            registers: BTreeMap::new(),
            register: None,
            register_fresh: false,
            recording: None,
            last_insert: None,
            replaying: false,
            search: None,
            expansions: Vec::new(),
            hints: None,
            keymaps: Keymaps {
                normal: Vec::new(),
                insert: Vec::new(),
                select: Vec::new(),
            },
            remap_prefix: None,
            remap_hints: None,
        })
    };
}

struct Plugin;

impl Guest for Plugin {
    fn init(config: String) -> Result<(), String> {
        let settings: serde_json::Value =
            serde_json::from_str(&config).map_err(|err| err.to_string())?;
        let (keymaps, errors) = keys::keymaps(&settings);
        if let Some(first) = errors.first() {
            let more = match errors.len() {
                1 => String::new(),
                n => format!(" (and {} more)", n - 1),
            };
            ui::show_message(&format!("helix.toml: {first}{more}; left out"));
        }
        input::push_layer();
        HELIX.with_borrow_mut(|helix| {
            helix.keymaps = keymaps;
            helix.set_mode(Mode::Normal);
            to_blocks(&editor::active_view());
        });
        Ok(())
    }

    fn handle_key(ev: KeyEvent) -> KeyResult {
        HELIX.with_borrow_mut(|helix| helix.handle_key(ev))
    }

    fn run_command(name: String, _args: String) -> Result<String, String> {
        Err(format!("no command {name}"))
    }

    fn on_event(ev: Event) {
        // The tree caught up with an edit: the bracket match waited for it.
        if let Event::Custom(event) = ev
            && event.name == "editor.syntax_updated"
        {
            highlight_match(&editor::active_view());
        }
    }
}

nib_plugin::export!(Plugin);

fn plain(ev: &KeyEvent) -> Option<char> {
    match ev.code {
        KeyCode::Char(c) if ev.modifiers.is_empty() => Some(c),
        _ => None,
    }
}

fn ctrl(ev: &KeyEvent) -> Option<char> {
    match ev.code {
        KeyCode::Char(c) if ev.modifiers == Modifiers::CTRL => Some(c),
        _ => None,
    }
}

impl Helix {
    fn handle_key(&mut self, ev: KeyEvent) -> KeyResult {
        if self.command_line.is_some() {
            self.command_line_key(ev);
            return KeyResult::Handled;
        }
        let view = editor::active_view();
        let version = view.buffer().version();
        let was_inserting = self.mode == Mode::Insert;
        let handled = match self.remapped(&view, ev) {
            Some(handled) => handled,
            None => self.dispatch(&view, ev),
        };
        self.record(ev, was_inserting);
        // A chosen register lasts for the next command only.
        if !self.register_fresh && self.pending.is_none() && self.count.is_none() {
            self.register = None;
        }
        self.register_fresh = false;
        self.show_hints();
        // After an edit, reading the tree would wait for it to be parsed;
        // editor.syntax_updated says when it is. The key may also have
        // switched buffers.
        let active = editor::active_view();
        if active.buffer().version() == version {
            highlight_match(&active);
        }
        if handled {
            KeyResult::Handled
        } else {
            KeyResult::Pass
        }
    }

    /// Records the keys of an insert, from the key that started it to the
    /// one that ended it, for `.`.
    fn record(&mut self, ev: KeyEvent, was_inserting: bool) {
        if self.replaying {
            return;
        }
        let inserting = self.mode == Mode::Insert;
        if !was_inserting {
            if inserting {
                self.recording = Some(vec![ev]);
            }
            return;
        }
        if let Some(keys) = &mut self.recording {
            keys.push(ev);
        }
        if !inserting {
            self.last_insert = self.recording.take();
        }
    }

    /// `.`: does the last insert again, `count` times.
    fn repeat(&mut self, count: u64) {
        let Some(keys) = self.last_insert.clone() else {
            ui::show_message("nothing to repeat yet");
            return;
        };
        self.replaying = true;
        for _ in 0..count {
            for &key in &keys {
                self.handle_key(key);
            }
        }
        self.replaying = false;
    }

    /// The register the next command uses: the chosen one, or `"`.
    fn take_register(&mut self) -> char {
        self.register.take().unwrap_or('"')
    }

    /// The built-in keys of the mode.
    fn dispatch(&mut self, view: &View, ev: KeyEvent) -> bool {
        match self.mode {
            Mode::Normal | Mode::Select => self.normal_key(view, ev),
            Mode::Insert => self.insert_key(view, ev),
        }
    }

    /// Handles `ev` with the keys from the settings, or returns `None` when
    /// they do not have it.
    fn remapped(&mut self, view: &View, ev: KeyEvent) -> Option<bool> {
        if let Some((table, typed)) = self.remap_prefix.take() {
            self.remap_hints = None;
            if let Some(binding) = keys::lookup(&table, &ev).cloned() {
                let mut typed = typed;
                typed.push(ev);
                return Some(self.run_binding(view, binding, typed));
            }
            // In insert mode, the keys typed were meant as text after all.
            if self.mode == Mode::Insert {
                for key in typed {
                    self.dispatch(view, key);
                }
            }
            return Some(self.dispatch(view, ev));
        }
        // Inside a built-in sequence such as `g`, keys keep their meaning.
        if self.pending.is_some() {
            return None;
        }
        let keymap = match self.mode {
            Mode::Normal => &self.keymaps.normal,
            Mode::Insert => &self.keymaps.insert,
            Mode::Select => &self.keymaps.select,
        };
        let binding = keys::lookup(keymap, &ev)?.clone();
        Some(self.run_binding(view, binding, vec![ev]))
    }

    fn run_binding(&mut self, view: &View, binding: Binding, typed: Vec<KeyEvent>) -> bool {
        match binding {
            Binding::Command(name) => call_or_show(&name),
            Binding::Keys(keys) => {
                for key in keys {
                    self.dispatch(view, key);
                }
            }
            Binding::Prefix(table) => {
                let title: Vec<String> = typed.iter().map(keys::label).collect();
                let mut lines = vec![vec![span(&title.join(" "), "ui.popup.title")]];
                let width = table
                    .iter()
                    .map(|(k, _)| keys::label(k).len())
                    .max()
                    .unwrap_or(0);
                for (key, binding) in &table {
                    lines.push(vec![
                        span(&format!("{:width$}", keys::label(key)), "ui.popup.key"),
                        span(&format!("  {}", keys::describe(binding)), ""),
                    ]);
                }
                self.remap_hints = Some(Popup::new(PopupAnchor::Corner, &lines));
                self.remap_prefix = Some((table, typed));
            }
        }
        true
    }

    fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        let (name, label, style, shape) = match mode {
            Mode::Normal => ("normal", " NOR ", "ui.mode.normal", CursorShape::Block),
            Mode::Select => ("select", " SEL ", "ui.mode.select", CursorShape::Block),
            Mode::Insert => ("insert", " INS ", "ui.mode.insert", CursorShape::Bar),
        };
        editor::active_view().set_cursor_shape(shape);
        ui::set_status("mode", Side::Left, 0, &[span(label, style)]);
        // For other plugins, such as a status line that shows the mode.
        events::emit("mode_changed", &format!("\"{name}\""));
    }

    /// Normal and select mode. Returns whether the key was used.
    fn normal_key(&mut self, view: &View, ev: KeyEvent) -> bool {
        if let Some(pending) = self.pending.take() {
            self.pending_key(view, pending, ev);
            self.count = None;
            return true;
        }
        if let Some(digit) = plain(&ev).and_then(|c| c.to_digit(10))
            && (digit != 0 || self.count.is_some())
        {
            self.count = Some(self.count.unwrap_or(0) * 10 + u64::from(digit));
            return true;
        }
        let count = self.count.take().unwrap_or(1);
        let vertical = matches!(
            ev.code,
            KeyCode::Char('j' | 'k') | KeyCode::Down | KeyCode::Up
        );
        if !vertical {
            self.column = None;
        }

        if let Some(c) = ctrl(&ev) {
            let amount = match c {
                'd' => ScrollAmount::HalfPage(1),
                'u' => ScrollAmount::HalfPage(-1),
                'f' => ScrollAmount::Page(1),
                'b' => ScrollAmount::Page(-1),
                'w' => {
                    self.wait(Pending::Window, count);
                    return true;
                }
                _ => return false,
            };
            self.scroll(view, amount);
            return true;
        }
        if ev.modifiers == Modifiers::ALT {
            match ev.code {
                KeyCode::Char(';') => flip_selections(view),
                KeyCode::Char('o') | KeyCode::Up => self.expand(view),
                KeyCode::Char('i') | KeyCode::Down => self.shrink(view),
                KeyCode::Char('n') | KeyCode::Right => select_siblings(view, true),
                KeyCode::Char('p') | KeyCode::Left => select_siblings(view, false),
                _ => return false,
            }
            return true;
        }
        match ev.code {
            KeyCode::Left => self.move_cursors(view, count, |doc, pos| doc.prev_grapheme(pos)),
            KeyCode::Right => self.move_cursors(view, count, next_char),
            KeyCode::Down => self.move_lines(view, count as i32),
            KeyCode::Up => self.move_lines(view, -(count as i32)),
            KeyCode::Escape => {
                if self.mode == Mode::Select {
                    self.set_mode(Mode::Normal);
                }
            }
            _ => match plain(&ev) {
                Some(c) => return self.normal_char(view, c, count),
                None => return false,
            },
        }
        true
    }

    fn normal_char(&mut self, view: &View, c: char, count: u64) -> bool {
        match c {
            'h' => self.move_cursors(view, count, |doc, pos| doc.prev_grapheme(pos)),
            'l' => self.move_cursors(view, count, next_char),
            'j' => self.move_lines(view, count as i32),
            'k' => self.move_lines(view, -(count as i32)),
            'w' => self.motion(view, count, |doc, pos| {
                doc::next_word_start(doc, pos, false)
            }),
            'W' => self.motion(view, count, |doc, pos| doc::next_word_start(doc, pos, true)),
            'e' => self.motion(view, count, |doc, pos| doc::next_word_end(doc, pos, false)),
            'E' => self.motion(view, count, |doc, pos| doc::next_word_end(doc, pos, true)),
            'b' => self.motion(view, count, |doc, pos| {
                doc::prev_word_start(doc, pos, false)
            }),
            'B' => self.motion(view, count, |doc, pos| doc::prev_word_start(doc, pos, true)),
            'f' => self.wait(Pending::Find(FindKind::Forward), count),
            't' => self.wait(Pending::Find(FindKind::Till), count),
            'F' => self.wait(Pending::Find(FindKind::Backward), count),
            'T' => self.wait(Pending::Find(FindKind::TillBackward), count),
            'g' => self.wait(Pending::Goto, count),
            'v' => {
                let mode = if self.mode == Mode::Select {
                    Mode::Normal
                } else {
                    Mode::Select
                };
                self.set_mode(mode);
            }
            'x' => {
                for _ in 0..count {
                    select_lines(view);
                }
            }
            '%' => {
                let len = view.buffer().len();
                set_ranges(view, vec![range(0, len)], 0);
            }
            ';' => to_blocks(view),
            ',' => {
                let selection = view.selection();
                let primary = selection.ranges[selection.primary as usize];
                set_ranges(view, vec![primary], 0);
            }
            'C' => {
                for _ in 0..count {
                    copy_to_next_line(view);
                }
            }
            'i' => self.insert_at(view, |range| range.anchor.min(range.head)),
            'a' => {
                self.insert_at(view, |range| range.anchor.max(range.head));
                self.appending = true;
            }
            'I' => {
                let doc = Doc::new(view.buffer());
                self.insert_at(view, |r| doc::first_non_blank(&doc, cursor(&doc, r)));
            }
            'A' => {
                let doc = Doc::new(view.buffer());
                self.insert_at(view, |r| doc.line_end(cursor(&doc, r)));
            }
            'o' => self.open_below(view),
            'O' => self.open_above(view),
            'y' => {
                let n = self.yank(view);
                let plural = if n == 1 { "" } else { "s" };
                ui::show_message(&format!("yanked {n} selection{plural}"));
            }
            '.' => self.repeat(count),
            '"' => self.wait(Pending::Register, count),
            'd' => {
                self.yank(view);
                delete_selections(view);
                to_blocks(view);
                self.set_mode(Mode::Normal);
            }
            'c' => {
                self.yank(view);
                delete_selections(view);
                self.start_insert();
                // The deletion and what is typed next undo together.
                self.inserted = true;
            }
            'p' => self.paste(view, false),
            'P' => self.paste(view, true),
            'r' => self.wait(Pending::Replace, count),
            '>' => indent(view, true),
            '<' => indent(view, false),
            'J' => join_lines(view),
            'u' => {
                if view.undo() {
                    to_blocks(view);
                }
            }
            'U' => {
                if view.redo() {
                    to_blocks(view);
                }
            }
            ':' => self.open_prompt(Prompt::Command),
            '/' => self.open_prompt(Prompt::Search { backward: false }),
            '?' => self.open_prompt(Prompt::Search { backward: true }),
            's' => self.open_prompt(Prompt::Select),
            'n' | 'N' => match self.search.clone() {
                Some(pattern) => search(view, &pattern, c == 'N'),
                None => ui::show_message("no search pattern yet"),
            },
            '*' => {
                let doc = Doc::new(view.buffer());
                let selection = view.selection();
                let r = selection.ranges[selection.primary as usize];
                let text = doc.slice(r.anchor.min(r.head), r.anchor.max(r.head));
                let pattern = regex_escape(&text);
                ui::show_message(&format!("search: {pattern}"));
                self.search = Some(pattern);
            }
            'm' => self.wait(Pending::Match, count),
            ' ' => self.wait(Pending::Space, count),
            ']' => self.wait(Pending::Object { forward: true }, count),
            '[' => self.wait(Pending::Object { forward: false }, count),
            _ => return false,
        }
        true
    }

    /// Shows what the next key can do while one is pending, and hides it
    /// afterwards.
    fn show_hints(&mut self) {
        match (self.pending, &self.hints) {
            (Some(pending), Some((shown, _))) if pending == *shown => {}
            (Some(pending), _) => {
                self.hints = hints::lines(pending)
                    .map(|lines| (pending, Popup::new(PopupAnchor::Corner, &lines)));
            }
            // Dropping the popup closes it.
            (None, _) => self.hints = None,
        }
    }

    /// Waits for the next key, keeping the count for it.
    fn wait(&mut self, pending: Pending, count: u64) {
        self.pending = Some(pending);
        self.count = (count > 1).then_some(count);
    }

    fn pending_key(&mut self, view: &View, pending: Pending, ev: KeyEvent) {
        let count = self.count.take();
        let Some(c) = plain(&ev) else {
            return;
        };
        match pending {
            Pending::Find(kind) => {
                self.motion(view, count.unwrap_or(1), |doc, pos| {
                    doc::find_char(doc, pos, c, kind)
                });
            }
            Pending::Replace => replace_with(view, c),
            Pending::Match => match c {
                'm' => self.move_cursors(view, 1, |doc, pos| {
                    tree::matching_pair(&doc.buffer, pos)
                        .or_else(|| doc::matching_bracket(doc, pos))
                        .unwrap_or(pos)
                }),
                'i' => self.pending = Some(Pending::MatchPair { around: false }),
                'a' => self.pending = Some(Pending::MatchPair { around: true }),
                _ => {}
            },
            Pending::MatchPair { around } => select_pairs(view, c, around),
            Pending::Object { forward } => goto_object(view, c, forward, count.unwrap_or(1)),
            Pending::Register => {
                self.register = Some(c);
                self.register_fresh = true;
            }
            Pending::Space => {
                // The system clipboard, as the `+` register.
                if matches!(c, 'y' | 'p' | 'P') {
                    self.register = Some('+');
                    match c {
                        'y' => {
                            self.yank(view);
                        }
                        'p' => self.paste(view, false),
                        _ => self.paste(view, true),
                    }
                    return;
                }
                if c == 'w' {
                    self.pending = Some(Pending::Window);
                    return;
                }
                let command = match c {
                    'f' => "picker.files",
                    'k' => "lsp.hover",
                    _ => return,
                };
                call_or_show(command);
            }
            Pending::Window => {
                let (command, args) = match c {
                    'v' => ("view.split", r#"{"direction":"vertical"}"#),
                    's' => ("view.split", r#"{"direction":"horizontal"}"#),
                    'w' => ("view.focus", r#"{"to":"next"}"#),
                    'h' => ("view.focus", r#"{"to":"left"}"#),
                    'j' => ("view.focus", r#"{"to":"down"}"#),
                    'k' => ("view.focus", r#"{"to":"up"}"#),
                    'l' => ("view.focus", r#"{"to":"right"}"#),
                    'q' => ("view.close", ""),
                    'o' => ("view.only", ""),
                    _ => return,
                };
                if let Err(err) = commands::call(command, args) {
                    ui::show_message(&err);
                }
            }
            Pending::Goto => {
                let goto: fn(&Doc, u64, Option<u64>) -> u64 = match c {
                    'g' => |doc, _, count| doc.line_start(count.map_or(0, |n| n.saturating_sub(1))),
                    'e' => |doc, _, _| doc.line_start(last_line(doc)),
                    'h' => |doc, pos, _| doc.line_start(doc.line_of(pos)),
                    'l' => |doc, pos, _| {
                        let (start, end) = (doc.line_start(doc.line_of(pos)), doc.line_end(pos));
                        if end > start {
                            doc.prev_grapheme(end)
                        } else {
                            start
                        }
                    },
                    's' => |doc, pos, _| doc::first_non_blank(doc, pos),
                    'd' => {
                        call_or_show("lsp.definition");
                        return;
                    }
                    'n' | 'p' => {
                        let command = if c == 'n' {
                            "buffer.next"
                        } else {
                            "buffer.previous"
                        };
                        let _ = commands::call(command, "");
                        to_blocks(&editor::active_view());
                        return;
                    }
                    _ => return,
                };
                self.move_cursors(view, 1, |doc, pos| goto(doc, pos, count));
            }
        }
    }

    /// `Alt-o`: grows every selection to the syntax node around it.
    fn expand(&mut self, view: &View) {
        let buffer = view.buffer();
        let before = view.selection();
        let ranges = before
            .ranges
            .iter()
            .map(|r| {
                let (start, end) = (r.anchor.min(r.head), r.anchor.max(r.head));
                tree::expand(&buffer, start, end).map_or(*r, |(s, e)| range(s, e))
            })
            .collect();
        set_ranges(view, ranges, before.primary);
        let after = view.selection();
        if after != before {
            self.expansions.push((before, after));
        }
    }

    /// `Alt-i`: undoes the last `Alt-o`, or without one, shrinks every
    /// selection to the first named node inside it.
    fn shrink(&mut self, view: &View) {
        let current = view.selection();
        match self.expansions.pop() {
            Some((before, after)) if after == current => {
                let _ = view.set_selection(&before);
                return;
            }
            // The selection changed since: the history no longer applies.
            _ => self.expansions.clear(),
        }
        let buffer = view.buffer();
        let ranges = current
            .ranges
            .iter()
            .map(|r| {
                let (start, end) = (r.anchor.min(r.head), r.anchor.max(r.head));
                tree::shrink(&buffer, start, end).map_or(*r, |(s, e)| range(s, e))
            })
            .collect();
        set_ranges(view, ranges, current.primary);
    }

    /// Moves every cursor with `to`, `count` times. In select mode the
    /// selections grow instead.
    fn move_cursors(&mut self, view: &View, count: u64, to: impl Fn(&Doc, u64) -> u64) {
        let doc = Doc::new(view.buffer());
        let select = self.mode == Mode::Select;
        let ranges = view
            .selection()
            .ranges
            .iter()
            .map(|r| {
                let mut pos = cursor(&doc, r);
                for _ in 0..count {
                    pos = to(&doc, pos);
                }
                if select {
                    extend(&doc, r, pos)
                } else {
                    block(&doc, pos)
                }
            })
            .collect();
        set_ranges(view, ranges, view.selection().primary);
    }

    /// Replaces every range with what `motion` selects from its cursor,
    /// `count` times. In select mode the selections grow instead.
    fn motion(
        &mut self,
        view: &View,
        count: u64,
        motion: impl Fn(&Doc, u64) -> Option<(u64, u64)>,
    ) {
        let doc = Doc::new(view.buffer());
        let select = self.mode == Mode::Select;
        let ranges = view
            .selection()
            .ranges
            .iter()
            .map(|r| {
                let mut new = *r;
                for _ in 0..count {
                    match motion(&doc, cursor(&doc, &new)) {
                        Some((anchor, head)) if anchor != head => new = range(anchor, head),
                        _ => break,
                    }
                }
                if select {
                    extend(&doc, r, cursor(&doc, &new))
                } else {
                    new
                }
            })
            .collect();
        set_ranges(view, ranges, view.selection().primary);
    }

    /// Scrolls, then moves each cursor as many lines, keeping it inside the
    /// scroll margin so the core does not scroll the view back to it.
    fn scroll(&mut self, view: &View, amount: ScrollAmount) {
        let lines = view.scroll(amount);
        let doc = Doc::new(view.buffer());
        let (start, end) = view.visible_range();
        let top = doc.line_of(start);
        let bottom = doc.line_of(end.saturating_sub(1).max(start));
        let rows = bottom - top + 1;
        let margin: u64 = settings::get("scroll-margin")
            .and_then(|m| m.parse().ok())
            .unwrap_or(0)
            .min(rows.saturating_sub(1) / 2);
        // No margin is needed where the view cannot scroll further.
        let low = if top == 0 { 0 } else { top + margin };
        let high = if bottom >= last_line(&doc) {
            bottom
        } else {
            bottom - margin
        };
        let selection = view.selection();
        let target = |r: &SelRange| {
            let line = doc.line_of(cursor(&doc, r)) as i64;
            let wanted = (line + i64::from(lines)).clamp(low as i64, high.max(low) as i64);
            (wanted - line) as i32
        };
        let moves: Vec<i32> = selection.ranges.iter().map(target).collect();
        let mut column = self.column;
        let ranges = selection
            .ranges
            .iter()
            .zip(moves)
            .map(
                |(r, lines)| match view.move_vertically(cursor(&doc, r), lines, column) {
                    Ok((pos, aimed)) => {
                        column = Some(aimed);
                        if self.mode == Mode::Select {
                            extend(&doc, r, pos)
                        } else {
                            block(&doc, pos)
                        }
                    }
                    Err(_) => *r,
                },
            )
            .collect();
        set_ranges(view, ranges, selection.primary);
        self.column = column;
    }

    fn move_lines(&mut self, view: &View, lines: i32) {
        let doc = Doc::new(view.buffer());
        let select = self.mode == Mode::Select;
        let mut column = self.column;
        let ranges = view
            .selection()
            .ranges
            .iter()
            .map(
                |r| match view.move_vertically(cursor(&doc, r), lines, column) {
                    Ok((pos, aimed)) => {
                        column = Some(aimed);
                        if select {
                            extend(&doc, r, pos)
                        } else {
                            block(&doc, pos)
                        }
                    }
                    Err(_) => *r,
                },
            )
            .collect();
        set_ranges(view, ranges, view.selection().primary);
        self.column = column;
    }

    fn insert_at(&mut self, view: &View, at: impl Fn(&SelRange) -> u64) {
        let selection = view.selection();
        let ranges = selection.ranges.iter().map(|r| point(at(r))).collect();
        set_ranges(view, ranges, selection.primary);
        self.start_insert();
    }

    fn start_insert(&mut self) {
        self.inserted = false;
        self.appending = false;
        self.set_mode(Mode::Insert);
    }

    /// Opens a line below each cursor, indented like the cursor's line.
    fn open_below(&mut self, view: &View) {
        let doc = Doc::new(view.buffer());
        let selection = view.selection();
        let edits: Vec<Edit> = selection
            .ranges
            .iter()
            .map(|r| {
                let pos = cursor(&doc, r);
                let end = doc.line_end(pos);
                insertion(end, format!("\n{}", doc::indentation(&doc, pos)))
            })
            .collect();
        // Points at the line ends move past the inserted text.
        let ranges = edits.iter().map(|edit| point(edit.start)).collect();
        set_ranges(view, ranges, selection.primary);
        self.start_insert();
        self.edit(view, &edits);
    }

    /// Opens a line above each cursor, indented like the cursor's line.
    fn open_above(&mut self, view: &View) {
        let doc = Doc::new(view.buffer());
        let changes = view
            .selection()
            .ranges
            .iter()
            .map(|r| {
                let pos = cursor(&doc, r);
                let start = doc.line_start(doc.line_of(pos));
                let indent = doc::indentation(&doc, pos);
                let cursor_at = indent.len() as u64;
                (
                    insertion(start, format!("{indent}\n")),
                    cursor_at,
                    cursor_at,
                )
            })
            .collect();
        if apply_placing(view, changes, UndoMode::NewStep) {
            self.start_insert();
            self.inserted = true;
        }
    }

    /// Copies the selections into the register, and returns how many.
    /// `_` keeps nothing.
    fn yank(&mut self, view: &View) -> usize {
        let doc = Doc::new(view.buffer());
        let values: Vec<String> = view
            .selection()
            .ranges
            .iter()
            .map(|r| doc.slice(r.anchor.min(r.head), r.anchor.max(r.head)))
            .collect();
        let count = values.len();
        match self.take_register() {
            '_' => {}
            // One text for the system clipboard, a line per selection.
            '+' => {
                if let Err(err) = clipboard::set(&values.join("\n")) {
                    ui::show_message(&format!("clipboard: {err}"));
                }
            }
            register => {
                self.registers.insert(register, values);
            }
        }
        count
    }

    /// `p` pastes after each selection, `P` before it. Text yanked by whole
    /// lines goes after or before the line instead. The pasted text ends up
    /// selected.
    fn paste(&mut self, view: &View, before: bool) {
        let register = self.take_register();
        let values = if register == '+' {
            match clipboard::get() {
                Ok(text) => vec![text],
                Err(err) => {
                    ui::show_message(&format!("clipboard: {err}"));
                    return;
                }
            }
        } else {
            self.registers.get(&register).cloned().unwrap_or_default()
        };
        if values.iter().all(String::is_empty) {
            ui::show_message(&format!("register {register} is empty"));
            return;
        }
        let doc = Doc::new(view.buffer());
        let last = values.len() - 1;
        let changes = view
            .selection()
            .ranges
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let value = &values[i.min(last)];
                let (from, to) = (r.anchor.min(r.head), r.anchor.max(r.head));
                // The pasted text is selected; a line break added in front of
                // it is not.
                let (pos, text, lead) = if value.ends_with('\n') {
                    if before {
                        (doc.line_start(doc.line_of(from)), value.clone(), 0)
                    } else {
                        let end = doc.line_end(cursor(&doc, r));
                        if end < doc.len {
                            (end + 1, value.clone(), 0)
                        } else {
                            // The last line has no line break to paste after.
                            (end, format!("\n{}", &value[..value.len() - 1]), 1)
                        }
                    }
                } else {
                    (if before { from } else { to }, value.clone(), 0)
                };
                let len = text.len() as u64;
                (insertion(pos, text), lead, len)
            })
            .collect();
        apply_placing(view, changes, UndoMode::NewStep);
    }

    fn insert_key(&mut self, view: &View, ev: KeyEvent) -> bool {
        match ev.code {
            KeyCode::Escape => {
                if self.appending {
                    self.appending = false;
                    let doc = Doc::new(view.buffer());
                    let selection = view.selection();
                    let ranges = selection
                        .ranges
                        .iter()
                        .map(|r| block(&doc, doc.prev_grapheme(r.head)))
                        .collect();
                    set_ranges(view, ranges, selection.primary);
                } else {
                    to_blocks(view);
                }
                self.set_mode(Mode::Normal);
            }
            KeyCode::Char('x') if ev.modifiers == Modifiers::CTRL => call_or_show("lsp.complete"),
            KeyCode::Char(c) if ev.modifiers.is_empty() => self.insert_text(view, &c.to_string()),
            KeyCode::Enter => {
                let doc = Doc::new(view.buffer());
                let edits: Vec<Edit> = view
                    .selection()
                    .ranges
                    .iter()
                    .map(|r| insertion(r.head, format!("\n{}", doc::indentation(&doc, r.head))))
                    .collect();
                self.edit(view, &edits);
            }
            KeyCode::Tab => self.insert_text(view, &indent_unit()),
            KeyCode::Backspace => {
                let doc = Doc::new(view.buffer());
                let edits = edits_for(view, |r| {
                    let start = doc.prev_grapheme(r.head);
                    (start < r.head).then(|| deletion(start, r.head))
                });
                self.edit(view, &edits);
            }
            KeyCode::Delete => {
                let doc = Doc::new(view.buffer());
                let edits = edits_for(view, |r| {
                    let end = doc.next_grapheme(r.head);
                    (end > r.head).then(|| deletion(r.head, end))
                });
                self.edit(view, &edits);
            }
            KeyCode::Left | KeyCode::Right => {
                let doc = Doc::new(view.buffer());
                let left = ev.code == KeyCode::Left;
                let selection = view.selection();
                let ranges = selection
                    .ranges
                    .iter()
                    .map(|r| {
                        point(if left {
                            doc.prev_grapheme(r.head)
                        } else {
                            doc.next_grapheme(r.head)
                        })
                    })
                    .collect();
                set_ranges(view, ranges, selection.primary);
            }
            KeyCode::Up | KeyCode::Down => {
                let lines = if ev.code == KeyCode::Up { -1 } else { 1 };
                let selection = view.selection();
                let ranges = selection
                    .ranges
                    .iter()
                    .map(|r| match view.move_vertically(r.head, lines, None) {
                        Ok((pos, _)) => point(pos),
                        Err(_) => *r,
                    })
                    .collect();
                set_ranges(view, ranges, selection.primary);
            }
            _ => return false,
        }
        true
    }

    fn insert_text(&mut self, view: &View, text: &str) {
        let edits: Vec<Edit> = view
            .selection()
            .ranges
            .iter()
            .map(|r| insertion(r.head, text.to_string()))
            .collect();
        self.edit(view, &edits);
    }

    /// Applies edits of this insert session as one undo step.
    fn edit(&mut self, view: &View, edits: &[Edit]) {
        if edits.is_empty() {
            return;
        }
        let undo = if self.inserted {
            UndoMode::Merge
        } else {
            UndoMode::NewStep
        };
        let version = view.buffer().version();
        if view.apply(version, edits, None, undo).is_ok() {
            self.inserted = true;
        }
    }

    fn open_prompt(&mut self, prompt: Prompt) {
        let label = prompt.label();
        let panel = Panel::new(&[vec![span(label, "")]]);
        panel.set_cursor(Some((0, label.len() as u32)));
        self.command_line = Some(CommandLine {
            prompt,
            panel,
            input: String::new(),
        });
    }

    fn command_line_key(&mut self, ev: KeyEvent) {
        let Some(line) = self.command_line.as_mut() else {
            return;
        };
        match ev.code {
            KeyCode::Escape => self.command_line = None,
            KeyCode::Enter => {
                let input = std::mem::take(&mut line.input);
                let prompt = line.prompt;
                self.command_line = None;
                self.run_prompt(prompt, input);
            }
            KeyCode::Backspace if line.input.is_empty() => self.command_line = None,
            KeyCode::Backspace => {
                line.input.pop();
            }
            KeyCode::Char(c) if ev.modifiers.is_empty() => line.input.push(c),
            _ => {}
        }
        if let Some(line) = &self.command_line {
            let text = format!("{}{}", line.prompt.label(), line.input);
            line.panel.update(&[vec![span(&text, "")]]);
            line.panel.set_cursor(Some((0, text.len() as u32)));
        }
    }

    fn run_prompt(&mut self, prompt: Prompt, input: String) {
        let view = editor::active_view();
        match prompt {
            Prompt::Command => run_command_line(input.trim()),
            _ if input.is_empty() => {}
            Prompt::Search { backward } => {
                search(&view, &input, backward);
                self.search = Some(input);
            }
            Prompt::Select => select_matches(&view, &input),
        }
    }
}

fn run_command_line(input: &str) {
    let (command, arg) = input.split_once(' ').unwrap_or((input, ""));
    let arg = arg.trim();
    let result = match command {
        "w" | "write" => save(),
        "q" | "quit" => quit(false),
        "q!" | "quit!" => quit(true),
        "wq" | "x" => save().and_then(|()| quit(false)),
        "bc" | "buffer-close" => close_buffer(false),
        "bc!" | "buffer-close!" => close_buffer(true),
        "o" | "open" | "e" | "edit" if !arg.is_empty() => {
            let args = format!(r#"{{"path":{}}}"#, json_string(arg));
            let opened = commands::call("buffer.open", &args).map(|_| ());
            to_blocks(&editor::active_view());
            opened
        }
        "o" | "open" | "e" | "edit" => Err(format!(":{command} needs a path")),
        "" => Ok(()),
        _ => Err(format!("unknown command: {command}")),
    };
    if let Err(err) = result {
        ui::show_message(&err);
    }
}

fn save() -> Result<(), String> {
    commands::call("buffer.save", "{}")?;
    let path = editor::active_view().buffer().path().unwrap_or_default();
    ui::show_message(&format!("{path} written"));
    Ok(())
}

fn close_buffer(force: bool) -> Result<(), String> {
    commands::call("buffer.close", &format!(r#"{{"force":{force}}}"#))?;
    to_blocks(&editor::active_view());
    Ok(())
}

fn quit(force: bool) -> Result<(), String> {
    commands::call("editor.quit", &format!(r#"{{"force":{force}}}"#)).map(|_| ())
}

fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The text Tab inserts, from the core's indent setting.
fn indent_unit() -> String {
    match settings::get("indent").as_deref() {
        Some("\"tab\"") => "\t".to_string(),
        Some(n) => " ".repeat(n.parse().unwrap_or(4)),
        None => "    ".to_string(),
    }
}

fn span(text: &str, style: &str) -> Span {
    Span {
        text: text.into(),
        style: style.into(),
    }
}

fn range(anchor: u64, head: u64) -> SelRange {
    SelRange { anchor, head }
}

fn point(pos: u64) -> SelRange {
    range(pos, pos)
}

fn insertion(pos: u64, text: String) -> Edit {
    Edit {
        start: pos,
        end: pos,
        text,
    }
}

fn deletion(start: u64, end: u64) -> Edit {
    Edit {
        start,
        end,
        text: String::new(),
    }
}

/// A block over the grapheme at `pos`, or a point at the end of the text.
fn block(doc: &Doc, pos: u64) -> SelRange {
    range(pos, doc.next_grapheme(pos))
}

/// Where the cursor of `r` is drawn: on the last grapheme of a forward
/// range, as the core draws it.
fn cursor(doc: &Doc, r: &SelRange) -> u64 {
    if r.head > r.anchor {
        doc.prev_grapheme(r.head)
    } else {
        r.head
    }
}

/// `r` grown so its cursor is at `pos`, keeping the grapheme at its anchor
/// side selected.
fn extend(doc: &Doc, r: &SelRange, pos: u64) -> SelRange {
    let anchor = if r.head >= r.anchor {
        r.anchor
    } else {
        doc.prev_grapheme(r.anchor)
    };
    if pos >= anchor {
        range(anchor, doc.next_grapheme(pos))
    } else {
        range(doc.next_grapheme(anchor), pos)
    }
}

/// The next grapheme, but not past the last one: the cursor stays on text.
fn next_char(doc: &Doc, pos: u64) -> u64 {
    let next = doc.next_grapheme(pos);
    if next < doc.len { next } else { pos }
}

/// The last line with text: a final line break does not start a new line
/// to jump to.
fn last_line(doc: &Doc) -> u64 {
    let last = doc.line_count().saturating_sub(1);
    if last > 0 && doc.line_start(last) == doc.len {
        last - 1
    } else {
        last
    }
}

fn set_ranges(view: &View, ranges: Vec<SelRange>, primary: u32) {
    let primary = primary.min(ranges.len().saturating_sub(1) as u32);
    let _ = view.set_selection(&Selection { ranges, primary });
}

/// Turns every range into a block at its cursor, as after leaving insert
/// mode or undoing.
fn to_blocks(view: &View) {
    let doc = Doc::new(view.buffer());
    let selection = view.selection();
    let ranges = selection
        .ranges
        .iter()
        .map(|r| block(&doc, cursor(&doc, r)))
        .collect();
    set_ranges(view, ranges, selection.primary);
}

fn flip_selections(view: &View) {
    let selection = view.selection();
    let ranges = selection
        .ranges
        .iter()
        .map(|r| range(r.head, r.anchor))
        .collect();
    set_ranges(view, ranges, selection.primary);
}

/// `x`: selects the lines of each range, or one more line when whole lines
/// are selected already.
fn select_lines(view: &View) {
    let doc = Doc::new(view.buffer());
    let selection = view.selection();
    let ranges = selection
        .ranges
        .iter()
        .map(|r| {
            let (from, to) = (r.anchor.min(r.head), r.anchor.max(r.head));
            let start = doc.line_start(doc.line_of(from));
            let whole_lines =
                from == start && to > from && to < doc.len && doc.line_start(doc.line_of(to)) == to;
            let last = if whole_lines {
                to
            } else {
                doc.prev_grapheme(to).max(from)
            };
            let end = (doc.line_end(last) + 1).min(doc.len);
            range(start, end)
        })
        .collect();
    set_ranges(view, ranges, selection.primary);
}

/// `C`: adds a cursor on the line below each range's cursor.
fn copy_to_next_line(view: &View) {
    let doc = Doc::new(view.buffer());
    let selection = view.selection();
    let mut ranges = selection.ranges.clone();
    let mut primary = selection.primary;
    for (i, r) in selection.ranges.iter().enumerate() {
        let pos = cursor(&doc, r);
        let Ok((below, _)) = view.move_vertically(pos, 1, None) else {
            continue;
        };
        if doc.line_of(below) != doc.line_of(pos) {
            ranges.push(block(&doc, below));
            if i as u32 == selection.primary {
                primary = (ranges.len() - 1) as u32;
            }
        }
    }
    set_ranges(view, ranges, primary);
}

fn delete_selections(view: &View) {
    let edits = edits_for(view, |r| {
        let (from, to) = (r.anchor.min(r.head), r.anchor.max(r.head));
        (from < to).then(|| deletion(from, to))
    });
    apply(view, &edits);
}

/// Applies `edits` as a new undo step, mapping the selections through them.
fn apply(view: &View, edits: &[Edit]) -> bool {
    if edits.is_empty() {
        return false;
    }
    let version = view.buffer().version();
    view.apply(version, edits, None, UndoMode::NewStep).is_ok()
}

/// Applies one edit per selection and makes each selection
/// `start..end`, counted in bytes from the start of its edit's text.
fn apply_placing(view: &View, mut changes: Vec<(Edit, u64, u64)>, undo: UndoMode) -> bool {
    changes.sort_by_key(|(edit, _, _)| edit.start);
    let mut shift = 0i64;
    let ranges = changes
        .iter()
        .map(|(edit, start, end)| {
            let at = (edit.start as i64 + shift) as u64;
            shift += edit.text.len() as i64 - (edit.end - edit.start) as i64;
            range(at + start, at + end)
        })
        .collect::<Vec<_>>();
    let edits: Vec<Edit> = changes.into_iter().map(|(edit, _, _)| edit).collect();
    let primary = view.selection().primary;
    let after = Selection {
        primary: primary.min(ranges.len().saturating_sub(1) as u32),
        ranges,
    };
    let version = view.buffer().version();
    view.apply(version, &edits, Some(&after), undo).is_ok()
}

/// `r`: replaces every char of each selection with `c`, keeping line breaks.
fn replace_with(view: &View, c: char) {
    let doc = Doc::new(view.buffer());
    let changes = view
        .selection()
        .ranges
        .iter()
        .filter(|r| r.anchor != r.head)
        .map(|r| {
            let (from, to) = (r.anchor.min(r.head), r.anchor.max(r.head));
            let text: String = doc
                .slice(from, to)
                .chars()
                .map(|ch| if ch == '\n' { ch } else { c })
                .collect();
            let len = text.len() as u64;
            let (start, end) = if r.head < r.anchor {
                (len, 0)
            } else {
                (0, len)
            };
            (
                Edit {
                    start: from,
                    end: to,
                    text,
                },
                start,
                end,
            )
        })
        .collect();
    apply_placing(view, changes, UndoMode::NewStep);
}

/// The lines each selection touches, each once.
fn selected_lines(doc: &Doc, view: &View) -> Vec<u64> {
    let mut lines: Vec<u64> = view
        .selection()
        .ranges
        .iter()
        .flat_map(|r| {
            let (from, to) = (r.anchor.min(r.head), r.anchor.max(r.head));
            let last = doc.prev_grapheme(to).max(from);
            doc.line_of(from)..=doc.line_of(last)
        })
        .collect();
    lines.sort_unstable();
    lines.dedup();
    lines
}

/// `>` and `<`: indents or unindents the selected lines by one unit.
fn indent(view: &View, more: bool) {
    let doc = Doc::new(view.buffer());
    let unit = indent_unit();
    let width = if unit == "\t" { 1 } else { unit.len() };
    let edits: Vec<Edit> = selected_lines(&doc, view)
        .into_iter()
        .filter_map(|line| {
            let start = doc.line_start(line);
            let leading = doc::indentation(&doc, start);
            if more {
                (doc.line_end(start) > start).then(|| insertion(start, unit.clone()))
            } else {
                // A tab counts as a whole unit.
                let mut end = start;
                for (i, c) in leading.chars().enumerate() {
                    if i >= width {
                        break;
                    }
                    end += c.len_utf8() as u64;
                    if c == '\t' {
                        break;
                    }
                }
                (end > start).then(|| deletion(start, end))
            }
        })
        .collect();
    apply(view, &edits);
}

/// `J`: joins the selected lines, or the line with the next one, replacing
/// each line break and the next line's indentation with a space.
fn join_lines(view: &View) {
    let doc = Doc::new(view.buffer());
    let mut breaks: Vec<u64> = view
        .selection()
        .ranges
        .iter()
        .flat_map(|r| {
            let (from, to) = (r.anchor.min(r.head), r.anchor.max(r.head));
            let first = doc.line_of(from);
            let last = doc.line_of(doc.prev_grapheme(to).max(from)).max(first + 1);
            first..last
        })
        .collect();
    breaks.sort_unstable();
    breaks.dedup();
    let edits: Vec<Edit> = breaks
        .into_iter()
        .filter(|&line| line + 1 < doc.line_count())
        .map(|line| {
            let newline = doc.line_end(doc.line_start(line));
            let next = newline + 1;
            let text_start = doc::first_non_blank(&doc, next);
            let empty = text_start == doc.line_end(next);
            Edit {
                start: newline,
                end: text_start,
                text: if empty { String::new() } else { " ".into() },
            }
        })
        .collect();
    apply(view, &edits);
}

fn edits_for(view: &View, edit: impl Fn(&SelRange) -> Option<Edit>) -> Vec<Edit> {
    view.selection().ranges.iter().filter_map(edit).collect()
}

/// Selects the next match of `pattern` after the primary selection, or the
/// previous one before it, wrapping around the buffer.
fn search(view: &View, pattern: &str, backward: bool) {
    let buffer = view.buffer();
    let selection = view.selection();
    let r = selection.ranges[selection.primary as usize];
    let (from, to) = (r.anchor.min(r.head), r.anchor.max(r.head));
    let (start, wrap_start) = if backward {
        (from, buffer.len())
    } else {
        (to, 0)
    };
    let found = match buffer.find(pattern, start, backward) {
        Ok(Some(found)) => Some((found, false)),
        Ok(None) => match buffer.find(pattern, wrap_start, backward) {
            Ok(found) => found.map(|found| (found, true)),
            Err(err) => return ui::show_message(&describe(err)),
        },
        Err(err) => return ui::show_message(&describe(err)),
    };
    let Some(((start, end), wrapped)) = found else {
        return ui::show_message(&format!("no matches for {pattern}"));
    };
    let doc = Doc::new(buffer);
    let found = if start == end {
        block(&doc, start)
    } else {
        range(start, end)
    };
    set_ranges(view, vec![found], 0);
    if wrapped {
        ui::show_message("search wrapped around");
    }
}

/// `s`: replaces the selections with the matches of `pattern` inside them.
fn select_matches(view: &View, pattern: &str) {
    let buffer = view.buffer();
    let mut ranges = Vec::new();
    for r in view.selection().ranges {
        match buffer.find_all(pattern, r.anchor.min(r.head), r.anchor.max(r.head)) {
            Ok(found) => ranges.extend(
                found
                    .into_iter()
                    .filter(|(start, end)| start < end)
                    .map(|(start, end)| range(start, end)),
            ),
            Err(err) => return ui::show_message(&describe(err)),
        }
    }
    if ranges.is_empty() {
        ui::show_message(&format!("no matches for {pattern}"));
    } else {
        set_ranges(view, ranges, 0);
    }
}

/// `mi` and `ma`: selects inside or around the pair of `c` around each
/// cursor.
/// Calls another plugin's command, showing its error if it fails.
fn call_or_show(command: &str) {
    if let Err(err) = commands::call(command, "") {
        ui::show_message(&err);
    }
}

/// Highlights the bracket that pairs with the one at the primary cursor.
/// Only the syntax tree is asked: searching the text for a bracket without
/// a pair would scan to the end of the file on every key.
fn highlight_match(view: &View) {
    let doc = Doc::new(view.buffer());
    let selection = view.selection();
    let pos = cursor(&doc, &selection.ranges[selection.primary as usize]);
    let decorations: Vec<Decoration> = tree::matching_pair(&doc.buffer, pos)
        .map(|other| Decoration {
            start: other,
            end: other + 1,
            style: "ui.cursor.match".into(),
        })
        .into_iter()
        .collect();
    ui::set_decorations(&doc.buffer, "match", &decorations);
}

/// `mi` and `ma`: selects a text object, or the inside or all of a pair.
fn select_pairs(view: &View, c: char, around: bool) {
    let doc = Doc::new(view.buffer());
    let selection = view.selection();
    let object = tree::object_name(c).map(|name| {
        let part = if around { "around" } else { "inside" };
        format!("{name}.{part}")
    });
    let ranges = selection
        .ranges
        .iter()
        .map(|r| {
            let pos = cursor(&doc, r);
            if let Some(capture) = &object {
                return tree::object_around(&doc.buffer, capture, pos)
                    .map_or(*r, |(start, end)| range(start, end));
            }
            // The tree knows which brackets are in strings and comments;
            // the text is the fallback, e.g. inside a comment.
            let pair = tree::surrounding_pair(&doc.buffer, pos, c)
                .or_else(|| doc::surrounding_pair(&doc, pos, c));
            match pair {
                Some((open, close)) if around => range(open, close + 1),
                Some((open, close)) => range(open + 1, close),
                None => *r,
            }
        })
        .collect();
    set_ranges(view, ranges, selection.primary);
}

/// `]f`, `[f`, and so on: selects the next or previous text object.
fn goto_object(view: &View, c: char, forward: bool, count: u64) {
    let Some(name) = tree::object_name(c) else {
        return;
    };
    // Jumping between arguments means landing on them, not their commas.
    let part = if c == 'a' { "inside" } else { "around" };
    let capture = format!("{name}.{part}");
    let doc = Doc::new(view.buffer());
    let selection = view.selection();
    let ranges = selection
        .ranges
        .iter()
        .map(|r| {
            // Backward from the start of the selection, so `[f` after `]f`
            // does not find the function it selected.
            let from = if forward {
                cursor(&doc, r)
            } else {
                r.anchor.min(r.head)
            };
            let found = tree::next_object(&doc.buffer, &capture, from, forward, count);
            match found {
                Some((start, end)) if forward => range(start, end),
                Some((start, end)) => range(end, start),
                None => *r,
            }
        })
        .collect();
    set_ranges(view, ranges, selection.primary);
}

/// `Alt-n` and `Alt-p`: selects the next or previous syntax node.
fn select_siblings(view: &View, forward: bool) {
    let buffer = view.buffer();
    let selection = view.selection();
    let ranges = selection
        .ranges
        .iter()
        .map(|r| {
            let (start, end) = (r.anchor.min(r.head), r.anchor.max(r.head));
            tree::sibling(&buffer, start, end, forward).map_or(*r, |(s, e)| range(s, e))
        })
        .collect();
    set_ranges(view, ranges, selection.primary);
}

fn describe(err: editor::Error) -> String {
    match err {
        editor::Error::InvalidPattern(message) => message,
        other => format!("{other:?}"),
    }
}

/// Escapes regex syntax, so `*` searches for the selected text as is.
fn regex_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if "\\.+*?()|[]{}^$#&-~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}
