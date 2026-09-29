//! Directory listings (docs/files.md): `picker.directory` shows what a
//! directory holds in a buffer of this plugin's, as Vim's netrw or Emacs's
//! dired does, with their keys. Moving and searching are the base's, as in
//! any buffer.

use std::cell::{Cell, RefCell};
use std::time::{SystemTime, UNIX_EPOCH};

use nib_plugin::nib::plugin::buffer::{self, Buffer};
use nib_plugin::nib::plugin::files::{self, DirEntry};
use nib_plugin::nib::plugin::prompt::{Action, Line};
use nib_plugin::nib::plugin::types::{Edit, SelRange, Selection, UndoMode};
use nib_plugin::nib::plugin::ui::{self, Decoration};
use nib_plugin::nib::plugin::{commands, editor, input, view};

/// The commands of listings, by the name after `picker.`.
pub const COMMANDS: [(&str, &str); 7] = [
    (
        "directory",
        "List a directory: {\"path\": dir}, or the shown file's",
    ),
    ("directory-enter", "Open the file or directory on this line"),
    ("directory-up", "List the directory above"),
    (
        "directory-new-file",
        "Open a new file in the listed directory",
    ),
    ("directory-new-dir", "Make a directory in the listed one"),
    ("directory-refresh", "List the directory again"),
    ("directory-close", "Close the listing"),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Style {
    Netrw,
    Dired,
}

impl Style {
    pub fn parse(text: &str) -> Result<Style, String> {
        match text {
            "netrw" => Ok(Style::Netrw),
            "dired" => Ok(Style::Dired),
            other => Err(format!(
                "directory-style must be \"netrw\" or \"dired\", not {other:?}"
            )),
        }
    }

    fn keys(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Style::Netrw => &[
                ("ret", "picker.directory-enter"),
                ("-", "picker.directory-up"),
                ("%", "picker.directory-new-file"),
                ("d", "picker.directory-new-dir"),
                ("q", "picker.directory-close"),
            ],
            Style::Dired => &[
                ("ret", "picker.directory-enter"),
                ("^", "picker.directory-up"),
                ("+", "picker.directory-new-dir"),
                ("g", "picker.directory-refresh"),
                ("q", "picker.directory-close"),
            ],
        }
    }
}

/// A directory shown in a buffer.
struct Listing {
    buffer: Buffer,
    dir: String,
    style: Style,
    /// The line of the first row.
    first: usize,
    /// The column names start at.
    column: usize,
    rows: Vec<Row>,
}

struct Row {
    /// ".." for the directory above.
    name: String,
    directory: bool,
}

/// A name being typed, of a file to open or a directory to make.
struct Asking {
    line: Line,
    dir: String,
    directory: bool,
    text: String,
}

thread_local! {
    /// From the settings; without it, the base decides.
    pub static STYLE: Cell<Option<Style>> = const { Cell::new(None) };
    static LISTINGS: RefCell<Vec<Listing>> = const { RefCell::new(Vec::new()) };
    static ASKING: RefCell<Option<Asking>> = const { RefCell::new(None) };
}

/// Runs a listing's command; `None` if `name` is not one.
pub fn run(name: &str, args: &str) -> Option<Result<(), String>> {
    Some(match name {
        "directory" => open(args),
        "directory-enter" => enter(),
        "directory-up" => up(),
        "directory-new-file" => ask(false),
        "directory-new-dir" => ask(true),
        "directory-refresh" => refresh(),
        "directory-close" => close(),
        _ => return None,
    })
}

/// A listing's buffer was closed from outside it.
pub fn closed(name: &str) {
    LISTINGS.with_borrow_mut(|listings| listings.retain(|l| name_of(&l.dir) != name));
}

/// The text of the name being asked for changed.
pub fn prompt_changed(id: u64, text: String) -> bool {
    ASKING.with_borrow_mut(
        |asking| match asking.as_mut().filter(|a| a.line.id() == id) {
            Some(asking) => {
                asking.text = text;
                true
            }
            None => false,
        },
    )
}

/// A key acted on the name being asked for.
pub fn prompt_action(id: u64, action: Action) -> bool {
    let Some(asking) = ASKING.take() else {
        return false;
    };
    if asking.line.id() != id {
        ASKING.set(Some(asking));
        return false;
    }
    let name = asking.text.trim();
    if action == Action::Accept
        && !name.is_empty()
        && let Err(err) = answer(&asking.dir, name, asking.directory)
    {
        ui::show_message(&err);
    }
    true
}

fn style() -> Style {
    STYLE
        .get()
        .unwrap_or_else(|| match input::current_mode().base.as_str() {
            "vim" => Style::Netrw,
            _ => Style::Dired,
        })
}

