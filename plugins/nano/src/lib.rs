//! GNU nano's way of editing (docs/nano.md): no modes, typed chars go in,
//! and Ctrl and Alt keys do the rest. The cursor is a point; with the mark
//! set, the selection runs from the mark, its anchor, to the cursor.

use std::cell::RefCell;

use base_kit::doc::{self, Doc};
use base_kit::edit::{indent_unit, insertion, point, range, set_ranges};
use base_kit::keys::{self, Binding, Keymap, Sequence, Step};
use base_kit::{call_or_show, error_message, hints, leader, line_edit, regex_escape};
use base_kit::{span, tree};
use nib_plugin::exports::nib::plugin::guest::{Guest, KeyResult};
use nib_plugin::nib::plugin::events::Event;
use nib_plugin::nib::plugin::prompt::{self as prompts, Action, Line};
use nib_plugin::nib::plugin::types::{CursorShape, Edit, KeyCode, KeyEvent, Modifiers, UndoMode};
use nib_plugin::nib::plugin::ui::{self, Panel, Popup, PopupAnchor};
use nib_plugin::nib::plugin::view::{self, ScrollAmount, View};
use nib_plugin::nib::plugin::{editor, input};

/// nano's function names, as nanorc's `bind` writes them, and the keys that
/// do them here, for `[settings.keys.global]`.
const FUNCTIONS: &[(&str, &str)] = &[
    ("exit", "C-x"),
    ("writeout", "C-o"),
    ("insert", "C-r"),
    ("whereis", "C-w"),
    ("replace", "C-\\"),
    ("cut", "C-k"),
    ("paste", "C-u"),
    ("execute", "C-t"),
    ("location", "C-c"),
    ("gotoline", "C-_"),
    ("undo", "A-u"),
    ("redo", "A-e"),
    ("mark", "A-a"),
    ("copy", "A-6"),
    ("findnext", "A-w"),
    ("findprevious", "A-q"),
    ("findbracket", "A-]"),
    ("firstline", "A-\\"),
    ("lastline", "A-/"),
    ("nohelp", "A-x"),
    ("up", "up"),
    ("down", "down"),
    ("left", "left"),
    ("right", "right"),
    ("home", "home"),
    ("end", "end"),
    ("pageup", "pageup"),
    ("pagedown", "pagedown"),
    ("nextword", "C-space"),
    ("prevword", "A-space"),
    ("backspace", "backspace"),
    ("delete", "del"),
    ("enter", "ret"),
    ("tab", "tab"),
];

/// The Alt keys nano uses itself; the others hold plugins' keys.
const OWN_ALT: &[char] = &['a', 'e', 'q', 'u', 'w', 'x', '6', ']', '\\', '/', ' '];

/// Columns each item of the two lines takes, key and all.
const HELP_ITEM: usize = 15;

/// The two lines of keys nano shows at the bottom.
const HELP: [&[(&str, &str)]; 2] = [
    &[
        ("^G", "Menu"),
        ("^O", "Write Out"),
        ("^W", "Where Is"),
        ("^K", "Cut"),
        ("^T", "Commands"),
        ("^C", "Location"),
        ("M-U", "Undo"),
        ("M-A", "Set Mark"),
    ],
    &[
        ("^X", "Exit"),
        ("^R", "Read File"),
        ("^\\", "Replace"),
        ("^U", "Paste"),
        ("M-]", "Bracket"),
        ("^/", "Go To Line"),
        ("M-E", "Redo"),
        ("M-6", "Copy"),
    ],
];

/// What a prompt of nano's asks.
#[derive(Clone, PartialEq, Eq)]
enum Ask {
    /// `exit` after writing, when ^X asked for a name.
    Write {
        exit: bool,
    },
    Insert,
    Search,
    ReplaceWhat,
    ReplaceWith(String),
    GotoLine,
    /// ^X with unsaved changes: Y, N, or ^C.
    SaveFirst,
}

struct Asking {
    ask: Ask,
    line: Line,
}

