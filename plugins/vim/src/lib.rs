//! Vim's way of editing (docs/design/bases/vim.md): normal, insert, replace, and visual
//! modes, operators with motions and text objects, counts, registers,
//! marks, macros, `.`, and the `:` command line. Where Vim and Neovim
//! differ, this follows Neovim's defaults.
//!
//! In normal mode the cursor is a point on the char it sits on, drawn as a
//! block. Visual mode keeps its two ends as marks and selects what is
//! between them.

mod command;
mod ex;
mod insert;
mod motion;
mod normal;
mod object;
mod parse;
mod pattern;
mod register;
mod text;
mod visual;

use std::cell::RefCell;
use std::collections::BTreeMap;

use base_kit::doc::{Doc, FindKind};
use base_kit::keys::{self, Binding, Keymap, Sequence, Step};
use base_kit::{call_or_show, hints, leader, span};
use nib_plugin::exports::nib::plugin::guest::{Guest, KeyResult};
use nib_plugin::nib::plugin::events::Event;
use nib_plugin::nib::plugin::input;
use nib_plugin::nib::plugin::prompt::{self as prompts, Action as PromptAction};
use nib_plugin::nib::plugin::types::{
    CursorShape, Edit, KeyCode, KeyEvent, Modifiers, SelRange, Selection, UndoMode,
};
use nib_plugin::nib::plugin::ui::{self, Popup, PopupAnchor, Side};
use nib_plugin::nib::plugin::view::{self, View};

use command::Prompting;
use parse::{Cmd, VisualKind};
use register::Registers;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Normal,
    Insert,
    Replace,
    Visual(VisualKind),
}

/// An insert or replace session: from the command that started it to
/// Escape.
pub struct Session {
    /// The command that started it, for `.` and its count.
    cmd: Option<Cmd>,
    /// Keys typed in it, for `.` and the count.
    keys: Vec<KeyEvent>,
    /// Indent added by Enter, `o`, or `O` on a line nothing was typed on
    /// yet: where the line starts. Leaving the line takes it away again.
    autoindent: Option<u64>,
    /// Replace mode: what each typed char replaced, for Backspace.
    replaced: Vec<(u64, Option<char>)>,
    /// Ctrl-r or Ctrl-v wait for the next key.
    waiting: Option<char>,
    /// Started on a block: Escape goes back to its corner.
    to_corner: bool,
}

/// The last change, for `.`.
#[derive(Clone)]
struct Change {
    cmd: Cmd,
    /// What was typed in the insert session it started, Escape included.
    inserted: Vec<KeyEvent>,
}

pub struct Keymaps {
    normal: Keymap,
    insert: Keymap,
    visual: Keymap,
}

pub struct Vim {
    mode: Mode,
    /// Keys of the command being typed.
    keys: Vec<KeyEvent>,
    /// The display column `j` and `k` aim for; `u32::MAX` after `$`.
    column: Option<u32>,
    registers: Registers,
    last_find: Option<(FindKind, char)>,
    /// The last search: vim's pattern, and whether it went backward.
    search: Option<(String, bool)>,
    /// The last `:s`: pattern, replacement, and flags.
    substitute: Option<(String, String, String)>,
    /// The current command has made its first edit, so later ones join it
    /// in one undo step.
    step_open: bool,
    session: Option<Session>,
    last_change: Option<Change>,
    /// Keys are being replayed by `.`, a count, a mapping, or a macro, so
    /// mappings are not looked up again.
    replaying: u32,
    /// `.` or a count is doing a change again: what is typed is not kept
    /// as the last change.
    dotting: bool,
    /// A command failed, as a motion that cannot move: a macro stops.
    failed: bool,
    /// `q`: the register, and the keys so far.
    recording: Option<(char, Vec<KeyEvent>)>,
    macros: BTreeMap<char, Vec<KeyEvent>>,
    last_macro: Option<char>,
    last_ex: Option<String>,
    prompting: Option<Prompting>,
    /// Earlier `:` lines and searches, oldest first.
    history: BTreeMap<char, Vec<String>>,
    last_visual: Option<VisualKind>,
    /// Ctrl-o in insert mode: one normal command, then back.
    one_command: bool,
    /// Where each buffer is in its jump list, by path.
    jump_at: BTreeMap<String, usize>,
    /// `A` to `Z`: the path of the buffer each is in.
    file_marks: BTreeMap<char, String>,
    keymaps: Keymaps,
    /// Keys of the settings or under the leader, typed so far.
    sequence: Sequence,
    leader: KeyEvent,
    hints: Option<Popup>,
}

