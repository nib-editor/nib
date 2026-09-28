//! Fuzzy pickers: `picker.files` lists the files of the working directory
//! with the core's `files.walk`, which honors .gitignore, and opens the
//! chosen one; `picker.commands` lists every command with its description
//! and runs the chosen one. The query is a prompt: the base in use decides
//! what keys do to it, and the picker hears the result.

mod fuzzy;

use std::cell::RefCell;

use nib_plugin::exports::nib::plugin::guest::{Guest, KeyResult};
use nib_plugin::nib::plugin::events::Event;
use nib_plugin::nib::plugin::prompt::{Action, Line};
use nib_plugin::nib::plugin::types::{KeyEvent, Span};
use nib_plugin::nib::plugin::ui::{self, Panel};
use nib_plugin::nib::plugin::{buffer, commands, files, view};

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
    /// The query, which the core keeps and draws.
    line: Line,
    /// The candidates, above the query.
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

    fn handle_key(_ev: KeyEvent) -> KeyResult {
        KeyResult::Pass
    }

    fn handle_paste(_text: String) -> KeyResult {
        KeyResult::Pass
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
        let chosen = PICKER.with_borrow_mut(|picker| {
            let open = picker.as_mut()?;
            match ev {
                Event::PromptChanged(change) if change.id == open.line.id() => {
                    open.query = change.text;
                    open.filter();
                }
                Event::PromptAction(act) if act.id == open.line.id() => match act.action {
                    Action::Next | Action::Complete => open.select(1),
                    Action::Previous | Action::CompleteBack => open.select(-1),
                    Action::PageNext => open.select(ROWS as isize),
                    Action::PagePrevious => open.select(-(ROWS as isize)),
                    Action::Cancel => {
                        close(picker);
                        return None;
                    }
                    Action::Accept => {
                        let &i = open.matches.get(open.selected)?;
                        let kind = open.kind;
                        let text = std::mem::take(&mut open.items[i].text);
                        close(picker);
                        return Some((kind, text));
                    }
                },
                // Shown as they come, so a large tree does not keep it
                // empty.
                Event::FilesListed(listed) if open.listing == Some(listed.job) => {
                    open.items.extend(listed.paths.into_iter().map(|text| Item {
                        text,
                        detail: String::new(),
                    }));
                    if listed.done {
                        open.listing = None;
                    }
                    open.filter();
                }
                _ => return None,
            }
            open.show();
            None
        });
        // Closed first, so the command runs with the picker gone, and may
        // open a picker again.
        if let Some((kind, text)) = chosen {
            let done = match kind {
                Kind::Files => open_file(&text),
                Kind::Commands => run(&text),
            };
            if let Err(err) = done {
                ui::show_message(&err);
            }
        }
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
        let open = picker.insert(Picker {
            kind,
            line: Line::new(kind.prompt()),
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

fn open_file(path: &str) -> Result<(), String> {
    let buffer = buffer::open(path)?;
    view::active().show(&buffer);
    Ok(())
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
            "listing…".to_string()
        } else {
            format!("{}/{}", self.matches.len(), self.items.len())
        };
        self.line.set_hint(&status);
        let mut lines = Vec::new();
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
        lines.resize(ROWS, Vec::new());
        self.panel.update(&lines);
    }
}

/// How much lower a match by description scores than one by name.
const DESCRIPTION_PENALTY: i64 = 1_000_000;

/// Closes the picker: dropping it closes its prompt and panel, and a
/// listing still running is stopped.
fn close(picker: &mut Option<Picker>) {
    if let Some(job) = picker.take().and_then(|p| p.listing) {
        files::cancel(job);
    }
}

fn span(text: &str, style: &str) -> Span {
    Span {
        text: text.into(),
        style: style.into(),
    }
}

nib_plugin::export!(Plugin);