#[derive(Default)]
struct Nano {
    /// The mark is set: the selection's anchor stays where it was set.
    marking: bool,
    /// The cutbuffer.
    cut: String,
    /// The key before cut a line, so another cut adds to it.
    cutting: bool,
    /// The key before typed a char, so another joins its undo step.
    typing: bool,
    /// The display column Up and Down aim for.
    column: Option<u32>,
    /// Escape was pressed: the next key counts as with Alt.
    meta: bool,
    search: Option<String>,
    asking: Option<Asking>,
    help: Option<Panel>,
    /// From `[settings.keys.global]`.
    keymap: Keymap,
    /// Keys of the settings or under the leader, typed so far.
    sequence: Sequence,
    hints: Option<Popup>,
}

thread_local! {
    static NANO: RefCell<Option<Nano>> = const { RefCell::new(None) };
}

fn with_nano<R>(f: impl FnOnce(&mut Nano) -> R) -> R {
    NANO.with_borrow_mut(|nano| f(nano.get_or_insert_with(Nano::default)))
}

struct Plugin;

impl Guest for Plugin {
    fn init(config: String) -> Result<(), String> {
        let settings: serde_json::Value =
            serde_json::from_str(&config).map_err(|err| err.to_string())?;
        let (keymap, errors) = keys::keymap(&settings, "global", &function_keys);
        if let Some(first) = errors.first() {
            ui::show_message(&format!("nano.toml: {first}; left out"));
        }
        input::push_layer();
        // No modes: typed chars always go into the text.
        input::set_mode("global", true);
        let view = view::active();
        view.set_cursor_shape(CursorShape::Block);
        // The start of whatever was selected, as a point.
        let selection = view.selection();
        let r = selection.ranges[selection.primary as usize];
        set_ranges(&view, vec![point(r.anchor.min(r.head))], 0);
        with_nano(|nano| {
            nano.keymap = keymap;
            nano.toggle_help();
        });
        Ok(())
    }

    fn handle_key(ev: KeyEvent) -> KeyResult {
        with_nano(|nano| nano.handle_key(ev))
    }

    /// Pasted text goes in at the cursor, as if typed, or into the prompt.
    fn handle_paste(text: String) -> KeyResult {
        if prompts::active().is_some() {
            return KeyResult::Pass;
        }
        with_nano(|nano| {
            let view = view::active();
            nano.marking = false;
            let head = primary_head(&view);
            replace(&view, head, head, &text);
        });
        KeyResult::Handled
    }

    fn run_command(name: String, _args: String) -> Result<String, String> {
        Err(format!("no command {name}"))
    }

    fn on_event(ev: Event) {
        if let Event::PromptAction(act) = ev {
            with_nano(|nano| nano.prompt_action(act.id, act.action));
        }
    }
}

nib_plugin::export!(Plugin);

/// The keys that do nano function `name` here.
fn function_keys(name: &str) -> Option<Vec<KeyEvent>> {
    let (_, keys) = FUNCTIONS.iter().find(|(function, _)| *function == name)?;
    Some(keys::parse_keys(keys).expect("the table's keys parse"))
}

fn ctrl(ev: &KeyEvent) -> Option<char> {
    match ev.code {
        KeyCode::Char(c) if ev.modifiers == Modifiers::CTRL => Some(c),
        _ => None,
    }
}

fn alt(ev: &KeyEvent) -> Option<char> {
    match ev.code {
        KeyCode::Char(c) if ev.modifiers == Modifiers::ALT => Some(c.to_ascii_lowercase()),
        _ => None,
    }
}

fn primary_head(view: &View) -> u64 {
    let selection = view.selection();
    selection.ranges[selection.primary as usize].head
}

impl Nano {
    fn handle_key(&mut self, ev: KeyEvent) -> KeyResult {
        if prompts::active().is_some() {
            return self.prompt_key(ev);
        }
        if let Some(offer) = prompts::offered()
            && let Some(action) = choice_action(ev).filter(|a| offer.actions.contains(a))
        {
            prompts::act(action);
            return KeyResult::Handled;
        }
        let mut ev = ev;
        if std::mem::take(&mut self.meta) {
            ev.modifiers |= Modifiers::ALT;
        } else if ev.code == KeyCode::Escape && ev.modifiers.is_empty() {
            self.meta = true;
            return KeyResult::Handled;
        }
        let result = match self.sequence_key(ev) {
            Some(result) => result,
            None => self.key(ev),
        };
        self.show_hints();
        result
    }

