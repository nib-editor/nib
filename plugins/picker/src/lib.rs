//! Fuzzy pickers, in a box in the middle of the screen (docs/finder.md):
//! `picker.files` lists the files of the working directory, or of
//! `{"path": dir}`, with the core's `files.walk`, which honors .gitignore,
//! shows the chosen one beside the list, and opens it; `picker.commands`
//! lists every command with its description and runs the chosen one. The
//! query is a prompt: the base in use decides what keys do to it, and the
//! picker hears the result.

mod fuzzy;

use std::cell::RefCell;
use std::cmp::Reverse;

use nib_plugin::exports::nib::plugin::guest::{Guest, KeyResult};
use nib_plugin::nib::plugin::events::Event;
use nib_plugin::nib::plugin::prompt::{Action, FilePreview, Line, Preview};
use nib_plugin::nib::plugin::types::{KeyEvent, Span};
use nib_plugin::nib::plugin::ui;
use nib_plugin::nib::plugin::{buffer, commands, files, view};

/// Rows a page key moves.
const PAGE: isize = 10;
/// Rows given to the box around the selected one; it shows as many as fit.
const WINDOW: usize = 200;
/// Below this many matches, all are sorted at once.
const SORT_ALL: usize = 20_000;

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

    fn title(self) -> &'static str {
        match self {
            Kind::Files => "files",
            Kind::Commands => "commands",
        }
    }
}

struct Item {
    /// What the query matches and what is chosen: a path, or a command.
    text: String,
    /// Shown beside it: a command's description.
    detail: String,
}

struct Picker {
    kind: Kind,
    /// The query, which the core keeps and draws.
    line: Line,
    query: String,
    items: Vec<Item>,
    /// The matching items as (score, index). The first `sorted` are the
    /// best, best first; the rest are in no order.
    matches: Vec<(i64, usize)>,
    sorted: usize,
    selected: usize,
    /// The `files.walk` job still listing, if any.
    listing: Option<u64>,
    /// Where the files are listed from, when not the working directory.
    dir: Option<String>,
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

    fn run_command(name: String, args: String) -> Result<String, String> {
        let kind = match name.as_str() {
            "files" => Kind::Files,
            "commands" => Kind::Commands,
            _ => return Err(format!("no command {name}")),
        };
        let args: serde_json::Value = match args.trim() {
            "" => serde_json::Value::Null,
            args => serde_json::from_str(args).map_err(|err| format!("{name}: {err}"))?,
        };
        open(kind, args["path"].as_str().map(String::from))?;
        Ok("null".into())
    }

