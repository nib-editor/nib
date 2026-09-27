//! Fuzzy pickers: `picker.files` lists the files of the working directory
//! with the core's `files.walk`, which honors .gitignore, and opens the
//! chosen one; `picker.commands` lists every command with its description
//! and runs the chosen one. While open, a picker takes the keys with its
//! own input layer.

mod fuzzy;

use std::cell::RefCell;

use nib_plugin::exports::nib::plugin::guest::{Guest, KeyResult};
use nib_plugin::nib::plugin::events::Event;
use nib_plugin::nib::plugin::types::{KeyCode, KeyEvent, Modifiers, Span};
use nib_plugin::nib::plugin::ui::{self, Panel};
use nib_plugin::nib::plugin::{commands, files, input};

/// Candidates shown at once.
const ROWS: usize = 10;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Files,
    Commands,
}

impl Kind {
    fn prompt(self) -> &'static str {
        match self {
            Kind::Files => "files> ",
            Kind::Commands => "commands> ",
        }
    }
}

struct Item {
    /// What the query matches and what is chosen: a path, or a command.
    text: String,
    /// Shown after it: a command's description.
    detail: String,
}

struct Picker {
    kind: Kind,
    panel: Panel,
    query: String,
    items: Vec<Item>,
    /// Indices into `items`, best first.
    matches: Vec<usize>,
    selected: usize,
    /// The `files.walk` job still listing, if any.
    listing: Option<u64>,
}

thread_local! {
    static PICKER: RefCell<Option<Picker>> = const { RefCell::new(None) };
}

struct Plugin;

impl Guest for Plugin {
    fn init(_config: String) -> Result<(), String> {
        commands::register("files", "Pick a file to open");
        commands::register("commands", "Pick a command to run");
        Ok(())
    }

    fn handle_key(ev: KeyEvent) -> KeyResult {
        let chosen = PICKER.with_borrow_mut(|picker| {
            let open = picker.as_mut()?;
            match key(open, ev) {
                Action::Stay => {
                    open.show();
                    None
                }
                Action::Close => {
                    close(picker);
                    None
                }
                Action::Accept(i) => {
                    let kind = open.kind;
                    let text = std::mem::take(&mut open.items[i].text);
                    close(picker);
                    Some((kind, text))
                }
            }
        });
        // Closed first, so the command runs with the keys back where they
        // were, and may open a picker again.
        if let Some((kind, text)) = chosen {
            let done = match kind {
                Kind::Files => {
                    let args = format!("{{\"path\":{}}}", json_string(&text));
                    commands::call("buffer.open", &args).map(|_| ())
                }
                Kind::Commands => run(&text),
            };
            if let Err(err) = done {
                ui::show_message(&err);
            }
        }
        // It owns the keys while open.
        KeyResult::Handled
    }

    fn run_command(name: String, _args: String) -> Result<String, String> {
        let kind = match name.as_str() {
            "files" => Kind::Files,
            "commands" => Kind::Commands,
            _ => return Err(format!("no command {name}")),
        };
        open(kind)?;
        Ok("null".into())
    }

    fn on_event(ev: Event) {
        PICKER.with_borrow_mut(|picker| {
            let Some(open) = picker else {
                return;
            };
            // Shown as they come, so a large tree does not keep it empty.
            if let Event::FilesListed(listed) = ev
                && open.listing == Some(listed.job)
            {
                open.items.extend(listed.paths.into_iter().map(|text| Item {
                    text,
                    detail: String::new(),
                }));
                if listed.done {
                    open.listing = None;
                }
                open.filter();
                open.show();
            }
        })
    }
}

fn open(kind: Kind) -> Result<(), String> {
    PICKER.with_borrow_mut(|picker| {
        if picker.is_some() {
            return Ok(());
        }
        let (items, listing) = match kind {
            Kind::Files => (Vec::new(), Some(files::walk(None)?)),
            Kind::Commands => {
                let mut items: Vec<Item> = commands::all()
                    .into_iter()
                    .map(|(text, detail)| Item { text, detail })
                    .collect();
                items.sort_by(|a, b| a.text.cmp(&b.text));
                (items, None)
            }
        };
        input::push_layer();
        let open = picker.insert(Picker {
            kind,
            panel: Panel::new(&[]),
            query: String::new(),
            items,
            matches: Vec::new(),
            selected: 0,
            listing,
        });
        open.filter();
        open.show();
        Ok(())
    })
}

