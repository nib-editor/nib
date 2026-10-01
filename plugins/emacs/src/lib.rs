//! GNU Emacs's way of editing (docs/design/bases/emacs.md): no modes, typed chars go
//! in, and Control and Meta keys, with prefixes such as `C-x`, do the rest.
//! The point is the cursor; the mark and the mark ring are the core's
//! marks, and while the region is active it is drawn as the selection from
//! the mark to the point.

mod bind;
mod command;
mod minibuffer;
mod motion;
mod rect;
mod register;
mod search;

use std::cell::RefCell;
use std::collections::BTreeMap;

use base_kit::edit::{point, range, set_ranges};
use base_kit::keys::{self, Binding, Keymap, Sequence, Step};
use base_kit::{call_or_show, hints, leader, span};
use nib_plugin::exports::nib::plugin::guest::{Guest, KeyResult};
use nib_plugin::nib::plugin::buffer::Buffer;
use nib_plugin::nib::plugin::events::Event;
use nib_plugin::nib::plugin::prompt::{self as prompts, Action};
use nib_plugin::nib::plugin::types::{
    CursorShape, Edit, KeyCode, KeyEvent, Modifiers, SelRange, Selection, UndoMode,
};
use nib_plugin::nib::plugin::ui::{self, Popup, PopupAnchor, Side};
use nib_plugin::nib::plugin::view::{self, View};
use nib_plugin::nib::plugin::{clipboard, input};

use minibuffer::Asking;
use register::Register;
use search::{Isearch, Query};

/// The kill ring holds this many kills, as `kill-ring-max`.
const KILL_RING_MAX: usize = 120;
/// The mark ring holds this many positions, as `mark-ring-max`.
const MARK_RING_MAX: usize = 16;
/// Typed chars join one undo step up to this many, as Emacs amalgamates.
const AMALGAMATE: u32 = 20;
/// `C-x e` with an argument of 0 plays the macro until it fails, but no
/// more often than this.
const MACRO_LIMIT: u32 = 100_000;

/// The prefix argument, as `C-u`, `M-5`, and `M--` give it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Arg {
    #[default]
    None,
    /// `C-u` pressed this many times: 4 to that power.
    Universal(u32),
    /// `-` alone.
    Minus,
    Number(i64),
}

impl Arg {
    /// The number it stands for; 1 without one.
    pub fn count(self) -> i64 {
        match self {
            Arg::None => 1,
            Arg::Universal(k) => 4i64.pow(k.min(15)),
            Arg::Minus => -1,
            Arg::Number(n) => n,
        }
    }

    pub fn given(self) -> bool {
        self != Arg::None
    }

    /// `C-u` alone, which some commands read as a flag.
    pub fn is_plain_universal(self) -> bool {
        self == Arg::Universal(1)
    }

    fn label(self) -> String {
        match self {
            Arg::None => String::new(),
            Arg::Universal(k) => vec!["C-u"; k as usize].join(" "),
            Arg::Minus => "C-u -".into(),
            Arg::Number(n) => format!("C-u {n}"),
        }
    }
}

/// What the command before did, where the next one cares.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Last {
    #[default]
    Other,
    SelfInsert,
    DeleteChar,
    DeleteBackward,
    Kill,
    Yank,
    SetMark,
    Vertical,
    Recenter,
    WindowLine,
    CycleSpacing,
    MarkWord,
    MarkSexp,
    MarkParagraph,
    Dabbrev,
}

/// A kill: its text, and for a rectangle, its lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Kill {
    pub text: String,
    pub rect: Option<Vec<String>>,
}

