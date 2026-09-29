//! Directory listings (docs/files.md): `files.open` shows what a directory
//! holds in a buffer of this plugin's, as Emacs's dired and Vim's netrw do.
//! In it, Enter opens the file or directory on the cursor's line, `-` goes
//! up, `+` opens a new file, and `q` closes it. Moving and searching are
//! the base's, as in any buffer.

use std::cell::RefCell;

use nib_plugin::exports::nib::plugin::guest::{Guest, KeyResult};
use nib_plugin::nib::plugin::buffer::{self, Buffer};
use nib_plugin::nib::plugin::events::Event;
use nib_plugin::nib::plugin::files;
use nib_plugin::nib::plugin::prompt::{Action, Line};
use nib_plugin::nib::plugin::types::{Edit, KeyEvent, SelRange, Selection, UndoMode};
use nib_plugin::nib::plugin::ui::{self, Decoration};
use nib_plugin::nib::plugin::{commands, editor, view};

/// Its keys in a listing, which vim and helix leave to it.
const KEYS: [(&str, &str); 4] = [
    ("ret", "files.enter"),
    ("-", "files.up"),
    ("+", "files.create"),
    ("q", "files.close"),
];

/// Columns before a name.
const INDENT: &str = "  ";

/// A directory shown in a buffer.
struct Listing {
    buffer: Buffer,
    dir: String,
    /// What each line from the second stands for.
    rows: Vec<Row>,
}

enum Row {
    Up,
    Entry {
        name: String,
        directory: bool,
        size: u64,
    },
}

/// The name of a new file being typed, for the directory it goes in.
struct Creating {
    line: Line,
    dir: String,
    text: String,
}

thread_local! {
    static LISTINGS: RefCell<Vec<Listing>> = const { RefCell::new(Vec::new()) };
    static CREATING: RefCell<Option<Creating>> = const { RefCell::new(None) };
}

struct Plugin;

impl Guest for Plugin {
    fn init(_config: String) -> Result<(), String> {
        commands::register(
            "open",
            "List a directory: {\"path\": dir}, or the shown file's",
        );
        commands::register("enter", "Open the file or directory on this line");
        commands::register("up", "List the directory above");
        commands::register("create", "Open a new file in the listed directory");
        commands::register("close", "Close the listing");
        Ok(())
    }

    fn handle_key(_ev: KeyEvent) -> KeyResult {
        KeyResult::Pass
    }

    fn handle_paste(_text: String) -> KeyResult {
        KeyResult::Pass
    }

    fn run_command(name: String, args: String) -> Result<String, String> {
        match name.as_str() {
            "open" => open(&args),
            "enter" => enter(),
            "up" => up(),
            "create" => create(),
            "close" => close(),
            _ => Err(format!("no command {name}")),
        }
        .map(|()| "null".into())
    }

    fn on_event(ev: Event) {
        match ev {
            Event::BufferClosed(closed) if closed.path.is_none() => {
                LISTINGS.with_borrow_mut(|listings| {
                    listings.retain(|l| l.buffer_name() != closed.name);
                });
            }
            Event::PromptChanged(change) => CREATING.with_borrow_mut(|creating| {
                if let Some(creating) = creating.as_mut().filter(|c| c.line.id() == change.id) {
                    creating.text = change.text;
                }
            }),
            Event::PromptAction(act) => {
                let Some(creating) = CREATING.take() else {
                    return;
                };
                if creating.line.id() != act.id {
                    CREATING.set(Some(creating));
                    return;
                }
                if act.action == Action::Accept
                    && let Err(err) = open_new(&creating.dir, creating.text.trim())
                {
                    ui::show_message(&err);
                }
            }
            _ => {}
        }
    }
}

nib_plugin::export!(Plugin);

impl Listing {
    fn buffer_name(&self) -> String {
        name_of(&self.dir)
    }
}

/// `files.open`: the directory given, or the shown file's with the cursor
/// on it, or the working directory.
fn open(args: &str) -> Result<(), String> {
    let args: serde_json::Value = match args.trim() {
        "" => serde_json::Value::Null,
        args => serde_json::from_str(args).map_err(|err| format!("files.open: {err}"))?,
    };
    let cwd = editor::working_directory();
    if let Some(dir) = args["path"].as_str() {
        return show(&absolute(&cwd, dir), None);
    }
    match view::active().buffer().path() {
        Some(path) => {
            let path = absolute(&cwd, &path);
            let (dir, name) = split(&path);
            show(&dir, Some(&name))
        }
        None => show(&cwd, None),
    }
}