/// Runs a chosen command. This plugin's own are run here: calling them
/// through the core would call into this plugin while it is in a call.
fn run(name: &str) -> Result<(), String> {
    match name {
        "picker.files" => open(Kind::Files),
        "picker.commands" => open(Kind::Commands),
        _ => commands::call(name, "{}").map(|_| ()),
    }
}

enum Action {
    Stay,
    Close,
    Accept(usize),
}

fn key(picker: &mut Picker, ev: KeyEvent) -> Action {
    let ctrl = ev.modifiers == Modifiers::CTRL;
    match ev.code {
        KeyCode::Escape => return Action::Close,
        KeyCode::Enter => {
            return match picker.matches.get(picker.selected) {
                Some(&i) => Action::Accept(i),
                None => Action::Stay,
            };
        }
        KeyCode::Up => picker.select(-1),
        KeyCode::Down => picker.select(1),
        KeyCode::Char('p') if ctrl => picker.select(-1),
        KeyCode::Char('n') if ctrl => picker.select(1),
        KeyCode::Backspace => {
            picker.query.pop();
            picker.filter();
        }
        KeyCode::Char(c) if ev.modifiers.is_empty() => {
            picker.query.push(c);
            picker.filter();
        }
        _ => {}
    }
    Action::Stay
}

impl Picker {
    fn filter(&mut self) {
        let mut scored: Vec<(i64, usize)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| Some((self.score(item)?, i)))
            .collect();
        // Best first; the list order breaks ties, so an empty query keeps it.
        scored.sort_by_key(|&(score, i)| (std::cmp::Reverse(score), i));
        self.matches = scored.into_iter().map(|(_, i)| i).collect();
        self.selected = 0;
    }

    /// A command also matches by its description, below any match by name.
    fn score(&self, item: &Item) -> Option<i64> {
        fuzzy::score(&self.query, &item.text).or_else(|| {
            (self.kind == Kind::Commands)
                .then(|| fuzzy::score(&self.query, &item.detail))
                .flatten()
                .map(|score| score - DESCRIPTION_PENALTY)
        })
    }

    fn select(&mut self, step: isize) {
        let count = self.matches.len().min(ROWS);
        if count > 0 {
            self.selected = (self.selected as isize + step).rem_euclid(count as isize) as usize;
        }
    }

    fn show(&self) {
        let status = if self.listing.is_some() {
            "  listing…".to_string()
        } else {
            format!("  {}/{}", self.matches.len(), self.items.len())
        };
        let prompt = self.kind.prompt();
        let mut lines = vec![vec![
            span(prompt, ""),
            span(&self.query, ""),
            span(&status, "comment"),
        ]];
        let width = self
            .matches
            .iter()
            .take(ROWS)
            .map(|&i| self.items[i].text.len())
            .max()
            .unwrap_or(0);
        for (row, &i) in self.matches.iter().take(ROWS).enumerate() {
            let style = if row == self.selected {
                "ui.menu.selected"
            } else {
                ""
            };
            let item = &self.items[i];
            let mut line = vec![span(&format!(" {:width$}", item.text), style)];
            if !item.detail.is_empty() {
                line.push(span(&format!("  {}", item.detail), "comment"));
            }
            lines.push(line);
        }
        // A fixed height, so the text above does not jump while typing.
        lines.resize(ROWS + 1, Vec::new());
        self.panel.update(&lines);
        let cursor = (prompt.len() + self.query.len()) as u32;
        self.panel.set_cursor(Some((0, cursor)));
    }
}

/// How much lower a match by description scores than one by name.
const DESCRIPTION_PENALTY: i64 = 1_000_000;

/// Closes the picker: dropping it closes its panel, and a listing still
/// running is stopped.
fn close(picker: &mut Option<Picker>) {
    if let Some(job) = picker.take().and_then(|p| p.listing) {
        files::cancel(job);
    }
    input::pop_layer();
}

fn span(text: &str, style: &str) -> Span {
    Span {
        text: text.into(),
        style: style.into(),
    }
}

fn json_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
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

nib_plugin::export!(Plugin);