/// A command that reads the next key itself.
#[derive(Clone, Debug)]
pub enum Waiting {
    /// `C-q`.
    Quoted(Arg),
    /// `M-z`, and whether up to the char only.
    Zap { up_to: bool, arg: Arg },
    /// A register's name, for this command.
    Register { command: &'static str, arg: Arg },
    /// `e` after `C-x e`, `z` after `C-x z`: that key again.
    Again(char),
    /// `C-x TAB` without an argument: arrows move the region's lines.
    IndentRigidly,
    /// `C-h k`: the key to describe, and the keys so far.
    Describe(Vec<KeyEvent>),
    /// `y` or `n`, for this question.
    YesOrNo(minibuffer::Confirm),
    /// `C-x s` and `C-x C-c`: whether to save each of these files, the
    /// first one asked now, and whether to quit after.
    SaveSome { paths: Vec<String>, quit: bool },
}

/// `M-SPC` pressed again deletes the space it left; the text it replaced.
#[derive(Clone)]
pub struct Spacing {
    start: u64,
    original: String,
    /// Where the point was in it.
    offset: u64,
    stage: u8,
}

/// `M-/` pressed again tries the next expansion.
pub struct Dabbrev {
    start: u64,
    /// What was typed before the first `M-/`.
    prefix: String,
    /// The expansions left, nearest first.
    left: Vec<String>,
    /// The one in the buffer now, after the prefix.
    inserted: String,
}

pub struct Emacs {
    /// The built-in keys with `[settings.keys.global]` over them.
    keymap: Keymap,
    /// `[settings.keys.global]` alone, for its `C-c` table.
    user: Keymap,
    sequence: Sequence,
    /// Escape was pressed: the next key counts as with Meta.
    meta: bool,
    arg: Arg,
    /// Digits and `-` still go into the argument.
    arg_digits: bool,
    /// The region is active.
    active: bool,
    /// A Shift motion activated it, and an unshifted one ends it.
    shift_selected: bool,
    rectangle: bool,
    last: Last,
    this: Last,
    /// The display column `C-n` and `C-p` aim for.
    column: Option<u32>,
    /// The current command has made its first edit: later ones join it in
    /// one undo step.
    step_open: bool,
    /// The next edit joins the step before, as typed chars do.
    merge: bool,
    /// Commands joined into the current undo step.
    amalgamated: u32,
    /// The command changed the buffer: the region ends.
    deactivate: bool,
    /// The command failed, so a macro stops.
    failed: bool,
    kills: Vec<Kill>,
    yank_at: usize,
    /// The text last put on the clipboard, to tell another program's.
    clipboard: Option<String>,
    killed_rectangle: Vec<String>,
    registers: BTreeMap<char, Register>,
    spacing: Option<Spacing>,
    dabbrev: Option<Dabbrev>,
    waiting: Option<Waiting>,
    asking: Option<Asking>,
    isearch: Option<Isearch>,
    query: Option<Query>,
    /// The last search string, and whether it was a pattern.
    last_search: Option<(String, bool)>,
    /// The last replacement: what, with what.
    last_replace: Option<(String, String)>,
    history: BTreeMap<&'static str, Vec<String>>,
    /// Keys being recorded as a macro.
    recording: Option<Vec<KeyEvent>>,
    /// Where the current command's keys start in the recording.
    command_start: usize,
    last_macro: Option<Vec<KeyEvent>>,
    replaying: u32,
    /// The last command and its argument and key, for `C-x z`.
    last_command: Option<(String, Arg, KeyEvent)>,
    /// Paths and positions `M-.` left, for `M-,`.
    xref: Vec<(Option<String>, u64)>,
    hints: Option<Popup>,
}

thread_local! {
    static EMACS: RefCell<Option<Emacs>> = const { RefCell::new(None) };
}

fn with_emacs<R>(f: impl FnOnce(&mut Emacs) -> R) -> R {
    EMACS.with_borrow_mut(|emacs| f(emacs.get_or_insert_with(Emacs::new)))
}

struct Plugin;

impl Guest for Plugin {
    fn init(config: String) -> Result<(), String> {
        let settings: serde_json::Value =
            serde_json::from_str(&config).map_err(|err| err.to_string())?;
        let (user, errors) = keys::keymap_of(&settings, "global", &|name| {
            bind::is_command(name).then(|| Binding::Command(name.to_string()))
        });
        if let Some(first) = errors.first() {
            let more = match errors.len() {
                1 => String::new(),
                n => format!(" (and {} more)", n - 1),
            };
            ui::show_message(&format!("emacs.toml: {first}{more}; left out"));
        }
        input::push_layer();
        // No modes: typed chars always go into the text.
        input::set_mode("global", true);
        let view = view::active();
        view.set_cursor_shape(CursorShape::Block);
        let selection = view.selection();
        let r = selection.ranges[selection.primary as usize];
        set_ranges(&view, vec![point(r.head)], 0);
        with_emacs(|emacs| {
            let mut keymap = bind::keymap();
            keys::merge(&mut keymap, user.clone());
            emacs.keymap = keymap;
            emacs.user = user;
        });
        Ok(())
    }

    fn handle_key(ev: KeyEvent) -> KeyResult {
        with_emacs(|emacs| emacs.handle_key(ev))
    }

    fn handle_paste(text: String) -> KeyResult {
        with_emacs(|emacs| emacs.paste(&text))
    }

    fn run_command(name: String, _args: String) -> Result<String, String> {
        Err(format!("no command {name}"))
    }