    /// Keys of the settings, and plugins' keys under the leader: the Alt
    /// keys nano leaves. `None` when the key is neither.
    fn sequence_key(&mut self, ev: KeyEvent) -> Option<KeyResult> {
        let leader_key =
            !self.sequence.is_waiting() && alt(&ev).is_some_and(|c| !OWN_ALT.contains(&c));
        let step = if leader_key {
            let table = leader::keymap(&[], input::leader_keys()).keymap;
            let plain = KeyEvent {
                code: ev.code,
                modifiers: Modifiers::empty(),
            };
            self.sequence.key(&table, plain)
        } else if self.sequence.is_waiting() {
            self.sequence.key(&self.keymap, ev)
        } else {
            // The shown buffer's keys come before nano's own; the settings'
            // before both.
            let mut table = keys::buffer_keymap(&|_| false);
            keys::merge(&mut table, self.keymap.clone());
            if table.is_empty() {
                return None;
            }
            self.sequence.key(&table, ev)
        };
        match step {
            Step::NotMine if leader_key => {
                ui::show_message(&format!("{}: nothing here", keys::label(&ev)));
            }
            Step::NotMine => return None,
            Step::Wait | Step::Dropped(_) => {}
            Step::Run(Binding::Command(name), _) => call_or_show(&name),
            Step::Run(Binding::Keys(keys), _) => {
                for key in keys {
                    self.key(key);
                }
            }
            Step::Run(Binding::Prefix(_), _) => {}
        }
        Some(KeyResult::Handled)
    }

    fn show_hints(&mut self) {
        self.hints = self.sequence.waiting().map(|(table, typed)| {
            Popup::new(PopupAnchor::Corner, &hints::keymap_lines(typed, table))
        });
    }

    /// nano's own keys.
    fn key(&mut self, ev: KeyEvent) -> KeyResult {
        let view = view::active();
        let cutting = std::mem::take(&mut self.cutting);
        let typing = std::mem::take(&mut self.typing);
        let vertical = matches!(ev.code, KeyCode::Up | KeyCode::Down)
            || matches!(ctrl(&ev), Some('p' | 'n' | 'y' | 'v'))
            || matches!(ev.code, KeyCode::PageUp | KeyCode::PageDown);
        if !vertical {
            self.column = None;
        }
        let plain = ev.modifiers.is_empty();
        let with_ctrl = ev.modifiers == Modifiers::CTRL;
        match (ctrl(&ev), alt(&ev), ev.code) {
            (Some('x'), ..) => self.exit(),
            (Some('o'), ..) => self.ask_write(false),
            (Some('r'), ..) => self.ask(Ask::Insert, "File to insert: "),
            (Some('w'), ..) => {
                let label = match &self.search {
                    Some(last) => format!("Search [{last}]: "),
                    None => "Search: ".into(),
                };
                self.ask(Ask::Search, &label);
            }
            (Some('\\' | '4'), ..) => self.ask(Ask::ReplaceWhat, "Search (to replace): "),
            (Some('k'), ..) => self.cut(&view, cutting, true),
            (Some('u'), ..) => self.paste(&view),
            (Some('t'), ..) => call_or_show("picker.commands"),
            (Some('c'), ..) => location(&view),
            (Some('_' | '/' | '7'), ..) => {
                self.ask(Ask::GotoLine, "Enter line number, column number: ")
            }
            (Some('a'), ..) | (_, _, KeyCode::Home) => {
                self.go(&view, |doc, pos| doc.line_start(doc.line_of(pos)))
            }
            (Some('e'), ..) | (_, _, KeyCode::End) => self.go(&view, |doc, pos| doc.line_end(pos)),
            (Some('p'), ..) | (_, _, KeyCode::Up) => self.move_lines(&view, -1),
            (Some('n'), ..) | (_, _, KeyCode::Down) => self.move_lines(&view, 1),
            (Some('y'), ..) | (_, _, KeyCode::PageUp) => self.page(&view, -1),
            (Some('v'), ..) | (_, _, KeyCode::PageDown) => self.page(&view, 1),
            (Some(' '), ..) => self.go(&view, next_word),
            (_, Some(' '), _) => self.go(&view, prev_word),
            (_, _, KeyCode::Right) if with_ctrl => self.go(&view, next_word),
            (_, _, KeyCode::Left) if with_ctrl => self.go(&view, prev_word),
            (Some('b'), ..) | (_, _, KeyCode::Left) => {
                self.go(&view, |doc, pos| doc.prev_grapheme(pos))
            }
            (Some('f'), ..) | (_, _, KeyCode::Right) => {
                self.go(&view, |doc, pos| doc.next_grapheme(pos))
            }
            (Some('h'), ..) | (_, _, KeyCode::Backspace) => self.delete(&view, true),
            (Some('d'), ..) | (_, _, KeyCode::Delete) => self.delete(&view, false),
            (_, Some('u'), _) => {
                self.marking = false;
                if view.undo().is_none() {
                    ui::show_message("Nothing to undo");
                }
            }
            (_, Some('e'), _) => {
                self.marking = false;
                if view.redo().is_none() {
                    ui::show_message("Nothing to redo");
                }
            }
            (_, Some('a'), _) => self.toggle_mark(&view),
            (_, Some('6'), _) => self.cut(&view, cutting, false),
            (_, Some('w'), _) => self.find_again(&view, false),
            (_, Some('q'), _) => self.find_again(&view, true),
            (_, Some(']'), _) => self.go(&view, |doc, pos| {
                tree::matching_pair(&doc.buffer, pos)
                    .or_else(|| doc::matching_bracket(doc, pos))
                    .unwrap_or(pos)
            }),
            (_, Some('\\'), _) => self.go(&view, |_, _| 0),
            (_, Some('/'), _) => self.go(&view, |doc, _| doc.len),
            (_, Some('x'), _) => self.toggle_help(),
            (_, _, KeyCode::Enter) if plain => self.insert(&view, "\n", false),
            (_, _, KeyCode::Tab) if plain => self.insert(&view, &indent_unit(), false),
            (None, None, KeyCode::Char(c)) if !ev.modifiers.contains(Modifiers::CTRL) => {
                self.insert(&view, &c.to_string(), typing);
                self.typing = true;
            }
            _ => return KeyResult::Pass,
        }
        KeyResult::Handled
    }