/// Lists `dir` in the focused view, with the cursor on `select`, or on the
/// first entry. A listing of `dir` already open is listed again.
fn show(dir: &str, select: Option<&str>) -> Result<(), String> {
    let mut entries = files::list(dir)?;
    // Directories first, each part by name as the core sorted them.
    entries.sort_by_key(|entry| !entry.directory);
    let mut rows = Vec::new();
    if parent(dir).is_some() {
        rows.push(Row::Up);
    }
    rows.extend(entries.into_iter().map(|entry| Row::Entry {
        name: entry.name,
        directory: entry.directory,
        size: entry.size,
    }));
    let (text, decorations) = render(dir, &rows);

    LISTINGS.with_borrow_mut(|listings| {
        let at = match listings.iter().position(|l| l.dir == dir) {
            Some(at) => at,
            None => {
                let buffer = buffer::create(&name_of(dir));
                let keys: Vec<(String, String)> = KEYS
                    .iter()
                    .map(|(key, command)| (key.to_string(), command.to_string()))
                    .collect();
                buffer.set_keys(&keys)?;
                listings.push(Listing {
                    buffer,
                    dir: dir.to_string(),
                    rows: Vec::new(),
                });
                listings.len() - 1
            }
        };
        let listing = &mut listings[at];
        let buffer = &listing.buffer;
        let edit = Edit {
            start: 0,
            end: buffer.len(),
            text,
        };
        buffer
            .apply(buffer.version(), &[edit], UndoMode::NewStep)
            .map_err(|err| format!("{err:?}"))?;
        ui::set_decorations(buffer, "files", &decorations);

        let chosen = rows.iter().position(|row| match (row, select) {
            (Row::Entry { name, .. }, Some(select)) => name == select,
            _ => false,
        });
        let first = rows.iter().position(|row| matches!(row, Row::Entry { .. }));
        let line = chosen.or(first).or((!rows.is_empty()).then_some(0));
        listing.rows = rows;

        let view = view::active();
        view.show(buffer);
        let at = match line.and_then(|i| buffer.line_start(i as u64 + 1)) {
            Some(start) => start + INDENT.len() as u64,
            None => 0,
        };
        let _ = view.set_selection(&Selection {
            ranges: vec![SelRange {
                anchor: at,
                head: at,
            }],
            primary: 0,
        });
        Ok(())
    })
}

/// The text of a listing: the directory, then a row a line, and the
/// decorations that color the directories.
fn render(dir: &str, rows: &[Row]) -> (String, Vec<Decoration>) {
    let width = rows
        .iter()
        .map(|row| match row {
            Row::Up => 3,
            Row::Entry {
                name, directory, ..
            } => name.chars().count() + usize::from(*directory),
        })
        .max()
        .unwrap_or(0);
    let mut text = format!("{}\n", name_of(dir));
    let mut decorations = vec![Decoration {
        start: 0,
        end: text.len() as u64 - 1,
        style: "ui.popup.title".into(),
    }];
    for row in rows {
        text.push_str(INDENT);
        let start = text.len() as u64;
        match row {
            Row::Up => text.push_str("../"),
            Row::Entry {
                name,
                directory: true,
                ..
            } => text.push_str(&format!("{name}/")),
            Row::Entry { name, size: n, .. } => {
                let pad = width.saturating_sub(name.chars().count());
                text.push_str(&format!("{name}{}  {:>6}", " ".repeat(pad), size(*n)));
            }
        }
        if matches!(
            row,
            Row::Up
                | Row::Entry {
                    directory: true,
                    ..
                }
        ) {
            decorations.push(Decoration {
                start,
                end: text.len() as u64,
                style: "ui.directory".into(),
            });
        }
        text.push('\n');
    }
    (text, decorations)
}

/// The listing in the focused view and the row of the cursor's line.
fn at_cursor() -> Result<(String, Option<(bool, String)>), String> {
    let view = view::active();
    let buffer = view.buffer();
    let selection = view.selection();
    let head = selection.ranges[selection.primary as usize].head;
    let line = buffer.line_of(head).unwrap_or(0) as usize;
    LISTINGS.with_borrow(|listings| {
        let name = buffer.name();
        let listing = listings
            .iter()
            .find(|l| l.buffer_name() == name)
            .ok_or("not in a listing of files")?;
        let row = line.checked_sub(1).and_then(|i| listing.rows.get(i));
        let row = row.map(|row| match row {
            Row::Up => (true, "..".to_string()),
            Row::Entry {
                name, directory, ..
            } => (*directory, name.clone()),
        });
        Ok((listing.dir.clone(), row))
    })
}

/// `files.enter`: into a directory, in place of this listing, or opens a
/// file in the view, keeping the listing.
fn enter() -> Result<(), String> {
    let (dir, row) = at_cursor()?;
    match row {
        None => Ok(()),
        Some((_, name)) if name == ".." => up(),
        Some((true, name)) => {
            let into = join(&dir, &name);
            show(&into, None)?;
            forget(&dir);
            Ok(())
        }
        Some((false, name)) => {
            let buffer = buffer::open(&join(&dir, &name))?;
            view::active().show(&buffer);
            Ok(())
        }
    }
}