    fn on_event(_ev: Event) {}
}

nib_plugin::export!(Plugin);

pub fn ctrl(ev: &KeyEvent) -> Option<char> {
    match ev.code {
        KeyCode::Char(c) if ev.modifiers == Modifiers::CTRL => Some(c),
        _ => None,
    }
}

pub fn meta(ev: &KeyEvent) -> Option<char> {
    match ev.code {
        KeyCode::Char(c) if ev.modifiers == Modifiers::ALT => Some(c),
        _ => None,
    }
}

pub fn plain(ev: &KeyEvent) -> Option<char> {
    match ev.code {
        KeyCode::Char(c) if !ev.modifiers.intersects(Modifiers::CTRL | Modifiers::ALT) => Some(c),
        _ => None,
    }
}

pub fn is_key(ev: &KeyEvent, code: KeyCode) -> bool {
    ev.code == code && ev.modifiers.is_empty()
}

/// `C-g`: stop what is going on.
pub fn is_quit(ev: &KeyEvent) -> bool {
    ctrl(ev) == Some('g')
}

/// A key as Emacs writes it, for messages.
pub fn key_label(key: &KeyEvent) -> String {
    let mut label = String::new();
    for (flag, prefix) in [
        (Modifiers::CTRL, "C-"),
        (Modifiers::ALT, "M-"),
        (Modifiers::SHIFT, "S-"),
    ] {
        if key.modifiers.contains(flag) {
            label.push_str(prefix);
        }
    }
    match key.code {
        KeyCode::Char(' ') => label.push_str("SPC"),
        KeyCode::Char(c) => label.push(c),
        KeyCode::Enter => label.push_str("RET"),
        KeyCode::Escape => label.push_str("ESC"),
        KeyCode::Tab => label.push_str("TAB"),
        KeyCode::Backspace => label.push_str("DEL"),
        KeyCode::Delete => label.push_str("<deletechar>"),
        KeyCode::F(n) => label.push_str(&format!("<f{n}>")),
        other => label.push_str(&format!(
            "<{}>",
            keys::label(&KeyEvent {
                code: other,
                modifiers: Modifiers::empty(),
            })
        )),
    }
    label
}

/// Shows `help` in `*Help*`, below, with the focus in it so `q` closes it
/// (as with `help-window-select` on).
fn show_help(help: &str) {
    base_kit::show_listing("*Help*", help, &[("q", "emacs.quit-window")]);
}

fn keys_label(keys: &[KeyEvent]) -> String {
    keys.iter().map(key_label).collect::<Vec<_>>().join(" ")
}

/// Selects `anchor..head`, as a search shows its match.
pub fn edit_range(view: &View, anchor: u64, head: u64) {
    set_ranges(view, vec![range(anchor, head)], 0);
}

pub fn primary(view: &View) -> SelRange {
    let selection = view.selection();
    selection.ranges[selection.primary as usize]
}

impl Emacs {
    fn new() -> Self {
        Self {
            keymap: bind::keymap(),
            user: Keymap::new(),
            sequence: Sequence::default(),
            meta: false,
            arg: Arg::None,
            arg_digits: false,
            active: false,
            shift_selected: false,
            rectangle: false,
            last: Last::Other,
            this: Last::Other,
            column: None,
            step_open: false,
            merge: false,
            amalgamated: 0,
            deactivate: false,
            failed: false,
            kills: Vec::new(),
            yank_at: 0,
            clipboard: None,
            killed_rectangle: Vec::new(),
            registers: BTreeMap::new(),
            spacing: None,
            dabbrev: None,
            waiting: None,
            asking: None,
            isearch: None,
            query: None,
            last_search: None,
            last_replace: None,
            history: BTreeMap::new(),
            recording: None,
            command_start: 0,
            last_macro: None,
            replaying: 0,
            last_command: None,
            xref: Vec::new(),
            hints: None,
        }
    }

    fn handle_key(&mut self, ev: KeyEvent) -> KeyResult {
        if self.replaying == 0
            && let Some(keys) = &mut self.recording
        {
            keys.push(ev);
        }
        let ev = if std::mem::take(&mut self.meta) {
            KeyEvent {
                code: ev.code,
                modifiers: ev.modifiers | Modifiers::ALT,
            }
        } else if is_key(&ev, KeyCode::Escape) && !self.prefix_binds(ev) {
            self.meta = true;
            self.show_status();
            return KeyResult::Handled;
        } else {
            ev
        };
        let result = self.route(ev);
        self.show_status();
        result
    }

    /// Pasted text, as `xterm-paste` puts it: at the point, with the mark at
    /// its start; into the search string while searching, and into the
    /// minibuffer while it asks.
    fn paste(&mut self, text: &str) -> KeyResult {
        let view = view::active();
        if self.isearch.is_some() {
            self.add_text(&view, text);
            return KeyResult::Handled;
        }
        if self.query.is_some() {
            return KeyResult::Handled;
        }
        if let Some(active) = prompts::active() {
            if !active.mine || self.asking.is_none() {
                return KeyResult::Pass;
            }
            self.paste_into_minibuffer(text);
            return KeyResult::Handled;
        }
        let key = KeyEvent {
            code: KeyCode::Char('v'),
            modifiers: Modifiers::CTRL | Modifiers::SHIFT,
        };
        self.whole("xterm-paste", Arg::None, key, |emacs, view| {
            let here = emacs.point(view);
            let end = here + text.len() as u64;
            emacs.replace(view, here, here, text, end);
            emacs.push_mark(view, here);
            Ok(())
        });
        self.show_status();
        KeyResult::Handled
    }