    /// Moves the cursor to what `to` says; with the mark set, the selection
    /// grows or shrinks instead.
    fn go(&mut self, view: &View, to: impl Fn(&Doc, u64) -> u64) {
        let doc = Doc::new(view.buffer());
        let selection = view.selection();
        let r = selection.ranges[selection.primary as usize];
        let head = to(&doc, r.head);
        let anchor = if self.marking { r.anchor } else { head };
        set_ranges(view, vec![range(anchor, head)], 0);
    }

    fn move_lines(&mut self, view: &View, lines: i32) {
        let head = primary_head(view);
        if let Ok((pos, aimed)) = view.move_vertically(head, lines, self.column) {
            self.column = Some(aimed);
            self.go(view, |_, _| pos);
        }
    }

    fn page(&mut self, view: &View, direction: i32) {
        let lines = view.scroll(ScrollAmount::Page(direction));
        self.move_lines(view, lines);
    }

    fn insert(&mut self, view: &View, text: &str, join: bool) {
        let undo = if join {
            UndoMode::Merge
        } else {
            UndoMode::NewStep
        };
        let edit = insertion(primary_head(view), text.to_string());
        let version = view.buffer().version();
        let _ = view.apply(version, &[edit], None, undo);
    }

    /// The marked text, if any is.
    fn marked(&self, view: &View) -> Option<(u64, u64)> {
        let selection = view.selection();
        let r = selection.ranges[selection.primary as usize];
        (self.marking && r.anchor != r.head).then(|| (r.anchor.min(r.head), r.anchor.max(r.head)))
    }

    fn delete(&mut self, view: &View, backward: bool) {
        let doc = Doc::new(view.buffer());
        let head = primary_head(view);
        let (from, to) = match self.marked(view) {
            Some(marked) => marked,
            None if backward => (doc.prev_grapheme(head), head),
            None => (head, doc.next_grapheme(head)),
        };
        self.marking = false;
        replace(view, from, to, "");
    }