/// `files.up`: the directory above, with the cursor on the one left.
fn up() -> Result<(), String> {
    let (dir, _) = at_cursor()?;
    let Some(above) = parent(&dir) else {
        return Ok(());
    };
    let (_, name) = split(&dir);
    show(&above, Some(&name))?;
    forget(&dir);
    Ok(())
}

/// `files.create`: asks for the name of a new file in the listed
/// directory.
fn create() -> Result<(), String> {
    let (dir, _) = at_cursor()?;
    CREATING.set(Some(Creating {
        line: Line::new("new file: "),
        dir,
        text: String::new(),
    }));
    Ok(())
}

/// Opens `name`, which may name directories under `dir` too; the file and
/// its directories are made when it is saved.
fn open_new(dir: &str, name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Ok(());
    }
    let buffer = buffer::open(&join(dir, name))?;
    view::active().show(&buffer);
    Ok(())
}

/// `files.close`: closes the listing in the focused view.
fn close() -> Result<(), String> {
    let (dir, _) = at_cursor()?;
    forget(&dir);
    Ok(())
}

/// Closes the listing of `dir`; views that showed it show the buffer
/// before.
fn forget(dir: &str) {
    let closing = LISTINGS.with_borrow_mut(|listings| {
        let at = listings.iter().position(|l| l.dir == dir)?;
        Some(listings.remove(at))
    });
    if let Some(listing) = closing {
        let _ = listing.buffer.close(true);
    }
}

/// How a directory's listing is named: its path and a separator.
fn name_of(dir: &str) -> String {
    let separator = separator(dir);
    match dir.ends_with(['/', '\\']) {
        true => dir.to_string(),
        false => format!("{dir}{separator}"),
    }
}

fn separator(path: &str) -> char {
    if path.contains('\\') && !path.contains('/') {
        '\\'
    } else {
        '/'
    }
}

fn join(dir: &str, name: &str) -> String {
    format!("{}{name}", name_of(dir))
}

fn absolute(cwd: &str, path: &str) -> String {
    let rooted = path.starts_with(['/', '\\']) || path.get(1..3) == Some(":\\");
    let path = if rooted {
        path.to_string()
    } else {
        join(cwd, path.trim_start_matches("./"))
    };
    match path.len() > 1 {
        true => path.trim_end_matches(['/', '\\']).to_string(),
        false => path,
    }
}

/// The directory above `dir`, if it is not a root.
fn parent(dir: &str) -> Option<String> {
    let trimmed = dir.trim_end_matches(['/', '\\']);
    let at = trimmed.rfind(['/', '\\'])?;
    Some(match &trimmed[..at] {
        // The root, as in "/home".
        "" => trimmed[..=at].to_string(),
        // A drive, as in "C:\\Users".
        drive if drive.ends_with(':') => trimmed[..=at].to_string(),
        above => above.to_string(),
    })
}

/// A path's directory and name.
fn split(path: &str) -> (String, String) {
    let trimmed = path.trim_end_matches(['/', '\\']);
    let name = trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed);
    let dir = parent(trimmed).unwrap_or_else(|| trimmed.to_string());
    (dir, name.to_string())
}

/// A size as `ls -h` writes it.
fn size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["K", "M", "G", "T"];
    if bytes < 1024 {
        return bytes.to_string();
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    match value < 10.0 {
        true => format!("{value:.1}{}", UNITS[unit]),
        false => format!("{value:.0}{}", UNITS[unit]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_go_up_to_the_root() {
        assert_eq!(parent("/home/me"), Some("/home".into()));
        assert_eq!(parent("/home"), Some("/".into()));
        assert_eq!(parent("/"), None);
        assert_eq!(parent("C:\\Users"), Some("C:\\".into()));
        assert_eq!(split("/home/me/a.txt"), ("/home/me".into(), "a.txt".into()));
        assert_eq!(join("/", "home"), "/home");
        assert_eq!(join("/home", "me"), "/home/me");
        assert_eq!(absolute("/home", "me/"), "/home/me");
        assert_eq!(absolute("/home", "/etc"), "/etc");
        assert_eq!(name_of("/home/me"), "/home/me/");
    }

    #[test]
    fn sizes_read_as_ls_writes_them() {
        assert_eq!(size(812), "812");
        assert_eq!(size(2150), "2.1K");
        assert_eq!(size(3 << 20), "3.0M");
        assert_eq!(size(40 << 10), "40K");
    }
}