    /// Whether the prefix being typed has `ev` in its table, as `ESC ESC`
    /// has the third `ESC`.
    fn prefix_binds(&self, ev: KeyEvent) -> bool {
        self.sequence
            .waiting()
            .is_some_and(|(table, _)| keys::lookup(table, &ev).is_some())
    }

    fn route(&mut self, ev: KeyEvent) -> KeyResult {
        if self.isearch.is_some() {
            return self.isearch_key(ev);
        }
        if self.query.is_some() {
            return self.query_key(ev);
        }
        if let Some(active) = prompts::active() {
            return if active.mine && self.asking.is_some() {
                self.asking_key(ev);
                KeyResult::Handled
            } else {
                self.other_prompt_key(ev)
            };
        }
        if let Some(offer) = prompts::offered()
            && let Some(action) = choice_action(ev).filter(|a| offer.actions.contains(a))
        {
            prompts::act(action);
            return KeyResult::Handled;
        }
        if let Some(waiting) = self.waiting.take() {
            self.waiting_key(waiting, ev);
            return KeyResult::Handled;
        }
        self.command_key(ev);
        KeyResult::Handled
    }

    /// A key read as part of a command: an argument, a prefix, a
    /// command's key, or a char to insert.
    fn command_key(&mut self, ev: KeyEvent) {
        let waiting = self.sequence.is_waiting();
        if !waiting {
            if self.arg == Arg::None
                && let Some(keys) = &self.recording
            {
                self.command_start = keys.len().saturating_sub(1);
            }
            if self.arg_digits && self.argument_key(ev) {
                return;
            }
            if ctrl(&ev) == Some('c') && !self.user_binds_c_c() {
                self.enter_leader(ev);
                return;
            }
        }
        let buffer_keys = keys::buffer_keymap(&emacs_keeps);
        let step = if buffer_keys.is_empty() {
            self.sequence.key(&self.keymap, ev)
        } else {
            // The shown buffer's keys, as a major mode's, over the global
            // ones; the settings' over both.
            let mut table = self.keymap.clone();
            keys::merge(&mut table, buffer_keys);
            keys::merge(&mut table, self.user.clone());
            self.sequence.key(&table, ev)
        };
        match step {
            Step::Wait => {}
            Step::Dropped(typed) => {
                self.arg = Arg::None;
                self.arg_digits = false;
                if is_quit(&ev) {
                    self.execute("keyboard-quit", ev);
                } else {
                    self.failed = true;
                    ui::show_message(&format!("{} is undefined", keys_label(&typed)));
                }
            }
            Step::Run(Binding::Command(name), _)
                if base_kit::own_command(&name, "emacs").is_some_and(bind::is_command) =>
            {
                self.execute(&name["emacs.".len()..], ev)
            }
            Step::Run(Binding::Command(name), _) if name.contains('.') => {
                self.arg = Arg::None;
                self.arg_digits = false;
                call_or_show(&name);
                self.render();
            }
            Step::Run(Binding::Command(name), _) => self.execute(&name, ev),
            Step::Run(Binding::Keys(keys), _) => {
                for key in keys {
                    self.command_key(key);
                }
            }
            Step::Run(Binding::Prefix(_), _) => {}
            Step::NotMine => self.unbound(ev),
        }
        self.show_hints();
    }

    fn user_binds_c_c(&self) -> bool {
        let c_c = keys::parse_key("C-c").expect("C-c parses");
        matches!(keys::lookup(&self.user, &c_c), Some(Binding::Command(_)))
    }

    /// `C-c`: plugins' keys, with the settings' `C-c` table over them.
    fn enter_leader(&mut self, ev: KeyEvent) {
        let mut table = leader::keymap(&[], input::leader_keys()).keymap;
        // The shown buffer's keys under C-c, as C-c C-c in magit.
        if let Some(Binding::Prefix(buffer)) =
            keys::lookup(&keys::buffer_keymap(&emacs_keeps), &ev).cloned()
        {
            keys::merge(&mut table, buffer);
        }
        if let Some(Binding::Prefix(user)) = keys::lookup(&self.user, &ev) {
            keys::merge(&mut table, user.clone());
        }
        self.sequence.enter(table, vec![ev]);
        self.show_hints();
    }