    /// ^K cuts the line, or the marked text; M-6 copies it, going to the
    /// next line after a line. Lines cut or copied one after another join.
    fn cut(&mut self, view: &View, again: bool, remove: bool) {
        let doc = Doc::new(view.buffer());
        let head = primary_head(view);
        let marked = self.marked(view);
        self.marking = false;
        let (from, to) = marked.unwrap_or_else(|| {
            let start = doc.line_start(doc.line_of(head));
            (start, (doc.line_end(head) + 1).min(doc.len))
        });
        if from == to {
            set_ranges(view, vec![point(head)], 0);
            return;
        }
        let text = doc.slice(from, to);
        if again && marked.is_none() {
            self.cut.push_str(&text);
        } else {
            self.cut = text;
        }
        self.cutting = marked.is_none();
        if remove {
            replace(view, from, to, "");
        } else {
            let to = if marked.is_none() { to } else { head };
            set_ranges(view, vec![point(to)], 0);
        }
    }

    fn paste(&mut self, view: &View) {
        if self.cut.is_empty() {
            ui::show_message("Cutbuffer is empty");
            return;
        }
        self.marking = false;
        let head = primary_head(view);
        replace(view, head, head, &self.cut.clone());
    }

    fn toggle_mark(&mut self, view: &View) {
        let head = primary_head(view);
        self.marking = !self.marking;
        set_ranges(view, vec![point(head)], 0);
        ui::show_message(if self.marking {
            "Mark Set"
        } else {
            "Mark Unset"
        });
    }

    fn toggle_help(&mut self) {
        self.help = match self.help.take() {
            Some(_) => None,
            None => {
                let lines: Vec<Vec<_>> = HELP
                    .iter()
                    .map(|row| {
                        row.iter()
                            .flat_map(|(key, what)| {
                                // Each item as wide as the others, so they
                                // line up whatever the key's length.
                                let width = HELP_ITEM - key.len() - 1;
                                [
                                    span(key, "ui.menu.selected"),
                                    span(&format!(" {what:<width$}"), ""),
                                ]
                            })
                            .collect()
                    })
                    .collect();
                Some(Panel::new(&lines))
            }
        };
    }

    fn ask(&mut self, ask: Ask, label: &str) {
        self.asking = Some(Asking {
            ask,
            line: Line::new(label),
        });
    }

    fn ask_write(&mut self, exit: bool) {
        let path = view::active().buffer().path().unwrap_or_default();
        self.ask(Ask::Write { exit }, "File Name to Write: ");
        if let Some(asking) = &self.asking {
            asking.line.set(&path, path.len() as u32);
        }
    }

    fn exit(&mut self) {
        if view::active().buffer().modified() || editor::quit(false).is_err() {
            self.ask(
                Ask::SaveFirst,
                "Save modified buffer?  Y Yes  N No  ^C Cancel ",
            );
        }
    }

    /// Keys while a prompt is open, this plugin's or another's: nano's
    /// prompt keys, and the core's for the rest.
    fn prompt_key(&mut self, ev: KeyEvent) -> KeyResult {
        let Some(active) = prompts::active() else {
            return KeyResult::Pass;
        };
        let answering = active.mine && matches!(&self.asking, Some(a) if a.ask == Ask::SaveFirst);
        if answering {
            match (ctrl(&ev), ev.code) {
                (Some('c'), _) => self.asking = None,
                (_, KeyCode::Char('y' | 'Y')) => {
                    self.asking = None;
                    self.save_and_exit();
                }
                (_, KeyCode::Char('n' | 'N')) => {
                    self.asking = None;
                    show_error(editor::quit(true));
                }
                _ => {}
            }
            return KeyResult::Handled;
        }
        let (text, cursor) = (active.text.as_str(), active.cursor as usize);
        let (text, cursor) = match ctrl(&ev) {
            Some('c') => {
                prompts::act(Action::Cancel);
                return KeyResult::Handled;
            }
            Some('a') => (text.to_string(), 0),
            Some('e') => (text.to_string(), text.len()),
            Some('b') => (text.to_string(), prev_char(text, cursor)),
            Some('f') => (text.to_string(), next_char(text, cursor)),
            Some('h') => {
                let before = prev_char(text, cursor);
                (format!("{}{}", &text[..before], &text[cursor..]), before)
            }
            Some('d') => {
                let after = next_char(text, cursor);
                (format!("{}{}", &text[..cursor], &text[after..]), cursor)
            }
            Some('k') => (String::new(), 0),
            Some('w') => line_edit::delete_word_before(text, cursor),
            _ => return KeyResult::Pass,
        };
        prompts::edit(&text, cursor as u32);
        KeyResult::Handled
    }