thread_local! {
    static VIM: RefCell<Option<Vim>> = const { RefCell::new(None) };
}

fn with_vim<R>(f: impl FnOnce(&mut Vim) -> R) -> R {
    VIM.with_borrow_mut(|vim| f(vim.get_or_insert_with(Vim::new)))
}

struct Plugin;

impl Guest for Plugin {
    fn init(config: String) -> Result<(), String> {
        let settings: serde_json::Value =
            serde_json::from_str(&config).map_err(|err| err.to_string())?;
        let mut errors = Vec::new();
        let mut keymap = |mode: &str| {
            let (keymap, mut wrong) = keys::keymap(&settings, mode, &vim_keys);
            errors.append(&mut wrong);
            keymap
        };
        let keymaps = Keymaps {
            normal: keymap("normal"),
            insert: keymap("insert"),
            visual: keymap("visual"),
        };
        let leader = match settings["leader"].as_str() {
            Some(text) => match keys::parse_key(text) {
                Ok(key) => key,
                Err(err) => {
                    errors.push(format!("leader: {err}"));
                    space()
                }
            },
            None => space(),
        };
        if let Some(first) = errors.first() {
            let more = match errors.len() {
                1 => String::new(),
                n => format!(" (and {} more)", n - 1),
            };
            ui::show_message(&format!("vim.toml: {first}{more}; left out"));
        }
        input::push_layer();
        with_vim(|vim| {
            vim.keymaps = keymaps;
            vim.leader = leader;
            let view = view::active();
            vim.set_mode(Mode::Normal);
            let pos = vim.cursor(&view);
            vim.place(&view, pos);
        });
        Ok(())
    }

    fn handle_key(ev: KeyEvent) -> KeyResult {
        with_vim(|vim| vim.handle_key(ev))
    }

    fn handle_paste(text: String) -> KeyResult {
        with_vim(|vim| vim.paste(&text))
    }

    fn run_command(name: String, _args: String) -> Result<String, String> {
        Err(format!("no command {name}"))
    }

    fn on_event(ev: Event) {
        match ev {
            Event::PromptChanged(change) => with_vim(|vim| vim.prompt_changed(change.id)),
            Event::PromptAction(act) => with_vim(|vim| vim.prompt_action(act.id, act.action)),
            _ => {}
        }
    }
}

nib_plugin::export!(Plugin);

/// The keys a plugin's buffer cannot take from vim in normal mode:
/// motions, counts, search, the command line, visual mode, yanking,
/// windows, scrolling, marks, and the leader (docs/design/bases/vim.md).
fn vim_keeps(key: &KeyEvent, leader: &KeyEvent) -> bool {
    if key == leader {
        return true;
    }
    match (plain(key), ctrl(key), key.code) {
        (Some(c), ..) => "hjklwWbBeE0123456789^$gG/?nN*#:fFtT;,{}HMLvVy\"'`mz".contains(c),
        (_, Some(c), _) => "wdufbeyoiv[c".contains(c),
        (None, None, code) => matches!(
            code,
            KeyCode::Escape
                | KeyCode::Up
                | KeyCode::Down
                | KeyCode::Left
                | KeyCode::Right
                | KeyCode::Home
                | KeyCode::End
                | KeyCode::PageUp
                | KeyCode::PageDown
        ),
    }
}

fn space() -> KeyEvent {
    KeyEvent {
        code: KeyCode::Char(' '),
        modifiers: Modifiers::empty(),
    }
}