    /// A key no table has: a char to insert, or a Shift motion.
    fn unbound(&mut self, ev: KeyEvent) {
        if plain(&ev).is_some() {
            return self.execute("self-insert-command", ev);
        }
        let unshifted = match ev.code {
            KeyCode::Char(c) if ev.modifiers.contains(Modifiers::CTRL) && c.is_uppercase() => {
                Some(KeyEvent {
                    code: KeyCode::Char(c.to_ascii_lowercase()),
                    modifiers: ev.modifiers,
                })
            }
            KeyCode::Char(_) => None,
            _ if ev.modifiers.contains(Modifiers::SHIFT) => Some(KeyEvent {
                code: ev.code,
                modifiers: ev.modifiers - Modifiers::SHIFT,
            }),
            _ => None,
        };
        if let Some(key) = unshifted
            && let Some(Binding::Command(name)) = keys::lookup(&self.keymap, &key)
            && bind::MOTIONS.contains(&name.as_str())
        {
            let name = name.clone();
            // A region set otherwise gives way to a new one from here.
            let view = view::active();
            if !self.active || !self.shift_selected {
                let here = primary(&view).head;
                self.push_mark(&view, here);
                self.active = true;
                self.rectangle = false;
                self.shift_selected = true;
            }
            return self.run_top(&name, key);
        }
        self.arg = Arg::None;
        self.arg_digits = false;
        self.failed = true;
        ui::show_message(&format!("{} is undefined", key_label(&ev)));
    }

    /// A digit or `-` typed into the argument.
    fn argument_key(&mut self, ev: KeyEvent) -> bool {
        let digit = match ev.code {
            KeyCode::Char(c) if ev.modifiers.is_empty() => c,
            _ => return false,
        };
        self.arg = match (self.arg, digit) {
            (Arg::Universal(_), '-') => Arg::Minus,
            (Arg::Universal(_) | Arg::None, d) if d.is_ascii_digit() => {
                Arg::Number(d.to_digit(10).expect("a digit") as i64)
            }
            (Arg::Minus, d) if d.is_ascii_digit() => {
                Arg::Number(-(d.to_digit(10).expect("a digit") as i64))
            }
            (Arg::Number(n), d) if d.is_ascii_digit() => {
                let d = d.to_digit(10).expect("a digit") as i64;
                let n = n.saturating_mul(10);
                Arg::Number(if n < 0 { n - d } else { n + d })
            }
            _ => return false,
        };
        true
    }

    /// Runs a command the keys named: prefix commands build the argument;
    /// the others take it.
    fn execute(&mut self, name: &str, key: KeyEvent) {
        match name {
            "universal-argument" => {
                self.arg = match self.arg {
                    Arg::Universal(k) if self.arg_digits => Arg::Universal(k + 1),
                    Arg::None => Arg::Universal(1),
                    // After digits, C-u ends the argument.
                    other => {
                        self.arg_digits = false;
                        self.arg = other;
                        return;
                    }
                };
                self.arg_digits = true;
            }
            "digit-argument" => {
                let d = match key.code {
                    KeyCode::Char(c) => c.to_digit(10).unwrap_or(0) as i64,
                    _ => 0,
                };
                self.arg = match self.arg {
                    Arg::Number(n) if self.arg_digits => {
                        let n = n.saturating_mul(10);
                        Arg::Number(if n < 0 { n - d } else { n + d })
                    }
                    Arg::Minus => Arg::Number(-d),
                    _ => Arg::Number(d),
                };
                self.arg_digits = true;
            }
            "negative-argument" => {
                self.arg = match self.arg {
                    Arg::Number(n) => Arg::Number(-n),
                    Arg::Minus => Arg::None,
                    _ => Arg::Minus,
                };
                self.arg_digits = true;
            }
            _ => {
                if self.shift_selected && bind::MOTIONS.contains(&name) && self.active {
                    self.active = false;
                    self.shift_selected = false;
                } else if !bind::MOTIONS.contains(&name) {
                    self.shift_selected = false;
                }
                self.run_top(name, key);
            }
        }
    }

    /// Runs a command at the top: it takes the argument, and what it did
    /// becomes the last command.
    fn run_top(&mut self, name: &str, key: KeyEvent) {
        let arg = std::mem::take(&mut self.arg);
        self.arg_digits = false;
        self.run_with(name, arg, key);
    }

    /// Runs command `name` with `arg`, as a whole command.
    pub fn run_with(&mut self, name: &str, arg: Arg, key: KeyEvent) {
        self.whole(name, arg, key, |emacs, view| {
            emacs.run(view, name, arg, key)
        });
    }