/// `picker.directory`: the directory given, or the shown file's with the
/// cursor on it, or the working directory.
fn open(args: &str) -> Result<(), String> {
    let args: serde_json::Value = match args.trim() {
        "" => serde_json::Value::Null,
        args => serde_json::from_str(args).map_err(|err| format!("picker.directory: {err}"))?,
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
    let style = style();
    let above = parent(dir).map(|above| DirEntry {
        name: "..".into(),
        ..own_entry(&above).unwrap_or_else(|| blank(".."))
    });
    let here = own_entry(dir).map(|here| DirEntry {
        name: ".".into(),
        ..here
    });
    let shown: Vec<DirEntry> = match style {
        Style::Netrw => above.into_iter().chain(entries).collect(),
        Style::Dired => here.into_iter().chain(above).chain(entries).collect(),
    };
    let (text, decorations, first, column) = render(style, dir, &shown);
    let rows: Vec<Row> = shown
        .into_iter()
        .map(|entry| Row {
            name: entry.name,
            directory: entry.directory,
        })
        .collect();

    LISTINGS.with_borrow_mut(|listings| {
        let at = match listings.iter().position(|l| l.dir == dir) {
            Some(at) => at,
            None => {
                let buffer = buffer::create(&name_of(dir));
                listings.push(Listing {
                    buffer,
                    dir: dir.to_string(),
                    style,
                    first,
                    column,
                    rows: Vec::new(),
                });
                listings.len() - 1
            }
        };
        let listing = &mut listings[at];
        if listing.rows.is_empty() || listing.style != style {
            let keys: Vec<(String, String)> = style
                .keys()
                .iter()
                .map(|(key, command)| (key.to_string(), command.to_string()))
                .collect();
            listing.buffer.set_keys(&keys)?;
        }
        (listing.style, listing.first, listing.column) = (style, first, column);
        let buffer = &listing.buffer;
        let edit = Edit {
            start: 0,
            end: buffer.len(),
            text,
        };
        buffer
            .apply(buffer.version(), &[edit], UndoMode::NewStep)
            .map_err(|err| format!("{err:?}"))?;
        ui::set_decorations(buffer, "picker", &decorations);

        let chosen = select.and_then(|select| rows.iter().position(|row| row.name == select));
        let entry = rows
            .iter()
            .position(|row| row.name != "." && row.name != "..");
        let row = chosen.or(entry).or((!rows.is_empty()).then_some(0));
        listing.rows = rows;

        let view = view::active();
        view.show(buffer);
        let at = match row.and_then(|i| buffer.line_start((first + i) as u64)) {
            Some(start) => start + column as u64,
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

/// The entry `dir` has in the directory above it, for its own mode and
/// time.
fn own_entry(dir: &str) -> Option<DirEntry> {
    let above = parent(dir)?;
    let (_, name) = split(dir);
    files::list(&above)
        .ok()?
        .into_iter()
        .find(|e| e.name == name)
}

fn blank(name: &str) -> DirEntry {
    DirEntry {
        name: name.into(),
        directory: true,
        size: 0,
        mode: None,
        modified: None,
    }
}

/// The text of a listing, the decorations that color it, the line of its
/// first row, and the column its names start at.
fn render(
    style: Style,
    dir: &str,
    entries: &[DirEntry],
) -> (String, Vec<Decoration>, usize, usize) {
    let mut text = String::new();
    let mut decorations = Vec::new();
    let mut add = |text: &mut String, line: &str, style: &str| {
        let start = text.len() as u64;
        text.push_str(line);
        if !style.is_empty() {
            decorations.push(Decoration {
                start,
                end: text.len() as u64,
                style: style.into(),
            });
        }
    };
    let (first, column) = match style {
        Style::Netrw => {
            add(&mut text, "\" nib directory listing\n", "comment");
            add(&mut text, &format!("\"   {}\n", name_of(dir)), "comment");
            add(
                &mut text,
                "\"   <ret>:open  -:up  %:new file  d:new directory  q:close\n",
                "comment",
            );
            for entry in entries {
                let name = shown_name(entry);
                let look = if entry.directory { "ui.directory" } else { "" };
                add(&mut text, &name, look);
                text.push('\n');
            }
            (3, 0)
        }
        Style::Dired => {
            add(
                &mut text,
                &format!("  {}:\n", name_of(dir)),
                "ui.popup.title",
            );
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            let sizes: Vec<String> = entries
                .iter()
                .map(|e| {
                    if e.directory {
                        "-".into()
                    } else {
                        size(e.size)
                    }
                })
                .collect();
            let width = sizes.iter().map(String::len).max().unwrap_or(1);
            let mut column = 0;
            for (entry, size) in entries.iter().zip(&sizes) {
                let columns = format!(
                    "  {}  {size:>width$}  {:<11}  ",
                    mode(entry),
                    entry.modified.map_or(String::new(), |at| time(at, now)),
                );
                column = columns.len();
                text.push_str(&columns);
                let look = if entry.directory { "ui.directory" } else { "" };
                add(&mut text, &shown_name(entry), look);
                text.push('\n');
            }
            (1, column)
        }
    };
    (text, decorations, first, column)
}

fn shown_name(entry: &DirEntry) -> String {
    match entry.directory {
        true => format!("{}/", entry.name),
        false => entry.name.clone(),
    }
}

/// Permissions as `ls -l` writes them: `drwxr-xr-x`.
fn mode(entry: &DirEntry) -> String {
    let kind = if entry.directory { 'd' } else { '-' };
    let Some(mode) = entry.mode else {
        return format!("{kind}{}", " ".repeat(9));
    };
    let mut text = String::from(kind);
    for shift in [6, 3, 0] {
        let bits = (mode >> shift) & 0o7;
        text.push(if bits & 4 != 0 { 'r' } else { '-' });
        text.push(if bits & 2 != 0 { 'w' } else { '-' });
        text.push(if bits & 1 != 0 { 'x' } else { '-' });
    }
    text
}

/// When a file changed, as `ls -l` writes it: the day and time within half
/// a year, the day and year before.
fn time(at: u64, now: u64) -> String {
    const HALF_YEAR: u64 = 183 * 24 * 60 * 60;
    let local = editor::local_time(at);
    if now.abs_diff(at) < HALF_YEAR {
        format!(
            "{:02}-{:02} {:02}:{:02}",
            local.month, local.day, local.hour, local.minute
        )
    } else {
        format!("{}-{:02}-{:02}", local.year, local.month, local.day)
    }
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
            .find(|l| name_of(&l.dir) == name)
            .ok_or("not in a listing of files")?;
        let row = line
            .checked_sub(listing.first)
            .and_then(|i| listing.rows.get(i))
            .map(|row| (row.directory, row.name.clone()));
        Ok((listing.dir.clone(), row))
    })
}

/// Into a directory, in place of this listing, or opens a file in the view,
/// keeping the listing.
fn enter() -> Result<(), String> {
    let (dir, row) = at_cursor()?;
    match row {
        None => Ok(()),
        Some((_, name)) if name == "." => refresh(),
        Some((_, name)) if name == ".." => up(),
        Some((true, name)) => {
            show(&join(&dir, &name), None)?;
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

/// The directory above, with the cursor on the one left.
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

fn refresh() -> Result<(), String> {
    let (dir, row) = at_cursor()?;
    show(&dir, row.as_ref().map(|(_, name)| name.as_str()))
}

/// Asks for the name of a new file or directory in the listed one.
fn ask(directory: bool) -> Result<(), String> {
    let (dir, _) = at_cursor()?;
    let label = if directory {
        "new directory: "
    } else {
        "new file: "
    };
    ASKING.set(Some(Asking {
        line: Line::new(label),
        dir,
        directory,
        text: String::new(),
    }));
    Ok(())
}

/// Opens a new file `name`, made with its directories when it is saved, or
/// makes directory `name` and lists it among the others.
fn answer(dir: &str, name: &str, directory: bool) -> Result<(), String> {
    let path = join(dir, name);
    if directory {
        files::make_dir(&path)?;
        let first = name.split(['/', '\\']).next().unwrap_or(name);
        return show(dir, Some(first));
    }
    let buffer = buffer::open(&path)?;
    view::active().show(&buffer);
    Ok(())
}

/// Closes the listing in the focused view.
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

/// Registers the listings' commands.
pub fn register() {
    for (name, description) in COMMANDS {
        commands::register(name, description);
    }
}

/// How a directory's listing is named: its path and a separator.
fn name_of(dir: &str) -> String {
    match dir.ends_with(['/', '\\']) {
        true => dir.to_string(),
        false => format!("{dir}{}", separator(dir)),
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
    fn sizes_and_modes_read_as_ls_writes_them() {
        assert_eq!(size(812), "812");
        assert_eq!(size(2150), "2.1K");
        assert_eq!(size(3 << 20), "3.0M");
        assert_eq!(size(40 << 10), "40K");
        let entry = |directory, mode| DirEntry {
            name: String::new(),
            directory,
            size: 0,
            mode,
            modified: None,
        };
        assert_eq!(mode(&entry(true, Some(0o40755))), "drwxr-xr-x");
        assert_eq!(mode(&entry(false, Some(0o100640))), "-rw-r-----");
        assert_eq!(mode(&entry(false, None)), "-         ");
    }
}