pub fn plain(ev: &KeyEvent) -> Option<char> {
    match ev.code {
        KeyCode::Char(c) if ev.modifiers.is_empty() => Some(c),
        _ => None,
    }
}

pub fn ctrl(ev: &KeyEvent) -> Option<char> {
    match ev.code {
        KeyCode::Char(c) if ev.modifiers == Modifiers::CTRL => Some(c),
        _ => None,
    }
}

fn is_escape(ev: &KeyEvent) -> bool {
    (ev.code == KeyCode::Escape && ev.modifiers.is_empty()) || matches!(ctrl(ev), Some('[' | 'c'))
}

/// Keys written as vim writes them in mappings (`dd`, `<C-w>v`, `:w<CR>`),
/// for the right side of `[settings.keys.*]`.
pub fn vim_keys(text: &str) -> Option<Vec<KeyEvent>> {
    let mut keys = Vec::new();
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if c == '<'
            && let Some(end) = rest.find('>')
            && let Some(key) = vim_key_name(&rest[1..end])
        {
            keys.push(key);
            rest = &rest[end + 1..];
            continue;
        }
        keys.push(KeyEvent {
            code: KeyCode::Char(c),
            modifiers: Modifiers::empty(),
        });
        rest = &rest[c.len_utf8()..];
    }
    (!keys.is_empty()).then_some(keys)
}

fn vim_key_name(name: &str) -> Option<KeyEvent> {
    let lower = name.to_ascii_lowercase();
    let plain = |code| KeyEvent {
        code,
        modifiers: Modifiers::empty(),
    };
    Some(match lower.as_str() {
        "esc" => plain(KeyCode::Escape),
        "cr" | "enter" | "return" => plain(KeyCode::Enter),
        "tab" => plain(KeyCode::Tab),
        "bs" => plain(KeyCode::Backspace),
        "del" => plain(KeyCode::Delete),
        "space" => plain(KeyCode::Char(' ')),
        "lt" => plain(KeyCode::Char('<')),
        "bar" => plain(KeyCode::Char('|')),
        "bslash" => plain(KeyCode::Char('\\')),
        "up" => plain(KeyCode::Up),
        "down" => plain(KeyCode::Down),
        "left" => plain(KeyCode::Left),
        "right" => plain(KeyCode::Right),
        "home" => plain(KeyCode::Home),
        "end" => plain(KeyCode::End),
        _ => {
            let (modifier, key) = name.split_once('-')?;
            let modifiers = match modifier.to_ascii_uppercase().as_str() {
                "C" => Modifiers::CTRL,
                "M" | "A" => Modifiers::ALT,
                _ => return None,
            };
            let mut chars = key.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else {
                let mut key = vim_key_name(key)?;
                key.modifiers |= modifiers;
                return Some(key);
            };
            let c = if modifiers == Modifiers::CTRL {
                c.to_ascii_lowercase()
            } else {
                c
            };
            KeyEvent {
                code: KeyCode::Char(c),
                modifiers,
            }
        }
    })
}

impl Vim {
    fn new() -> Self {
        Self {
            mode: Mode::Normal,
            keys: Vec::new(),
            column: None,
            registers: Registers::default(),
            last_find: None,
            search: None,
            substitute: None,
            step_open: false,
            session: None,
            last_change: None,
            replaying: 0,
            dotting: false,
            failed: false,
            recording: None,
            macros: BTreeMap::new(),
            last_macro: None,
            last_ex: None,
            prompting: None,
            history: BTreeMap::new(),
            last_visual: None,
            one_command: false,
            jump_at: BTreeMap::new(),
            file_marks: BTreeMap::new(),
            keymaps: Keymaps {
                normal: Keymap::new(),
                insert: Keymap::new(),
                visual: Keymap::new(),
            },
            sequence: Sequence::default(),
            leader: space(),
            hints: None,
        }
    }