    fn on_event(ev: Event) {
        let chosen = PICKER.with_borrow_mut(|picker| {
            let open = picker.as_mut()?;
            match ev {
                Event::PromptChanged(change) if change.id == open.line.id() => {
                    open.narrow(change.text);
                }
                Event::PromptAction(act) if act.id == open.line.id() => match act.action {
                    Action::Next | Action::Complete => open.select(1),
                    Action::Previous | Action::CompleteBack => open.select(-1),
                    Action::PageNext => open.select(PAGE),
                    Action::PagePrevious => open.select(-PAGE),
                    Action::Cancel => {
                        close(picker);
                        return None;
                    }
                    Action::Accept => {
                        let text = open.chosen()?;
                        let kind = open.kind;
                        close(picker);
                        return Some((kind, text));
                    }
                },
                // Shown as they come, so a large tree does not keep it
                // empty; only what came is matched.
                Event::FilesListed(listed) if open.listing == Some(listed.job) => {
                    let from = open.items.len();
                    open.items.extend(listed.paths.into_iter().map(|text| Item {
                        text,
                        detail: String::new(),
                    }));
                    if listed.done {
                        open.listing = None;
                    }
                    open.add(from);
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

fn open(kind: Kind, dir: Option<String>) -> Result<(), String> {
    PICKER.with_borrow_mut(|picker| {
        if picker.is_some() {
            return Ok(());
        }
        let (items, listing) = match kind {
            Kind::Files => (Vec::new(), Some(files::walk(dir.as_deref())?)),
            Kind::Commands => {
                let mut items: Vec<Item> = commands::all()
                    .into_iter()
                    .map(|(text, detail)| Item { text, detail })
                    .collect();
                items.sort_by(|a, b| a.text.cmp(&b.text));
                (items, None)
            }
        };
        let line = Line::new(kind.prompt());
        line.show_in_box(kind.title());
        let open = picker.insert(Picker {
            kind,
            line,
            query: String::new(),
            items,
            matches: Vec::new(),
            sorted: 0,
            selected: 0,
            listing,
            dir,
        });
        open.add(0);
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
        "picker.files" => open(Kind::Files, None),
        "picker.commands" => open(Kind::Commands, None),
        _ => commands::call(name, "{}").map(|_| ()),
    }
}

impl Picker {
    /// Matches the items from `from` on, which just came, and adds them.
    fn add(&mut self, from: usize) {
        let before = self.matches.len();
        for i in from..self.items.len() {
            if let Some(score) = self.score(&self.items[i]) {
                self.matches.push((score, i));
            }
        }
        // Nothing typed, they all score the same and stay in order.
        if self.query.is_empty() {
            self.sorted = self.matches.len();
        } else if self.matches.len() > before {
            self.sorted = 0;
        }
    }

    /// The query changed: only what matched the old one can match one that
    /// goes on from it.
    fn narrow(&mut self, query: String) {
        let from_matches = !self.query.is_empty() && query.starts_with(&self.query);
        self.query = query;
        let candidates: Vec<usize> = match from_matches {
            true => self.matches.iter().map(|&(_, i)| i).collect(),
            false => (0..self.items.len()).collect(),
        };
        self.matches = candidates
            .into_iter()
            .filter_map(|i| Some((self.score(&self.items[i])?, i)))
            .collect();
        self.sorted = 0;
        if self.query.is_empty() {
            self.matches.sort_unstable_by_key(|&(_, i)| i);
            self.sorted = self.matches.len();
        }
        self.selected = 0;
    }

    /// Sorts the best `count` matches to the front, best first; the list
    /// order breaks ties.
    fn sort_to(&mut self, count: usize) {
        let count = count.min(self.matches.len());
        if self.sorted >= count {
            return;
        }
        let key = |&(score, i): &(i64, usize)| (Reverse(score), i);
        if self.matches.len() <= SORT_ALL || count * 2 > self.matches.len() {
            self.matches.sort_unstable_by_key(key);
            self.sorted = self.matches.len();
        } else {
            self.matches.select_nth_unstable_by_key(count - 1, key);
            self.matches[..count].sort_unstable_by_key(key);
            self.sorted = count;
        }
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
        let count = self.matches.len();
        if count > 0 {
            self.selected = (self.selected as isize + step).rem_euclid(count as isize) as usize;
        }
    }

    /// The text of the selected item: a path from the working directory,
    /// or a command.
    fn chosen(&mut self) -> Option<String> {
        self.sort_to(self.selected + 1);
        let &(_, i) = self.matches.get(self.selected)?;
        let text = &self.items[i].text;
        Some(match &self.dir {
            Some(dir) => format!("{}/{text}", dir.trim_end_matches('/')),
            None => text.clone(),
        })
    }

    fn show(&mut self) {
        let listing = if self.listing.is_some() { "…" } else { "" };
        self.line.set_hint(&format!(
            "{}/{}{listing}",
            self.matches.len(),
            self.items.len()
        ));
        let start = self.selected.saturating_sub(WINDOW / 2);
        let end = (start + WINDOW).min(self.matches.len());
        self.sort_to(end);
        let rows: Vec<Vec<Span>> = self.matches[start..end]
            .iter()
            .map(|&(_, i)| {
                let item = &self.items[i];
                let mut row = highlighted(&item.text, &fuzzy::positions(&self.query, &item.text));
                if self.kind == Kind::Commands && !item.detail.is_empty() {
                    row.push(span(&format!("  {}", item.detail), "comment"));
                }
                row
            })
            .collect();
        let selected = (!rows.is_empty()).then(|| (self.selected - start) as u32);
        self.line.set_rows(&rows, selected);
        let preview = match self.matches.get(self.selected) {
            None => Preview::None,
            Some(&(_, i)) => match self.kind {
                Kind::Files => Preview::File(FilePreview {
                    path: self.chosen().unwrap_or_default(),
                    line: None,
                }),
                Kind::Commands => {
                    let item = &self.items[i];
                    Preview::Lines(vec![
                        vec![span(&item.text, "ui.popup.title")],
                        Vec::new(),
                        vec![span(&item.detail, "")],
                    ])
                }
            },
        };
        self.line.set_preview(&preview);
    }
}

/// `text` with the chars at `matched` in the match color.
fn highlighted(text: &str, matched: &[usize]) -> Vec<Span> {
    let mut line: Vec<Span> = Vec::new();
    for (i, c) in text.chars().enumerate() {
        let style = if matched.contains(&i) {
            "ui.popup.key"
        } else {
            ""
        };
        match line.last_mut() {
            Some(last) if last.style == style => last.text.push(c),
            _ => line.push(span(&c.to_string(), style)),
        }
    }
    line
}

/// How much lower a match by description scores than one by name.
const DESCRIPTION_PENALTY: i64 = 1_000_000;

/// Closes the picker: dropping it closes its prompt, and a listing still
/// running is stopped.
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