    fn save_and_exit(&mut self) {
        if view::active().buffer().path().is_none() {
            self.ask_write(true);
            return;
        }
        if write(None) {
            show_error(editor::quit(false));
        }
    }

    fn prompt_action(&mut self, id: u64, action: Action) {
        let Some(asking) = self.asking.take_if(|a| a.line.id() == id) else {
            return;
        };
        match action {
            Action::Accept => {}
            Action::Cancel => return ui::show_message("Cancelled"),
            _ => {
                self.asking = Some(asking);
                return;
            }
        }
        let text = asking.line.text();
        let view = view::active();
        match asking.ask {
            Ask::Write { exit } => {
                let path = view.buffer().path();
                let other = (!text.is_empty() && path.as_deref() != Some(text.as_str()))
                    .then_some(text.as_str());
                if write(other) && exit {
                    show_error(editor::quit(false));
                }
            }
            Ask::Insert => match std::fs::read_to_string(&text) {
                Ok(inserted) => {
                    let head = primary_head(&view);
                    replace(&view, head, head, &inserted);
                    let lines = inserted.lines().count();
                    ui::show_message(&format!("Read {lines} lines"));
                }
                Err(err) => ui::show_message(&format!("{text}: {err}")),
            },
            Ask::Search => {
                let pattern = if text.is_empty() {
                    self.search.clone()
                } else {
                    Some(text)
                };
                if let Some(pattern) = pattern {
                    self.search = Some(pattern.clone());
                    self.find(&view, &pattern, false);
                }
            }
            Ask::ReplaceWhat if !text.is_empty() => {
                self.ask(Ask::ReplaceWith(text), "Replace with: ");
            }
            Ask::ReplaceWhat => {}
            Ask::ReplaceWith(what) => replace_all(&view, &what, &text),
            Ask::GotoLine => go_to_line(&view, &text),
            Ask::SaveFirst => {}
        }
    }

    fn find_again(&mut self, view: &View, backward: bool) {
        match self.search.clone() {
            Some(pattern) => self.find(view, &pattern, backward),
            None => ui::show_message("No current search pattern"),
        }
    }

    /// Goes to the next match of `pattern` as it is written, after the
    /// cursor, or the one before it, wrapping around.
    fn find(&mut self, view: &View, pattern: &str, backward: bool) {
        let buffer = view.buffer();
        let doc = Doc::new(view.buffer());
        let head = primary_head(view);
        let regex = regex_escape(pattern);
        let start = if backward {
            head
        } else {
            doc.next_grapheme(head)
        };
        let found = match buffer.find(&regex, start, backward) {
            Ok(Some(found)) => Some(found),
            Ok(None) => {
                let wrap = if backward { buffer.len() } else { 0 };
                let found = buffer.find(&regex, wrap, backward).ok().flatten();
                if found.is_some() {
                    ui::show_message("Search Wrapped");
                }
                found
            }
            Err(err) => return ui::show_message(&error_message(err)),
        };
        match found {
            Some(found) => {
                let at = found.start;
                self.marking = false;
                set_ranges(view, vec![point(at)], 0);
            }
            None => ui::show_message(&format!("\"{pattern}\" not found")),
        }
    }
}

/// Replaces `from..to` with `text`, leaving the cursor after it.
fn replace(view: &View, from: u64, to: u64, text: &str) {
    let edit = Edit {
        start: from,
        end: to,
        text: text.to_string(),
    };
    let after = point(from + text.len() as u64);
    let selection = nib_plugin::nib::plugin::types::Selection {
        ranges: vec![after],
        primary: 0,
    };
    let version = view.buffer().version();
    let _ = view.apply(version, &[edit], Some(&selection), UndoMode::NewStep);
}