    fn handle_key(&mut self, ev: KeyEvent) -> KeyResult {
        // Keys come here for any prompt, this plugin's or another's.
        if prompts::active().is_some() {
            if let Some((_, keys)) = &mut self.recording {
                keys.push(ev);
            }
            return self.prompt_key(ev);
        }
        // Keys for a list another plugin shows, such as completions.
        if let Some(offer) = prompts::offered()
            && let Some(action) = choice_action(ev).filter(|a| offer.actions.contains(a))
        {
            prompts::act(action);
            return KeyResult::Handled;
        }
        if let Some((_, keys)) = &mut self.recording {
            keys.push(ev);
        }
        self.failed = false;
        let handled = match self.sequence_key(ev) {
            Some(handled) => handled,
            None => self.dispatch(ev),
        };
        self.show_hints();
        self.show_status();
        if handled {
            KeyResult::Handled
        } else {
            KeyResult::Pass
        }
    }

    /// Keys of the settings, and plugins' keys under the leader. `None`
    /// when the key is neither.
    fn sequence_key(&mut self, ev: KeyEvent) -> Option<bool> {
        if !self.keys.is_empty() || self.replaying > 0 {
            return None;
        }
        let mut keymap = match self.mode {
            Mode::Normal => self.keymaps.normal.clone(),
            Mode::Insert | Mode::Replace => self.keymaps.insert.clone(),
            Mode::Visual(_) => self.keymaps.visual.clone(),
        };
        // In normal mode, the shown buffer's keys come before vim's own,
        // but for those vim keeps; the settings' keys come first.
        if self.mode == Mode::Normal && !self.sequence.is_waiting() {
            let mut table = keys::buffer_keymap(&|key| vim_keeps(key, &self.leader));
            if !table.is_empty() {
                keys::merge(&mut table, keymap);
                keymap = table;
            }
        }
        let keymap = &keymap;
        let waiting = self.sequence.is_waiting();
        let in_leader = !waiting
            && ev == self.leader
            && matches!(self.mode, Mode::Normal | Mode::Visual(_))
            && keys::lookup(keymap, &ev).is_none();
        if in_leader {
            let table = leader::keymap(&[], input::leader_keys()).keymap;
            self.sequence.enter(table, vec![ev]);
            return Some(true);
        }
        if keymap.is_empty() && !waiting {
            return None;
        }
        let keymap = keymap.clone();
        match self.sequence.key(&keymap, ev) {
            Step::NotMine => None,
            Step::Wait => Some(true),
            Step::Dropped(typed) => {
                // Typed in insert mode, the keys were text after all.
                if matches!(self.mode, Mode::Insert | Mode::Replace) {
                    for key in typed {
                        self.dispatch(key);
                    }
                }
                Some(true)
            }
            Step::Run(Binding::Command(name), _) => {
                match base_kit::own_command(&name, "vim") {
                    Some(own) => {
                        if let Err(err) = self.run_own(own) {
                            ui::show_message(&err);
                        }
                    }
                    None => call_or_show(&name),
                }
                Some(true)
            }
            Step::Run(Binding::Keys(keys), _) => {
                self.replaying += 1;
                for key in keys {
                    self.dispatch(key);
                }
                self.replaying -= 1;
                Some(true)
            }
            Step::Run(Binding::Prefix(_), _) => Some(true),
        }
    }

    /// Vim's own commands by their dotted names, as its listings' keys and
    /// the command line run them.
    fn run_own(&mut self, name: &str) -> Result<(), String> {
        match name {
            "close-listing" => {
                base_kit::close_listing();
                Ok(())
            }
            _ => Err(format!("no command vim.{name}")),
        }
    }

    /// The built-in keys of the mode.
    fn dispatch(&mut self, ev: KeyEvent) -> bool {
        match self.mode {
            Mode::Normal => self.normal_key(ev),
            Mode::Insert | Mode::Replace => {
                self.insert_key(ev);
                true
            }
            Mode::Visual(kind) => self.visual_key(ev, kind),
        }
    }

    /// Replays keys as if typed, as `.`, counts, and macros do.
    /// Stops at a command that fails, as vim stops a macro.
    fn replay(&mut self, keys: &[KeyEvent]) {
        self.replaying += 1;
        for &key in keys {
            if prompts::active().is_some() {
                self.prompt_key_replayed(key);
            } else {
                self.dispatch(key);
            }
            if self.failed {
                break;
            }
        }
        self.replaying -= 1;
    }