    /// Does `f` as the whole of command `name`: undo steps, the region,
    /// and what the next command sees as the last one.
    pub fn whole(
        &mut self,
        name: &str,
        arg: Arg,
        key: KeyEvent,
        f: impl FnOnce(&mut Self, &View) -> Result<(), String>,
    ) {
        self.this = Last::Other;
        self.deactivate = false;
        self.step_open = false;
        self.merge = false;
        let view = view::active();
        if let Err(message) = f(self, &view) {
            self.failed = true;
            ui::show_message(&message);
        }
        if self.deactivate {
            self.active = false;
            self.rectangle = false;
            self.shift_selected = false;
        }
        if self.this != Last::Vertical {
            self.column = None;
        }
        if self.this != Last::CycleSpacing {
            self.spacing = None;
        }
        if self.this != Last::Dabbrev {
            self.dabbrev = None;
        }
        if self.this != self.last {
            self.amalgamated = 0;
        }
        self.last = self.this;
        if !matches!(
            name,
            "repeat" | "kmacro-end-and-call-macro" | "kmacro-end-or-call-macro"
        ) {
            self.last_command = Some((name.to_string(), arg, key));
        }
        if self.isearch.is_none() && self.query.is_none() {
            self.render();
        }
    }

    /// Draws the region, or the point alone.
    pub fn render(&mut self) {
        let view = view::active();
        let here = primary(&view).head;
        let mark = self.mark(&view.buffer());
        match mark {
            Some(mark) if self.active && self.rectangle => {
                let (ranges, primary) = rect::ranges(&view, mark, here);
                set_ranges(&view, ranges, primary);
            }
            Some(mark) if self.active => set_ranges(&view, vec![range(mark, here)], 0),
            _ => {
                self.active = false;
                set_ranges(&view, vec![point(here)], 0)
            }
        }
    }

    pub fn point(&self, view: &View) -> u64 {
        primary(view).head
    }

    pub fn goto(&mut self, view: &View, pos: u64) {
        set_ranges(view, vec![point(pos)], 0);
    }

    /// Applies `edits` and leaves the point at `after`: in the current undo
    /// step if the command already edited or joins the one before.
    pub fn edit(&mut self, view: &View, edits: Vec<Edit>, after: u64) -> bool {
        if edits.is_empty() {
            self.goto(view, after);
            return false;
        }
        let undo = if self.step_open || self.merge {
            UndoMode::Merge
        } else {
            UndoMode::NewStep
        };
        let selection = Selection {
            ranges: vec![point(after)],
            primary: 0,
        };
        let version = view.buffer().version();
        let done = view.apply(version, &edits, Some(&selection), undo).is_ok();
        if done {
            self.step_open = true;
            self.deactivate = true;
        }
        done
    }

    /// Applies edits anywhere in the buffer, keeping the point on its char:
    /// before text put in where it is, and at the start of text deleted
    /// around it.
    pub fn edit_around(&mut self, view: &View, edits: Vec<Edit>) -> bool {
        let here = self.point(view);
        let mut after = here as i64;
        for e in &edits {
            if e.start < here {
                if e.end <= here {
                    after += e.text.len() as i64 - (e.end - e.start) as i64;
                } else {
                    after -= (here - e.start) as i64;
                }
            }
        }
        self.edit(view, edits, after.max(0) as u64)
    }

    /// Replaces `from..to` with `text`, leaving the point at `after`.
    pub fn replace(&mut self, view: &View, from: u64, to: u64, text: &str, after: u64) -> bool {
        self.edit(
            view,
            vec![Edit {
                start: from,
                end: to,
                text: text.to_string(),
            }],
            after,
        )
    }

    pub fn mark(&self, buffer: &Buffer) -> Option<u64> {
        buffer.marks("mark").first().copied()
    }

    /// Sets the mark to `pos`, keeping the one before in the mark ring.
    pub fn push_mark(&mut self, view: &View, pos: u64) {
        let buffer = view.buffer();
        if let Some(old) = self.mark(&buffer) {
            let mut ring = buffer.marks("mark-ring");
            ring.insert(0, old);
            ring.truncate(MARK_RING_MAX);
            buffer.set_marks("mark-ring", &ring);
        }
        buffer.set_marks("mark", &[pos]);
    }

    /// Sets the mark without touching the ring.
    pub fn set_mark(&mut self, view: &View, pos: u64) {
        view.buffer().set_marks("mark", &[pos]);
    }

    /// The region, from its start to its end, or why there is none.
    pub fn region(&self, view: &View) -> Result<(u64, u64), String> {
        let mark = self
            .mark(&view.buffer())
            .ok_or("The mark is not set now, so there is no region")?;
        let here = self.point(view);
        Ok((mark.min(here), mark.max(here)))
    }