fn replace_all(view: &View, what: &str, with: &str) {
    let buffer = view.buffer();
    let found = match buffer.find_all(&regex_escape(what), 0, buffer.len()) {
        Ok(found) => found,
        Err(err) => return ui::show_message(&error_message(err)),
    };
    let edits: Vec<Edit> = found
        .iter()
        .filter(|r| r.start < r.end)
        .map(|r| Edit {
            start: r.start,
            end: r.end,
            text: with.to_string(),
        })
        .collect();
    if edits.is_empty() {
        return ui::show_message(&format!("\"{what}\" not found"));
    }
    let version = buffer.version();
    let _ = view.apply(version, &edits, None, UndoMode::NewStep);
    let n = edits.len();
    let plural = if n == 1 { "" } else { "s" };
    ui::show_message(&format!("Replaced {n} occurrence{plural}"));
}

/// Saves, under `path` if another one is given. Says how it went.
fn write(path: Option<&str>) -> bool {
    let buffer = view::active().buffer();
    match buffer.save(path) {
        Ok(()) => {
            let lines = buffer.line_count().saturating_sub(1).max(1);
            let plural = if lines == 1 { "" } else { "s" };
            ui::show_message(&format!("Wrote {lines} line{plural}"));
            true
        }
        Err(err) => {
            ui::show_message(&err);
            false
        }
    }
}

fn show_error(result: Result<(), String>) {
    if let Err(err) = result {
        ui::show_message(&err);
    }
}

/// ^C: where the cursor is, as nano says it.
fn location(view: &View) {
    let doc = Doc::new(view.buffer());
    let head = primary_head(view);
    let line = doc.line_of(head);
    let lines = doc.last_line() + 1;
    let start = doc.line_start(line);
    let column = doc.slice(start, head).chars().count() + 1;
    let width = doc.slice(start, doc.line_end(head)).chars().count() + 1;
    let percent = |part: u64, whole: u64| part * 100 / whole.max(1);
    ui::show_message(&format!(
        "line {}/{lines} ({}%), col {column}/{width} ({}%), char {head}/{} ({}%)",
        line + 1,
        percent(line + 1, lines),
        percent(column as u64, width as u64),
        doc.len,
        percent(head, doc.len),
    ));
}

/// ^_: "line" or "line,column", counted from 1.
fn go_to_line(view: &View, text: &str) {
    let mut parts = text.split(',').map(|part| part.trim().parse::<u64>());
    let (Some(Ok(line)), column) = (parts.next(), parts.next()) else {
        return ui::show_message("Invalid line or column number");
    };
    let doc = Doc::new(view.buffer());
    let start = doc.line_start(line.saturating_sub(1).min(doc.last_line()));
    let end = doc.line_end(start);
    let mut pos = start;
    for _ in 1..column.and_then(Result::ok).unwrap_or(1) {
        if pos >= end {
            break;
        }
        pos = doc.next_grapheme(pos);
    }
    set_ranges(view, vec![point(pos)], 0);
}

fn next_word(doc: &Doc, pos: u64) -> u64 {
    doc::next_word_start(doc, pos, false).map_or(doc.len, |(_, head)| head)
}

fn prev_word(doc: &Doc, pos: u64) -> u64 {
    doc::prev_word_start(doc, pos, false).map_or(0, |(_, head)| head)
}

fn prev_char(text: &str, cursor: usize) -> usize {
    text[..cursor]
        .char_indices()
        .next_back()
        .map_or(0, |(i, _)| i)
}

fn next_char(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .chars()
        .next()
        .map_or(cursor, |c| cursor + c.len_utf8())
}

/// What a key does to a list shown without a line to type into, such as
/// completions.
fn choice_action(ev: KeyEvent) -> Option<Action> {
    let plain = ev.modifiers.is_empty();
    match (ctrl(&ev), ev.code) {
        (Some('n'), _) => Some(Action::Next),
        (Some('p'), _) => Some(Action::Previous),
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
    fn every_function_has_keys_that_parse() {
        for (name, _) in FUNCTIONS {
            function_keys(name).unwrap();
        }
    }
}