    fn show_hints(&mut self) {
        self.hints = self.sequence.waiting().map(|(table, typed)| {
            Popup::new(PopupAnchor::Corner, &hints::keymap_lines(typed, table))
        });
    }

    fn show_status(&self) {
        let mut line = Vec::new();
        if let Some((register, _)) = self.recording {
            line.push(span(&format!("recording @{register} "), "ui.text"));
        }
        let typed: String = self.keys.iter().map(key_label).collect();
        if !typed.is_empty() {
            line.push(span(&typed, "ui.text"));
        }
        if line.is_empty() {
            ui::remove_status("pending");
        } else {
            ui::set_status("pending", Side::Right, -10, &line);
        }
    }

    fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        let (name, label, style, shape) = match mode {
            Mode::Normal => ("normal", " NORMAL ", "ui.mode.normal", CursorShape::Block),
            Mode::Insert => ("insert", " INSERT ", "ui.mode.insert", CursorShape::Bar),
            Mode::Replace => (
                "replace",
                " REPLACE ",
                "ui.mode.insert",
                CursorShape::Underline,
            ),
            Mode::Visual(VisualKind::Chars) => {
                ("visual", " VISUAL ", "ui.mode.select", CursorShape::Block)
            }
            Mode::Visual(VisualKind::Lines) => {
                ("visual", " V-LINE ", "ui.mode.select", CursorShape::Block)
            }
            Mode::Visual(VisualKind::Block) => {
                ("visual", " V-BLOCK ", "ui.mode.select", CursorShape::Block)
            }
        };
        view::active().set_cursor_shape(shape);
        ui::set_status("mode", Side::Left, 0, &[span(label, style)]);
        // For other plugins, such as a status line that shows the mode, or
        // completions that come while typing.
        input::set_mode(name, matches!(mode, Mode::Insert | Mode::Replace));
    }

    /// Pasted text, as Neovim puts it: typed in insert mode, after the
    /// cursor in normal mode, over the selection in visual mode, and into
    /// a prompt by the core.
    fn paste(&mut self, text: &str) -> KeyResult {
        if prompts::active().is_some() {
            return KeyResult::Pass;
        }
        let view = view::active();
        match self.mode {
            Mode::Insert | Mode::Replace => self.type_text(&view, text),
            Mode::Normal => {
                self.step_open = false;
                let doc = Doc::new(view.buffer());
                let pos = self.cursor(&view);
                let at = if pos >= doc.line_end(pos) {
                    pos
                } else {
                    doc.next_grapheme(pos)
                };
                let end = at + text.len() as u64;
                self.edit(
                    &view,
                    vec![base_kit::edit::insertion(at, text.to_string())],
                    Some(vec![at]),
                );
                let doc = Doc::new(view.buffer());
                let pos = doc.prev_grapheme(end).max(at);
                self.place(&view, pos);
            }
            Mode::Visual(_) => {
                self.step_open = false;
                let edits: Vec<Edit> = view
                    .selection()
                    .ranges
                    .iter()
                    .map(|r| Edit {
                        start: r.anchor.min(r.head),
                        end: r.anchor.max(r.head),
                        text: text.to_string(),
                    })
                    .collect();
                let first = edits.iter().map(|e| e.start).min().unwrap_or(0);
                self.set_mode(Mode::Normal);
                self.edit(&view, edits, Some(vec![first]));
                let doc = Doc::new(view.buffer());
                let end = doc.prev_grapheme(first + text.len() as u64).max(first);
                self.place(&view, end);
            }
        }
        self.show_status();
        KeyResult::Handled
    }

    /// The cursor: the primary point, or in visual mode, its moving end.
    fn cursor(&self, view: &View) -> u64 {
        if let Mode::Visual(_) = self.mode
            && let [_, cursor] = view.buffer().marks("visual")[..]
        {
            return cursor;
        }
        let selection = view.selection();
        let r = selection.ranges[selection.primary as usize];
        if r.head > r.anchor {
            view.buffer().prev_grapheme(r.head).unwrap_or(r.head)
        } else {
            r.head
        }
    }

    /// Puts the cursor at `pos`, on a char as normal mode keeps it.
    fn place(&mut self, view: &View, pos: u64) {
        let doc = Doc::new(view.buffer());
        let pos = clamp_normal(&doc, pos);
        set_points(view, &[pos]);
    }

    /// Applies `edits` in the current undo step, or a new one for the first
    /// edit of a command. `after` is where the cursors go.
    fn edit(&mut self, view: &View, edits: Vec<Edit>, after: Option<Vec<u64>>) -> bool {
        if edits.is_empty() {
            return false;
        }
        let undo = if self.step_open {
            UndoMode::Merge
        } else {
            UndoMode::NewStep
        };
        let after = after.map(|points| Selection {
            ranges: points
                .iter()
                .map(|&p| SelRange { anchor: p, head: p })
                .collect(),
            primary: 0,
        });
        let version = view.buffer().version();
        let done = view.apply(version, &edits, after.as_ref(), undo).is_ok();
        if done {
            self.step_open = true;
            let first = edits.iter().map(|e| e.start).min().unwrap_or(0);
            view.buffer().set_marks(".", &[first]);
        }
        done
    }
}