    /// Puts `text` in the kill ring: added to the kill before it if the
    /// command before killed too, in front when killing backward.
    pub fn kill(&mut self, text: String, backward: bool) {
        self.this = Last::Kill;
        if self.last == Last::Kill
            && let Some(first) = self.kills.first_mut()
            && first.rect.is_none()
        {
            if backward {
                first.text.insert_str(0, &text);
            } else {
                first.text.push_str(&text);
            }
        } else if text.is_empty() {
            return;
        } else {
            self.kills.insert(0, Kill { text, rect: None });
            self.kills.truncate(KILL_RING_MAX);
        }
        self.yank_at = 0;
        self.sync_clipboard();
    }

    /// A new kill, as `M-w` and rectangles make.
    pub fn kill_new(&mut self, kill: Kill) {
        self.kills.insert(0, kill);
        self.kills.truncate(KILL_RING_MAX);
        self.yank_at = 0;
        self.sync_clipboard();
    }

    pub fn sync_clipboard(&mut self) {
        let text = self.kills[0].text.clone();
        if clipboard::set(&text).is_ok() {
            self.clipboard = Some(text);
        }
    }

    /// Before a yank: text another program put on the clipboard becomes
    /// the newest kill.
    pub fn from_clipboard(&mut self) {
        let Ok(text) = clipboard::get() else {
            return;
        };
        let newest = self.kills.first().map(|kill| &kill.text);
        if text.is_empty() || self.clipboard.as_ref() == Some(&text) || newest == Some(&text) {
            return;
        }
        self.clipboard = Some(text.clone());
        self.kills.insert(0, Kill { text, rect: None });
        self.kills.truncate(KILL_RING_MAX);
        self.yank_at = 0;
    }

    /// Plays `keys` as if typed, stopping when a command fails.
    pub fn play(&mut self, keys: &[KeyEvent]) -> bool {
        self.replaying += 1;
        self.failed = false;
        for &key in keys {
            self.handle_key(key);
            if self.failed {
                break;
            }
        }
        self.replaying -= 1;
        !self.failed
    }

    /// Plays the last macro `count` times, or until it fails for 0.
    pub fn call_macro(&mut self, count: i64) -> Result<(), String> {
        let keys = self
            .last_macro
            .clone()
            .ok_or("No kbd macro has been defined")?;
        let times = if count <= 0 {
            MACRO_LIMIT
        } else {
            count as u32
        };
        // The macro's commands are its own; this one should not look like
        // the last command to them or after them.
        let (this, last) = (self.this, self.last);
        for _ in 0..times {
            if !self.play(&keys) {
                break;
            }
        }
        self.this = this;
        self.last = last;
        // Played until it failed, as asked.
        if count <= 0 {
            self.failed = false;
        }
        Ok(())
    }

    /// A key a command waited for.
    fn waiting_key(&mut self, waiting: Waiting, ev: KeyEvent) {
        if is_quit(&ev) && !matches!(waiting, Waiting::Quoted(_) | Waiting::Describe(_)) {
            self.waiting = None;
            return self.execute("keyboard-quit", ev);
        }
        match waiting {
            Waiting::Quoted(arg) => self.whole("quoted-insert", arg, ev, |emacs, view| {
                emacs.insert_quoted(view, arg, ev)
            }),
            Waiting::Zap { up_to, arg } => {
                let name = if up_to {
                    "zap-up-to-char"
                } else {
                    "zap-to-char"
                };
                self.whole(name, arg, ev, |emacs, view| emacs.zap(view, up_to, arg, ev));
            }
            Waiting::Register { command, arg } => self.whole(command, arg, ev, |emacs, view| {
                emacs.register_command(view, command, arg, ev)
            }),
            Waiting::Again(c) if plain(&ev) == Some(c) => {
                let name = if c == 'e' {
                    "kmacro-end-and-call-macro"
                } else {
                    "repeat"
                };
                self.run_with(name, Arg::None, ev);
            }
            Waiting::Again(_) => self.command_key(ev),
            Waiting::IndentRigidly => {
                if !self.indent_rigidly_key(ev) {
                    self.command_key(ev);
                }
            }
            Waiting::Describe(mut typed) => {
                typed.push(ev);
                self.describe_key(typed);
            }
            Waiting::SaveSome { paths, quit } => self.save_some_key(paths, quit, ev),
            Waiting::YesOrNo(confirm) => match plain(&ev) {
                Some('y' | 'Y') => self.confirmed(confirm, true),
                Some('n' | 'N') => self.confirmed(confirm, false),
                _ => {
                    ui::show_message(&format!("{} (y or n) ", confirm.question()));
                    self.waiting = Some(Waiting::YesOrNo(confirm));
                }
            },
        }
    }