/// Where normal mode can have the cursor: on a char, not on the line break
/// of a line with text, and not on the empty line after a final line
/// break.
pub fn clamp_normal(doc: &Doc, pos: u64) -> u64 {
    let pos = pos.min(doc.len);
    let last = doc.last_line();
    let pos = if doc.line_of(pos) > last {
        doc.line_end(doc.line_start(last))
    } else {
        pos
    };
    let start = doc.line_start(doc.line_of(pos));
    if pos == doc.line_end(pos) && pos > start {
        doc.prev_grapheme(pos)
    } else {
        pos
    }
}

pub fn set_points(view: &View, points: &[u64]) {
    let ranges = points
        .iter()
        .map(|&p| SelRange { anchor: p, head: p })
        .collect();
    let _ = view.set_selection(&Selection { ranges, primary: 0 });
}

/// A key as vim shows typed keys.
fn key_label(key: &KeyEvent) -> String {
    match (key.code, key.modifiers) {
        (KeyCode::Char(c), m) if m.is_empty() => c.to_string(),
        (KeyCode::Char(c), m) if m == Modifiers::CTRL => format!("^{}", c.to_ascii_uppercase()),
        _ => format!("<{}>", keys::label(key)),
    }
}

/// What a key does to a list shown without a line to type into, as in
/// vim's completion menu.
fn choice_action(ev: KeyEvent) -> Option<PromptAction> {
    let plain = ev.modifiers.is_empty();
    match (ctrl(&ev), ev.code) {
        (Some('n'), _) => Some(PromptAction::Next),
        (Some('p'), _) => Some(PromptAction::Previous),
        (Some('y'), _) => Some(PromptAction::Accept),
        (Some('e'), _) => Some(PromptAction::Cancel),
        (_, KeyCode::Down) if plain => Some(PromptAction::Next),
        (_, KeyCode::Up) if plain => Some(PromptAction::Previous),
        (_, KeyCode::Tab | KeyCode::Enter) if plain => Some(PromptAction::Accept),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mappings_read_vims_key_names() {
        let keys = vim_keys(":w<CR><C-w>v<lt><Esc>").unwrap();
        assert_eq!(keys.len(), 7);
        assert_eq!(keys[2].code, KeyCode::Enter);
        assert_eq!(
            keys[3],
            KeyEvent {
                code: KeyCode::Char('w'),
                modifiers: Modifiers::CTRL
            }
        );
        assert_eq!(keys[5].code, KeyCode::Char('<'));
        assert_eq!(keys[6].code, KeyCode::Escape);
        assert_eq!(vim_keys("<nope>").unwrap().len(), 6);
    }
}