    /// `C-h k`: follows the keys through the tables and says what they run.
    fn describe_key(&mut self, typed: Vec<KeyEvent>) {
        let mut table = &self.keymap;
        for (i, key) in typed.iter().enumerate() {
            match keys::lookup(table, key) {
                Some(Binding::Prefix(next)) => {
                    table = next;
                    if i + 1 == typed.len() {
                        self.waiting = Some(Waiting::Describe(typed));
                        return;
                    }
                }
                Some(Binding::Command(name)) => {
                    let mut help =
                        format!("{} runs the command {name}.\n", keys_label(&typed[..=i]));
                    let bound = bind::keys_of(name);
                    if !bound.is_empty() {
                        help.push_str(&format!("\nIt is bound to {}.\n", bound.join(", ")));
                    }
                    if let Some(what) = bind::describe(name) {
                        help.push_str(&format!("\n{what}.\n"));
                    }
                    return show_help(&help);
                }
                _ => break,
            }
        }
        let label = keys_label(&typed);
        if typed.len() == 1 && plain(&typed[0]).is_some() {
            show_help(&format!(
                "{label} runs the command self-insert-command.\n\nInsert the character typed.\n"
            ));
        } else {
            ui::show_message(&format!("{label} is undefined"));
        }
    }

    /// Keys for a prompt another plugin opened, such as the picker's: the
    /// minibuffer's editing keys, and the core's for the rest.
    fn other_prompt_key(&mut self, ev: KeyEvent) -> KeyResult {
        let Some(active) = prompts::active() else {
            return KeyResult::Pass;
        };
        if is_quit(&ev) || ev == minibuffer::escape_quit() {
            prompts::act(Action::Cancel);
            return KeyResult::Handled;
        }
        let action = match (ctrl(&ev), ev.code) {
            (Some('n'), _) => Some(Action::Next),
            (Some('p'), _) => Some(Action::Previous),
            (_, KeyCode::Tab) if ev.modifiers.is_empty() => Some(Action::Complete),
            _ => None,
        };
        if let Some(action) = action {
            prompts::act(action);
            return KeyResult::Handled;
        }
        let edited = minibuffer::edit_key(
            &active.text,
            active.cursor as usize,
            ev,
            self.kills.first().map(|kill| kill.text.as_str()),
        );
        match edited {
            Some((text, cursor)) => {
                prompts::edit(&text, cursor as u32);
                KeyResult::Handled
            }
            None => KeyResult::Pass,
        }
    }

    fn show_hints(&mut self) {
        self.hints = self.sequence.waiting().map(|(table, typed)| {
            Popup::new(PopupAnchor::Corner, &hints::keymap_lines(typed, table))
        });
    }

    fn show_status(&self) {
        let mut line = Vec::new();
        if self.recording.is_some() {
            line.push(span("Def ", "ui.text"));
        }
        let mut pending = self.arg.label();
        if let Some((_, typed)) = self.sequence.waiting() {
            if !pending.is_empty() {
                pending.push(' ');
            }
            pending.push_str(&keys_label(typed));
        }
        if self.meta {
            if !pending.is_empty() {
                pending.push(' ');
            }
            pending.push_str("ESC");
        }
        if !pending.is_empty() {
            line.push(span(&format!("{pending}-"), "ui.text"));
        }
        if line.is_empty() {
            ui::remove_status("pending");
        } else {
            ui::set_status("pending", Side::Right, -10, &line);
        }
    }
}

/// The keys a plugin's buffer cannot take from Emacs: quitting, `C-x`, and
/// `M-x`.
fn emacs_keeps(key: &KeyEvent) -> bool {
    matches!(ctrl(key), Some('g' | 'x')) || meta(key) == Some('x')
}

/// What a key does to a list shown without a line to type into, such as
/// completions.
fn choice_action(ev: KeyEvent) -> Option<Action> {
    let plain = ev.modifiers.is_empty();
    match (ctrl(&ev), ev.code) {
        (Some('n'), _) => Some(Action::Next),
        (Some('p'), _) => Some(Action::Previous),
        (Some('g'), _) => Some(Action::Cancel),
        (_, KeyCode::Down) if plain => Some(Action::Next),
        (_, KeyCode::Up) if plain => Some(Action::Previous),
        (_, KeyCode::Tab | KeyCode::Enter) if plain => Some(Action::Accept),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_count_as_emacs_counts_them() {
        assert_eq!(Arg::None.count(), 1);
        assert_eq!(Arg::Universal(2).count(), 16);
        assert_eq!(Arg::Minus.count(), -1);
        assert_eq!(Arg::Universal(1).label(), "C-u");
        assert_eq!(Arg::Number(-3).label(), "C-u -3");
    }

    #[test]
    fn keys_are_labeled_as_emacs_writes_them() {
        let keys = keys::parse_keys("C-x C-s A-x space ret").unwrap();
        assert_eq!(keys_label(&keys), "C-x C-s M-x SPC RET");
    }
}
