use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Instant;

use ropey::Rope;

use crate::Edit;
use crate::Error;
use crate::background::{Inbox, Message, Waker};
use crate::buffer::Buffer;
use crate::clipboard::{self, Clipboard};
use crate::config::{CONFIG_TEMPLATE, Config, Indent, PluginConfig, Settings, plugin_template};
use crate::events::{Command, Event, Mode, Timer};
use crate::files::FileJobs;
use crate::history::UndoMode;
use crate::input::KeyEvent;
use crate::layout;
use crate::plugin::{LeaderKey, PluginId, Plugins};
use crate::process::Processes;
use crate::prompt::{Action, Choices, Outcome, Prompt};
use crate::selection::Selection;
use crate::syntax::{BufferSyntax, Languages};
use crate::ui::{Panel, Popup, StatusItem, Theme};
use crate::updates::{Checked, PendingInstall, PendingUpdate, PluginStore, Prepared};
use crate::view::View;
use crate::windows::{self, Direction, Rect, Separator, Splits};
use std::sync::Arc;

/// Everything plugins can see and change. While a plugin runs, it is lent to
/// that plugin's store so host functions can reach it.
pub(crate) struct State {
    pub buffers: Vec<Buffer>,
    /// The focused view, which takes the keys.
    pub view: View,
    /// Its id in `tree`.
    pub focused: u32,
    /// The other views, by id.
    pub others: Vec<(u32, View)>,
    /// How the views share the screen.
    pub splits: Splits,
    pub last_view_id: u32,
    pub width: u16,
    pub height: u16,
    pub quit: bool,
    /// Plugins taking keys, bottom first.
    pub layers: Vec<PluginId>,
    /// Shown to the user until the next key, e.g. a plugin error.
    pub message: Option<String>,
    /// The core menu is open and takes the next key.
    pub menu: Option<Menu>,
    pub settings: Settings,
    pub status: Vec<StatusItem>,
    /// Bottom panels, in the order they were opened.
    pub panels: Vec<Panel>,
    pub last_panel_id: u32,
    /// Popups, drawn in the order they were opened.
    pub popups: Vec<Popup>,
    pub last_popup_id: u32,
    /// Prompts, oldest first; keys go to the last one.
    pub prompts: Vec<Prompt>,
    /// Choices, oldest first; the base acts on the last one. They share
    /// ids with prompts, as their events do.
    pub choices: Vec<Choices>,
    pub last_prompt_id: u32,
    /// The base acted on the last choices during the key being handled.
    pub choices_acted: bool,
    /// What enabled plugins suggest under the base's leader.
    pub leader_keys: Vec<LeaderKey>,
    /// Events waiting for the current call to end, with the plugin they
    /// are for, or `None` for every plugin that listens to their kind.
    pub events: VecDeque<(Option<PluginId>, Event)>,
    /// Commands plugins registered.
    pub commands: Vec<Command>,
    pub timers: Vec<Timer>,
    pub last_timer_id: u64,
    /// Where background threads queue their messages.
    pub inbox: Arc<Inbox>,
    /// Programs plugins started.
    pub processes: Processes,
    /// File lists being made for plugins.
    pub files: FileJobs,
    pub clipboard: Box<dyn Clipboard>,
    /// Views of buffers not shown, so switching back restores the selection
    /// and scroll position.
    /// The plugin the arrows point at in the core menu's list.
    /// Where the settings are, from the frontend.
    pub config_dir: Option<PathBuf>,
    /// Each plugin's `settings.example.toml`, by name, for the template of
    /// its settings file.
    pub settings_examples: BTreeMap<String, String>,
    /// The file a prompt's box shows last, kept while the same one is
    /// asked for.
    pub(crate) preview: Option<crate::preview::LoadedFile>,
    /// A file there was saved, or config.reload called: the editor reads
    /// the settings again after the call.
    pub reload_config: bool,
    pub menu_cursor: usize,
    /// What is typed into the core menu, as what to install.
    pub menu_input: String,
    pub hidden_views: HashMap<usize, View>,
    pub languages: Languages,
    pub theme: Theme,
    /// What the running base says of its state.
    pub mode: Mode,
}

/// Where `view.focus` moves the focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Toward {
    Next,
    Left,
    Right,
    Up,
    Down,
}

impl State {
    /// The open buffer that has `path`.
    fn find_open(&self, path: &Path) -> Option<usize> {
        let same_file = |other: &Path| {
            other == path
                || matches!(
                    (other.canonicalize(), path.canonicalize()),
                    (Ok(a), Ok(b)) if a == b
                )
        };
        self.buffers
            .iter()
            .position(|b| !b.is_closed() && b.path().is_some_and(same_file))
    }

    fn load(&self, path: PathBuf) -> Result<Buffer, Error> {
        let template = self.settings_template(&path);
        let mut buffer = Buffer::open(path)?;
        if let Some(language) = buffer.path().and_then(|p| self.languages.for_path(p)) {
            buffer.syntax = Some(BufferSyntax::new(language));
        }
        // Unsaved, so the file is made only if it is saved.
        if let Some(template) = template {
            let version = buffer.version();
            let edit = vec![Edit::new(0, 0, template)];
            buffer.apply(version, edit, &Selection::point(0), None, UndoMode::NewStep)?;
        }
        Ok(buffer)
    }

    /// What a settings file not made yet starts as: `config.toml` or
    /// `plugins/<name>.toml` in the settings directory, however it is
    /// opened (docs/files.md).
    fn settings_template(&self, path: &Path) -> Option<String> {
        let dir = self.config_dir.as_ref()?;
        let path = match path.is_absolute() {
            true => path.to_path_buf(),
            false => std::env::current_dir().ok()?.join(path),
        };
        if path.exists() {
            return None;
        }
        if path == dir.join("config.toml") {
            return Some(CONFIG_TEMPLATE.to_string());
        }
        let name = path.file_stem()?.to_str()?;
        let in_plugins = path.parent() == Some(dir.join("plugins").as_path());
        (in_plugins && path.extension()? == "toml").then(|| {
            let example = self.settings_examples.get(name).map(String::as_str);
            plugin_template(name, example)
        })
    }

    /// Opens `path` without showing it, or finds the buffer that has it.
    pub fn open_buffer(&mut self, path: impl Into<PathBuf>) -> Result<usize, Error> {
        let path = path.into();
        if let Some(open) = self.find_open(&path) {
            return Ok(open);
        }
        let buffer = self.load(path)?;
        self.buffers.push(buffer);
        let index = self.buffers.len() - 1;
        self.push_event(None, Event::BufferOpened(index));
        Ok(index)
    }

    /// Opens `path` in the view. The initial empty buffer is replaced if it
    /// was never touched.
    pub fn open(&mut self, path: impl Into<PathBuf>) -> Result<(), Error> {
        let path = path.into();
        if let Some(open) = self.find_open(&path) {
            self.switch_to(open);
            return Ok(());
        }
        let buffer = self.load(path)?;
        // An empty scratch buffer, when it is the only one open, gives way
        // to the file.
        let mut open = self
            .buffers
            .iter()
            .enumerate()
            .filter(|(_, b)| !b.is_closed());
        let only = match (open.next(), open.next()) {
            (Some((index, b)), None) if b.path().is_none() && b.is_empty() && !b.is_modified() => {
                Some(index)
            }
            _ => None,
        };
        if let Some(index) = only {
            self.buffers[index] = buffer;
            self.view = View::new(index);
            self.push_event(None, Event::BufferOpened(index));
        } else {
            self.buffers.push(buffer);
            self.switch_to(self.buffers.len() - 1);
            self.push_event(None, Event::BufferOpened(self.buffers.len() - 1));
        }
        Ok(())
    }

    /// Gives buffers without a language the one for their file type, and
    /// parses the others again, since they may inject the new languages.
    pub fn attach_syntax(&mut self) {
        for buffer in &mut self.buffers {
            match &mut buffer.syntax {
                Some(syntax) => syntax.invalidate(),
                None => {
                    buffer.syntax = buffer
                        .path()
                        .and_then(|p| self.languages.for_path(p))
                        .map(BufferSyntax::new);
                }
            }
        }
    }

    /// Parses the shown buffers if they changed since the last parse, and
    /// the languages injected into them on the screen. Hidden buffers wait
    /// until they are shown. Returns whether the colors may have changed.
    pub fn update_syntax(&mut self) -> bool {
        // Trees the syntax thread finished, for hidden buffers too.
        let (mut fresh, layers) = self.take_parses(None);
        let mut changed = layers || !fresh.is_empty();
        for index in self.shown_buffers() {
            if self.languages.in_background() {
                self.start_parse(index);
            } else if self.parse(index) {
                fresh.push(index);
                changed = true;
            }
            let parsed = fresh.contains(&index);
            let screen = self.screen_of(index, 0);
            let buffer = &mut self.buffers[index];
            let text = buffer.text().clone();
            let Some(syntax) = &mut buffer.syntax else {
                continue;
            };
            // A file just opened shows the colors of its own language
            // first; its injections wait for the next call.
            if parsed && syntax.injections_pending() {
                continue;
            }
            changed |= self.languages.update_injections(syntax, &text, &screen);
        }
        changed
    }

    /// Parses the injected languages a screen above and below the shown
    /// ones, so scrolling finds them parsed, and forgets those far away,
    /// which would cost time on every edit.
    pub fn prefetch_injections(&mut self) {
        for index in self.shown_buffers() {
            let (near, keep) = (self.screen_of(index, 1), self.screen_of(index, 4));
            let buffer = &mut self.buffers[index];
            let text = buffer.text().clone();
            if let Some(syntax) = &mut buffer.syntax {
                self.languages
                    .prefetch_injections(syntax, &text, &near, &keep);
            }
        }
    }

    fn shown_buffers(&self) -> Vec<usize> {
        let mut shown: Vec<usize> = self.others.iter().map(|(_, v)| v.buffer).collect();
        shown.push(self.view.buffer);
        shown.sort_unstable();
        shown.dedup();
        shown
    }

    /// The text of buffer `index` in its views, and `margin` screens above
    /// and below each.
    fn screen_of(&self, index: usize, margin: usize) -> Vec<std::ops::Range<usize>> {
        let text = self.buffers[index].text();
        let rows = self.height as usize;
        std::iter::once(&self.view)
            .chain(self.others.iter().map(|(_, v)| v))
            .filter(|v| v.buffer == index)
            .map(|v| {
                let line = |n: usize| text.line_to_byte(n.min(text.len_lines()));
                line(v.top_line.saturating_sub(margin * rows))
                    ..line(v.top_line + (margin + 1) * rows)
            })
            .collect()
    }

    /// Parses buffer `index` if it changed since the last parse, loading its
    /// grammar the first time. Returns whether it parsed.
    fn parse(&mut self, index: usize) -> bool {
        let buffer = &mut self.buffers[index];
        let text = buffer.text().clone();
        let Some(syntax) = buffer.syntax.as_mut().filter(|s| s.dirty) else {
            return false;
        };
        if let Err(err) = self.languages.parse(syntax, &text) {
            // Shown without highlighting from now on.
            buffer.syntax = None;
            self.message = Some(format!("syntax: {err}"));
            return true;
        }
        self.syntax_updated(index);
        true
    }

    /// Asks the syntax thread to parse buffer `index` if it changed.
    fn start_parse(&mut self, index: usize) {
        let buffer = &mut self.buffers[index];
        let text = buffer.text().clone();
        let Some(syntax) = &mut buffer.syntax else {
            return;
        };
        if let Err(err) = self.languages.start_parse(syntax, &text) {
            buffer.syntax = None;
            self.message = Some(format!("syntax: {err}"));
        }
    }

    /// Takes the trees the syntax thread finished, after waiting for `job`
    /// if given. Returns the buffers whose own trees are now up to date,
    /// and whether trees of injected layers changed colors.
    fn take_parses(&mut self, job: Option<u64>) -> (Vec<usize>, bool) {
        let mut fresh = Vec::new();
        let mut layers = false;
        for done in self.languages.finished(job) {
            let id = done.id();
            let own = |b: &Buffer| b.syntax.as_ref().is_some_and(|s| s.job() == Some(id));
            let any = |b: &Buffer| b.syntax.as_ref().is_some_and(|s| s.jobs().contains(&id));
            // Parses of buffers since closed or parsed again are dropped.
            let Some(index) = self.buffers.iter().position(any) else {
                continue;
            };
            let is_own = own(&self.buffers[index]);
            let buffer = &mut self.buffers[index];
            let text = buffer.text().clone();
            let syntax = buffer.syntax.as_mut().expect("found by its job");
            if !is_own {
                layers |= self.languages.take_layer_parse(syntax, done, &text);
                continue;
            }
            match self.languages.take_parse(syntax, done, &text) {
                Ok(true) => fresh.push(index),
                Ok(false) => {}
                Err(err) => {
                    buffer.syntax = None;
                    self.message = Some(format!("syntax: {err}"));
                }
            }
        }
        for &index in &fresh {
            self.syntax_updated(index);
        }
        (fresh, layers)
    }

    /// Waits until the syntax thread has parsed every shown buffer up to
    /// its text, and its injected layers near the screen, sending the parses
    /// still needed.
    pub fn wait_for_parses(&mut self) {
        if !self.languages.in_background() {
            return;
        }
        loop {
            let under_way = self
                .buffers
                .iter()
                .find_map(|b| b.syntax.as_ref().and_then(|s| s.jobs().first().copied()));
            if let Some(job) = under_way {
                self.take_parses(Some(job));
                // No tree came: the thread is gone. Parse on this one.
                for syntax in self.buffers.iter_mut().filter_map(|b| b.syntax.as_mut()) {
                    if syntax.jobs().contains(&job) {
                        syntax.forget_job();
                        self.languages.set_background(None);
                    }
                }
                continue;
            }
            for index in self.shown_buffers() {
                self.start_parse(index);
            }
            let started = self
                .buffers
                .iter()
                .any(|b| b.syntax.as_ref().is_some_and(|s| !s.jobs().is_empty()));
            if !started {
                return;
            }
        }
    }

    /// Tells plugins that buffer `index` has an up-to-date tree, so what
    /// they read from it on every key can wait for this instead of for a
    /// parse (docs/plugin-api.md).
    fn syntax_updated(&mut self, index: usize) {
        let version = self.buffers[index].version();
        self.push_event(
            None,
            Event::SyntaxUpdated {
                buffer: index,
                version,
            },
        );
    }

    /// Runs `f` with the up-to-date syntax tree of buffer `index`, if it has
    /// one, and the id of its language.
    /// Calls `f` with the syntax of buffer `index`, up to date with its
    /// text, injected layers too: parses under way are likely nearly done,
    /// and the buffer's own tree is parsed here only if the text changed
    /// again meanwhile. `None` for a buffer without a language or a tree.
    pub(crate) fn with_syntax<R>(
        &mut self,
        index: usize,
        f: impl FnOnce(&mut Languages, &BufferSyntax, &Rope) -> R,
    ) -> Option<R> {
        while let Some(job) = self.buffers[index]
            .syntax
            .as_ref()
            .and_then(|s| s.jobs().first().copied())
        {
            self.take_parses(Some(job));
            // A thread gone keeps its jobs; parse on this one instead.
            if let Some(syntax) = self.buffers[index].syntax.as_mut()
                && syntax.jobs().contains(&job)
            {
                syntax.forget_job();
                self.languages.set_background(None);
            }
        }
        self.parse(index);
        let buffer = &self.buffers[index];
        let syntax = buffer.syntax.as_ref().filter(|s| s.tree.is_some())?;
        Some(f(&mut self.languages, syntax, buffer.text()))
    }

    /// Highlight styles for the bytes in `range` of the shown buffer, if it
    /// has a parsed syntax tree.
    pub fn syntax_styles(
        &self,
        index: usize,
        range: std::ops::Range<usize>,
    ) -> Option<Vec<Option<crate::grid::Style>>> {
        let buffer = &self.buffers[index];
        let syntax = buffer.syntax.as_ref()?;
        syntax.tree.as_ref()?;
        Some(
            self.languages
                .highlight(&self.theme, syntax, buffer.text(), range),
        )
    }

    /// Shows buffer `index` in the focused view for a plugin. The empty
    /// buffer nib starts with gives way, as when opening a file.
    pub fn show(&mut self, index: usize) {
        let old = self.view.buffer;
        self.switch_to(index);
        let buffer = &self.buffers[old];
        let untouched = buffer.path().is_none() && buffer.is_empty() && !buffer.is_modified();
        let shown = self.others.iter().any(|(_, view)| view.buffer == old);
        if old != index && untouched && !shown {
            let _ = self.close_buffer_at(old, false);
        }
    }

    /// Shows buffer `index`, keeping the view of the current one for later.
    pub fn switch_to(&mut self, index: usize) {
        if index == self.view.buffer {
            return;
        }
        let view = self
            .hidden_views
            .remove(&index)
            .unwrap_or_else(|| View::new(index));
        let old = std::mem::replace(&mut self.view, view);
        self.hidden_views.insert(old.buffer, old);
    }

    /// Rows left for text above the panels and the status line.
    pub fn text_area_rows(&self) -> u16 {
        let status = u16::from(self.height > 1);
        // A prompt in a box is over the text, not below it.
        let prompt = self.prompts.last().is_some_and(|p| p.boxed.is_none());
        let panels: usize =
            self.panels.iter().map(|p| p.lines.len()).sum::<usize>() + usize::from(prompt);
        self.height
            .saturating_sub(status)
            .saturating_sub(panels.min(u16::MAX as usize) as u16)
    }

    /// Where the focused view is, in the rows for text that `rows` leaves.
    pub fn focused_rect(&self, rows: u16) -> Rect {
        let area = Rect {
            x: 0,
            y: 0,
            width: self.width,
            height: rows,
        };
        let (rects, _) = self.splits.layout(area);
        rects
            .into_iter()
            .find(|(id, _)| *id == self.focused)
            .map_or(area, |(_, rect)| rect)
    }

    /// The rows of the focused view.
    pub fn text_rows(&self) -> u16 {
        self.focused_rect(self.text_area_rows()).height
    }

    /// Where each view goes in the rows for text, and the lines between.
    pub fn view_layout(&self, rows: u16) -> (Vec<(u32, Rect)>, Vec<Separator>) {
        self.splits.layout(Rect {
            x: 0,
            y: 0,
            width: self.width,
            height: rows,
        })
    }

    pub fn view_by_id(&self, id: u32) -> &View {
        if id == self.focused {
            return &self.view;
        }
        let (_, view) = self
            .others
            .iter()
            .find(|(other, _)| *other == id)
            .expect("views in the tree exist");
        view
    }

    /// Makes view `id` the one taking keys.
    pub fn focus(&mut self, id: u32) {
        let Some(at) = self.others.iter().position(|(other, _)| *other == id) else {
            return;
        };
        let (_, view) = self.others.remove(at);
        let old = std::mem::replace(&mut self.view, view);
        self.others.push((self.focused, old));
        self.focused = id;
    }

    /// Splits the focused view in two, and focuses the new half.
    pub fn split(&mut self, side_by_side: bool) {
        self.last_view_id += 1;
        let new = self.last_view_id;
        self.splits.split(self.focused, new, side_by_side);
        self.others.push((self.focused, self.view.clone()));
        self.focused = new;
    }

    /// Closes the focused view, focusing the one before it.
    /// Whether buffer `index` is a file in the settings directory.
    fn is_config(&self, index: usize) -> bool {
        let (Some(dir), Some(path)) = (&self.config_dir, self.buffers[index].path()) else {
            return false;
        };
        let canonical = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        canonical(path).starts_with(canonical(dir))
    }

    /// The command `[core] open-directory` names, with the arguments that
    /// open `dir` with it (docs/files.md).
    pub(crate) fn directory_command(&self, dir: &Path) -> (String, String) {
        let dir = match dir.is_absolute() {
            true => dir.to_path_buf(),
            false => std::env::current_dir().unwrap_or_default().join(dir),
        };
        let args = serde_json::json!({ "path": dir.display().to_string() });
        (self.settings.open_directory.clone(), args.to_string())
    }

    /// The command a core command stands for, if it stands for one:
    /// `config.open-directory` opens the settings directory with
    /// `[core] open-directory`'s command.
    pub(crate) fn redirect(&self, name: &str) -> Option<Result<(String, String), String>> {
        (name == "config.open-directory").then(|| match &self.config_dir {
            _ if self.settings.open_directory == name => {
                Err(format!("open-directory cannot be {name} itself"))
            }
            Some(dir) => Ok(self.directory_command(dir)),
            None => Err("nib does not know where the settings are".into()),
        })
    }

    /// Opens config.toml, or `plugins/<plugin>.toml`. A missing one opens
    /// with the defaults commented out, unsaved, so it exists once saved.
    pub fn open_config(&mut self, plugin: Option<&str>) -> Result<(), String> {
        let dir = self
            .config_dir
            .clone()
            .ok_or("nib does not know where the settings are")?;
        let path = match plugin {
            None => dir.join("config.toml"),
            Some(name) => dir.join("plugins").join(format!("{name}.toml")),
        };
        self.open(&path)
            .map_err(|err| format!("{}: {err}", path.display()))
    }

    /// The open buffer after `from`, or before it, going round. `from`
    /// itself if it is the only one.
    fn next_open(&self, from: usize, forward: bool) -> usize {
        let count = self.buffers.len();
        let step = if forward { 1 } else { count - 1 };
        let mut at = from;
        for _ in 0..count {
            at = (at + step) % count;
            if !self.buffers[at].is_closed() {
                return at;
            }
        }
        from
    }

    /// Shows the open buffer after the shown one, or before it.
    pub fn show_next(&mut self, forward: bool) {
        let next = self.next_open(self.view.buffer, forward);
        self.switch_to(next);
    }

    /// Saves buffer `index`, or with `path`, saves it there and makes that
    /// its path.
    pub fn save_buffer(&mut self, index: usize, path: Option<&str>) -> Result<(), String> {
        let buffer = &mut self.buffers[index];
        let saved = match path {
            Some(path) => buffer.save_as(path),
            None => buffer.save(),
        };
        saved.map_err(|err| err.to_string())?;
        self.push_event(None, Event::BufferSaved(index));
        if self.is_config(index) {
            self.reload_config = true;
        }
        Ok(())
    }

    /// Quits, unless buffers have unsaved changes and it is not `force`d.
    pub fn quit(&mut self, force: bool) -> Result<(), String> {
        let modified = self.modified_buffers();
        if modified > 0 && !force {
            let buffers = if modified == 1 {
                "buffer has"
            } else {
                "buffers have"
            };
            return Err(format!("{modified} {buffers} unsaved changes"));
        }
        self.quit = true;
        Ok(())
    }

    pub fn open_menu(&mut self) {
        self.menu = Some(Menu::Main);
        self.menu_cursor = 0;
        self.menu_input.clear();
    }

    /// Moves the focus to the next view, or the one on a side.
    pub fn focus_toward(&mut self, toward: Toward) {
        let target = if toward == Toward::Next {
            let order = self.splits.leaves();
            let at = order.iter().position(|id| *id == self.focused).unwrap_or(0);
            Some(order[(at + 1) % order.len()])
        } else {
            let direction = match toward {
                Toward::Left => Direction::Left,
                Toward::Right => Direction::Right,
                Toward::Up => Direction::Up,
                _ => Direction::Down,
            };
            let (rects, _) = self.view_layout(self.text_area_rows());
            windows::neighbor(&rects, self.focused, direction)
        };
        if let Some(id) = target {
            self.focus(id);
        }
    }

    /// Closes the shown buffer; see `close_buffer_at`.
    pub fn close_buffer(&mut self, force: bool) -> Result<(), String> {
        self.close_buffer_at(self.view.buffer, force)
    }

    /// Closes buffer `index`. Views showing it show another open one, or a
    /// new empty buffer if it was the last.
    pub fn close_buffer_at(&mut self, index: usize, force: bool) -> Result<(), String> {
        let buffer = &self.buffers[index];
        if buffer.is_modified() && !force {
            let name = buffer.name();
            return Err(format!(
                "{name} has unsaved changes; {{\"force\": true}} drops them"
            ));
        }
        let path = buffer.path().map(|p| p.to_string_lossy().into_owned());
        let name = buffer.name();
        let previous = self.next_open(index, false);
        let shown = if previous == index {
            self.buffers.push(Buffer::default());
            self.buffers.len() - 1
        } else {
            previous
        };
        if self.view.buffer == index {
            self.switch_to(shown);
        }
        for (_, view) in &mut self.others {
            if view.buffer == index {
                *view = View::new(shown);
            }
        }
        self.hidden_views.remove(&index);
        // Events of the buffer not yet delivered would hand plugins a
        // buffer that no longer exists.
        self.flush_changes();
        self.events.retain(|(_, event)| match event {
            Event::BufferOpened(b) | Event::BufferSaved(b) => *b != index,
            Event::BufferChanged { buffer, .. } | Event::SyntaxUpdated { buffer, .. } => {
                *buffer != index
            }
            _ => true,
        });
        self.buffers[index] = Buffer::closed();
        self.push_event(None, Event::BufferClosed { path, name });
        Ok(())
    }

    pub fn close_view(&mut self) -> Result<(), String> {
        let order = self.splits.leaves();
        if order.len() == 1 {
            return Err("the last view cannot be closed".into());
        }
        let at = order.iter().position(|id| *id == self.focused).unwrap_or(0);
        let next = if at == 0 { order[1] } else { order[at - 1] };
        let closed = self.focused;
        self.focus(next);
        self.others.retain(|(id, _)| *id != closed);
        self.splits.remove(closed);
        Ok(())
    }

    /// Closes every view but the focused one.
    pub fn only_view(&mut self) {
        self.others.clear();
        self.splits = Splits::Leaf(self.focused);
    }

    /// Puts pasted text into the open prompt, or before each selection of
    /// the shown buffer, as a new undo step.
    fn paste(&mut self, text: &str) {
        if let Some(prompt) = self.prompts.last_mut() {
            prompt.paste(text);
            let event = Event::PromptChanged {
                prompt: prompt.id,
                text: prompt.text.clone(),
                cursor: prompt.cursor,
            };
            let owner = prompt.owner;
            self.push_event(Some(owner), event);
            return;
        }
        let index = self.view.buffer;
        if !self.buffers[index].editable() {
            self.message = Some(format!("{} is read-only", self.buffers[index].name()));
            return;
        }
        let edits = self
            .view
            .selection
            .ranges()
            .iter()
            .map(|range| Edit::insert(range.from(), text))
            .collect();
        let buffer = &mut self.buffers[index];
        let version = buffer.version();
        if let Ok(change) = buffer.apply(
            version,
            edits,
            &self.view.selection,
            None,
            UndoMode::NewStep,
        ) {
            self.sync_views(index, &change.changes);
            self.view.selection = change.selection;
        }
    }

    /// Maps the selections of other views on buffer `index`, shown or
    /// hidden, through `changes` the focused view made.
    pub fn sync_views(&mut self, index: usize, changes: &[crate::ChangeSet]) {
        if changes.is_empty() {
            return;
        }
        let text = self.buffers[index].text();
        let views = self
            .others
            .iter_mut()
            .map(|(_, view)| view)
            .chain(self.hidden_views.get_mut(&index))
            .filter(|view| view.buffer == index);
        for view in views {
            view.selection = view.selection.map_all(changes, text);
        }
    }

    /// From the start of the first line shown to the end of the last one.
    pub fn visible_range(&self) -> (usize, usize) {
        let buffer = &self.buffers[self.view.buffer];
        let top = self.view.top_line;
        let start = buffer.line_start(top).unwrap_or(buffer.len());
        let end = buffer
            .line_start(top + self.text_rows() as usize)
            .unwrap_or(buffer.len());
        (start, end)
    }

    /// Scrolls the view without moving the cursor. Returns the number of
    /// lines `amount` stands for, so a keymap can move the cursor as far.
    pub fn scroll(&mut self, amount: ScrollAmount) -> i32 {
        let rows = i32::from(self.text_rows().max(1));
        let lines = match amount {
            ScrollAmount::Lines(n) => n,
            ScrollAmount::HalfPage(n) => n.saturating_mul((rows / 2).max(1)),
            ScrollAmount::Page(n) => n.saturating_mul(rows),
        };
        let last = self.buffers[self.view.buffer]
            .line_count()
            .saturating_sub(1);
        let top = self.view.top_line as i64 + i64::from(lines);
        self.view.top_line = top.clamp(0, last as i64) as usize;
        lines
    }

    pub fn modified_buffers(&self) -> usize {
        self.buffers.iter().filter(|b| b.is_modified()).count()
    }

    /// Runs a core command. Arguments and the result are JSON.
    pub fn run_command(&mut self, name: &str, args: &str) -> Result<String, String> {
        let args: serde_json::Value = match args.trim() {
            "" => serde_json::Value::Null,
            args => serde_json::from_str(args)
                .map_err(|err| format!("{name}: invalid arguments: {err}"))?,
        };
        // The same as the functions plugins call, by name for keys and
        // command lines (docs/api-0.6.md).
        match name {
            "buffer.save" => self.save_buffer(self.view.buffer, args["path"].as_str())?,
            "config.open" => self.open_config(args["plugin"].as_str())?,
            "config.reload" => self.reload_config = true,
            "core.menu" => self.open_menu(),
            "buffer.open" => {
                let path = args["path"]
                    .as_str()
                    .ok_or(r#"buffer.open needs {"path": string}"#)?;
                self.open(path).map_err(|err| format!("{path}: {err}"))?;
            }
            "buffer.next" | "buffer.previous" => self.show_next(name == "buffer.next"),
            "buffer.close" => self.close_buffer(args["force"].as_bool() == Some(true))?,
            "view.split" => {
                let side_by_side = match args["direction"].as_str() {
                    Some("vertical") | None => true,
                    Some("horizontal") => false,
                    Some(other) => return Err(format!("view.split: no direction {other:?}")),
                };
                self.split(side_by_side);
            }
            "view.close" => self.close_view()?,
            "view.only" => self.only_view(),
            "view.focus" => {
                let toward = match args["to"].as_str().unwrap_or("next") {
                    "next" => Toward::Next,
                    "left" => Toward::Left,
                    "right" => Toward::Right,
                    "up" => Toward::Up,
                    "down" => Toward::Down,
                    other => return Err(format!("view.focus: no direction {other:?}")),
                };
                self.focus_toward(toward);
            }
            "editor.quit" => self.quit(args["force"].as_bool().unwrap_or(false))?,
            _ => return Err(format!("no command named {name}")),
        }
        Ok("null".into())
    }

    /// The tab width of buffer `index`.
    pub fn tab_width(&self, index: usize) -> u16 {
        self.buffers[index]
            .overrides
            .tab_width
            .map_or(self.settings.tab_width, |(_, width)| width)
    }

    /// The indentation of buffer `index`.
    pub fn indent(&self, index: usize) -> Indent {
        self.buffers[index]
            .overrides
            .indent
            .map_or(self.settings.indent, |(_, indent)| indent)
    }

    /// Sets the tab width of buffer `index` alone, or with `None` goes back
    /// to config.toml's value.
    pub fn set_tab_width(
        &mut self,
        index: usize,
        owner: PluginId,
        width: Option<u16>,
    ) -> Result<(), String> {
        if width.is_some_and(|w| !(1..=16).contains(&w)) {
            return Err("the tab width must be 1 to 16".into());
        }
        self.buffers[index].overrides.tab_width = width.map(|w| (owner, w));
        Ok(())
    }

    /// Sets the indentation of buffer `index` alone, or with `None` goes
    /// back to config.toml's value.
    pub fn set_indent(
        &mut self,
        index: usize,
        owner: PluginId,
        indent: Option<Indent>,
    ) -> Result<(), String> {
        if let Some(Indent::Spaces(n)) = indent
            && !(1..=16).contains(&n)
        {
            return Err("an indentation must be a tab or 1 to 16 spaces".into());
        }
        self.buffers[index].overrides.indent = indent.map(|i| (owner, i));
        Ok(())
    }

    /// Removes everything `plugin` put into the editor.
    pub fn remove_plugin_parts(&mut self, plugin: PluginId) {
        self.layers.retain(|&layer| layer != plugin);
        self.status.retain(|item| item.owner != plugin);
        self.panels.retain(|panel| panel.owner != plugin);
        self.popups.retain(|popup| popup.owner != plugin);
        self.prompts.retain(|prompt| prompt.owner != plugin);
        self.choices.retain(|choices| choices.owner != plugin);
        for buffer in &mut self.buffers {
            buffer.remove_decorations(plugin);
        }
        let owned: Vec<usize> = (0..self.buffers.len())
            .filter(|&i| !self.buffers[i].is_closed() && self.buffers[i].owner() == Some(plugin))
            .collect();
        for index in owned {
            let _ = self.close_buffer_at(index, true);
        }
        self.commands.retain(|command| command.owner != plugin);
        self.timers.retain(|timer| timer.owner != plugin);
        self.processes.remove_owner(plugin);
        self.files.remove_owner(plugin);
        self.events.retain(|(target, _)| *target != Some(plugin));
    }

    /// Queues an event behind the buffer changes made so far, so events
    /// arrive in the order things happened.
    pub fn push_event(&mut self, target: Option<PluginId>, event: Event) {
        self.flush_changes();
        self.events.push_back((target, event));
    }

    pub fn pop_event(&mut self) -> Option<(Option<PluginId>, Event)> {
        self.flush_changes();
        self.events.pop_front()
    }

    /// Turns the changes buffers logged into `buffer-changed` events.
    fn flush_changes(&mut self) {
        for (index, buffer) in self.buffers.iter_mut().enumerate() {
            for (version, changes) in buffer.change_log.drain(..) {
                self.events.push_back((
                    None,
                    Event::BufferChanged {
                        buffer: index,
                        version,
                        changes,
                    },
                ));
            }
        }
    }

    /// The timers that are due, earliest first, removed from the list.
    pub fn take_due_timers(&mut self, now: Instant) -> Vec<Timer> {
        let (mut due, waiting): (Vec<Timer>, Vec<Timer>) =
            self.timers.drain(..).partition(|timer| timer.due <= now);
        self.timers = waiting;
        due.sort_by_key(|timer| (timer.due, timer.id));
        due
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollAmount {
    Lines(i32),
    HalfPage(i32),
    Page(i32),
}

/// The core menu, opened with the reserved menu key. It is drawn and
/// handled by the core alone, so it works however broken the plugins are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menu {
    /// Lists the plugins.
    Main,
    /// Actions on one plugin.
    Plugin(PluginId),
    /// Quitting would drop unsaved changes.
    ConfirmQuit,
    /// A newer release of the plugin wants more capabilities.
    ConfirmUpdate(PluginId),
    /// What to install, typed into `menu_input`.
    AddPlugin,
    /// A plugin fetched, waiting for a yes.
    ConfirmInstall,
    /// Removing the plugin, waiting for a yes.
    ConfirmRemove(PluginId),
    /// The first start: which base to use, `menu_cursor` among the bases.
    ChooseBase,
}

pub struct Editor {
    /// `None` only while lent to a plugin, when nothing else can reach the
    /// editor.
    pub(crate) state: Option<State>,
    pub(crate) plugins: Plugins,
    /// From `plugins/<name>.toml`, by plugin name.
    plugin_configs: BTreeMap<String, PluginConfig>,
    /// From the frontend, for installing, updating, and removing plugins
    /// in the core menu.
    pub(crate) store: Option<Arc<dyn PluginStore>>,
    /// A newer release that wants more capabilities, waiting for a yes.
    pub(crate) pending_update: Option<(PluginId, Box<dyn PendingUpdate>)>,
    /// A plugin fetched for installing, waiting for a yes.
    pub(crate) pending_install: Option<Box<dyn PendingInstall>>,
}

const LENT: &str = "editor state is only lent during plugin calls";

/// The commands the core runs itself, with their descriptions.
pub(crate) const CORE_COMMANDS: &[(&str, &str)] = &[
    (
        "buffer.save",
        "Save the current buffer, or as {\"path\": string}",
    ),
    ("buffer.open", "Open a file: {\"path\": string}"),
    ("buffer.next", "Show the next buffer"),
    (
        "config.open",
        "Open config.toml, or plugins/<name>.toml with {\"plugin\": name}",
    ),
    ("config.reload", "Read the settings again"),
    (
        "config.open-directory",
        "Open the settings directory, as [core] open-directory opens one",
    ),
    ("core.menu", "Open the core menu"),
    (
        "buffer.close",
        "Close the shown buffer; {\"force\": true} drops unsaved changes",
    ),
    ("buffer.previous", "Show the previous buffer"),
    (
        "view.split",
        "Split the view: {\"direction\": \"vertical\" or \"horizontal\"}",
    ),
    ("view.close", "Close the focused view"),
    ("view.only", "Close every view but the focused one"),
    (
        "view.focus",
        "Focus another view: {\"to\": \"next\", \"left\", \"right\", \"up\", or \"down\"}",
    ),
    (
        "editor.quit",
        "Quit; {\"force\": true} drops unsaved changes",
    ),
];

impl Default for Editor {
    /// Starts with an empty buffer that has no path.
    fn default() -> Self {
        let inbox = Arc::new(Inbox::default());
        Self {
            state: Some(State {
                buffers: vec![Buffer::default()],
                view: View::new(0),
                focused: 1,
                others: Vec::new(),
                splits: Splits::Leaf(1),
                last_view_id: 1,
                width: 0,
                height: 0,
                quit: false,
                layers: Vec::new(),
                message: None,
                menu: None,
                settings: Settings::default(),
                status: Vec::new(),
                panels: Vec::new(),
                last_panel_id: 0,
                popups: Vec::new(),
                last_popup_id: 0,
                prompts: Vec::new(),
                choices: Vec::new(),
                last_prompt_id: 0,
                choices_acted: false,
                leader_keys: Vec::new(),
                events: VecDeque::new(),
                commands: Vec::new(),
                timers: Vec::new(),
                last_timer_id: 0,
                processes: Processes::new(inbox.clone()),
                files: FileJobs::new(inbox.clone()),
                clipboard: Box::new(clipboard::Internal::default()),
                inbox,
                menu_cursor: 0,
                config_dir: None,
                settings_examples: BTreeMap::new(),
                preview: None,
                reload_config: false,
                menu_input: String::new(),
                hidden_views: HashMap::new(),
                languages: Languages::default(),
                theme: Theme::default(),
                mode: Mode::default(),
            }),
            plugins: Plugins::default(),
            plugin_configs: BTreeMap::new(),
            store: None,
            pending_update: None,
            pending_install: None,
        }
    }
}

impl Editor {
    pub(crate) fn state(&self) -> &State {
        self.state.as_ref().expect(LENT)
    }

    pub(crate) fn state_mut(&mut self) -> &mut State {
        self.state.as_mut().expect(LENT)
    }

    /// Applies config.toml. Call before loading plugins, which get their
    /// tables from it.
    pub fn apply_config(&mut self, config: Config) {
        let options = &mut self.plugins.options;
        options.call_timeout = config.core.plugin_timeout;
        options.init_timeout = config.core.plugin_init_timeout;
        options.memory_limit = config.core.plugin_memory;
        self.state_mut().settings = config.core;
        let state = self.state_mut();
        state.theme = config.theme;
        // Kept colors come from the old theme.
        for syntax in state.buffers.iter_mut().filter_map(|b| b.syntax.as_mut()) {
            syntax.forget_colors();
        }
        self.plugin_configs = config.plugins;
    }

    pub fn settings(&self) -> &Settings {
        &self.state().settings
    }

    /// The plugin's `plugins/<name>.toml`, or the defaults.
    pub fn plugin_config(&self, name: &str) -> PluginConfig {
        self.plugin_configs.get(name).cloned().unwrap_or_default()
    }

    /// Opens `path` in the view. The initial empty buffer is replaced if it
    /// was never touched.
    pub fn open(&mut self, path: impl Into<PathBuf>) -> Result<(), Error> {
        self.state_mut().open(path)
    }

    /// Opens directory `dir` with the command `[core] open-directory` names
    /// (docs/files.md).
    pub fn open_directory(&mut self, dir: &Path) -> Result<(), String> {
        let (command, args) = self.state().directory_command(dir);
        self.call_command(&command, &args)
            .map(drop)
            .map_err(|err| format!("open-directory: {err}"))
    }

    /// Does work left for after a frame, such as the first parse of a
    /// buffer, so opening a file shows it before its highlighting. Returns
    /// whether the screen needs drawing again.
    /// Parses buffers on a thread of their own, as the terminal does, so
    /// keys and frames never wait for a tree (docs/architecture.md, "解析の
    /// スレッド"). Off by default: each key then leaves the trees up to
    /// date, as tests want.
    pub fn set_background_parsing(&mut self, on: bool) {
        let state = self.state_mut();
        let inbox = on.then(|| state.inbox.clone());
        state.languages.set_background(inbox);
        // Parses sent to a thread now gone never come back.
        for syntax in state.buffers.iter_mut().filter_map(|b| b.syntax.as_mut()) {
            syntax.forget_job();
        }
    }

    /// Forgets the highlight colors kept from the last frame, so the next
    /// paints everything again.
    pub fn repaint_all(&mut self) {
        for syntax in self
            .state_mut()
            .buffers
            .iter_mut()
            .filter_map(|b| b.syntax.as_mut())
        {
            syntax.forget_colors();
        }
    }

    /// Whether the syntax thread has parses of buffers under way.
    pub fn is_parsing(&self) -> bool {
        self.state()
            .buffers
            .iter()
            .any(|b| b.syntax.as_ref().is_some_and(|s| !s.jobs().is_empty()))
    }

    /// Waits for the syntax thread to parse the shown buffers, for tests.
    /// `catch_up` then takes the trees in as the frontend's loop would.
    pub fn wait_for_syntax(&mut self) {
        self.state_mut().wait_for_parses();
    }

    pub fn catch_up(&mut self) -> bool {
        let delivered = self.deliver_events();
        if delivered {
            self.scroll_to_cursor();
        }
        let parsed = self.state_mut().update_syntax();
        if !delivered && !parsed {
            // Off the screen, so nothing to draw again.
            self.state_mut().prefetch_injections();
        }
        delivered || parsed
    }

    pub fn resize(&mut self, width: u16, height: u16) {
        let state = self.state_mut();
        state.width = width;
        state.height = height;
        self.scroll_to_cursor();
    }

    pub fn size(&self) -> (u16, u16) {
        (self.state().width, self.state().height)
    }

    pub fn view(&self) -> &View {
        &self.state().view
    }

    pub fn view_mut(&mut self) -> &mut View {
        &mut self.state_mut().view
    }

    pub fn buffer(&self) -> &Buffer {
        let state = self.state();
        &state.buffers[state.view.buffer]
    }

    pub fn message(&self) -> Option<&str> {
        self.state().message.as_deref()
    }

    /// Shows `message` until the next key.
    pub fn show_message(&mut self, message: impl Into<String>) {
        self.state_mut().message = Some(message.into());
    }

    /// Sends the key down the input stack until a plugin handles it.
    ///
    /// The menu key never goes to plugins: it opens the core menu, so
    /// plugins can always be managed even if one swallows every key.
    pub fn handle_key(&mut self, key: KeyEvent) {
        self.state_mut().message = None;
        if let Some(menu) = self.state_mut().menu.take() {
            self.handle_menu_key(menu, key);
        } else if key == self.menu_key() {
            self.state_mut().open_menu();
        } else if self.state().prompts.is_empty() {
            self.send_to_plugins_offering(key);
        } else {
            self.prompt_key(key);
        }
        self.after_plugins_ran();
    }

    /// Sends pasted text down the input stack as one piece, or to the base
    /// while a prompt is open. When no plugin takes it, it goes into the
    /// prompt, or before each selection of the shown buffer.
    pub fn handle_paste(&mut self, text: &str) {
        self.state_mut().message = None;
        if self.state().menu.is_some() {
            return;
        }
        // Terminals send line breaks as they were copied, or as CR.
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let handled = if self.state().prompts.is_empty() {
            let layers = self.state().layers.clone();
            layers
                .into_iter()
                .rev()
                .any(|plugin| self.plugin_handle_paste(plugin, &text))
        } else {
            self.running_base()
                .is_some_and(|base| self.plugin_handle_paste(base, &text))
        };
        if !handled {
            self.state_mut().paste(&text);
        }
        self.after_plugins_ran();
    }

    /// Gives plugins the system clipboard. Without it, they get one inside
    /// the editor.
    pub fn set_clipboard(&mut self, clipboard: Box<dyn Clipboard>) {
        self.state_mut().clipboard = clipboard;
    }

    /// How the core menu installs, updates, and removes plugins. Without
    /// it, the menu offers none of that.
    pub fn set_plugin_store(&mut self, store: Option<Arc<dyn PluginStore>>) {
        self.store = store;
    }

    /// Where the settings are, for opening them from inside nib and reading
    /// them again when they are saved.
    pub fn set_config_dir(&mut self, dir: Option<PathBuf>) {
        self.state_mut().config_dir = dir;
    }

    pub(crate) fn reload_config_if_asked(&mut self) {
        if std::mem::take(&mut self.state_mut().reload_config) {
            let message = self.reload_config();
            self.state_mut().message = Some(message);
        }
    }

    /// Reads the settings again: what takes effect at once does, the
    /// plugins whose settings changed restart, and what waits for the next
    /// start is named. Broken settings are left for the ones in use.
    fn reload_config(&mut self) -> String {
        let Some(dir) = self.state().config_dir.clone() else {
            return "nib does not know where the settings are".into();
        };
        let config = match Config::load(&dir) {
            Ok(config) => config,
            Err(err) => return format!("{err}; the settings in use are kept"),
        };
        let old = std::mem::take(&mut self.plugin_configs);
        let mut restart = Vec::new();
        let mut later = Vec::new();
        for (id, plugin) in self.plugins().iter().enumerate() {
            let was = old.get(&plugin.name).cloned().unwrap_or_default();
            let now = config
                .plugins
                .get(&plugin.name)
                .cloned()
                .unwrap_or_default();
            if was.settings != now.settings && plugin.enabled {
                restart.push(id);
            }
            let settings_aside = |c: &PluginConfig| PluginConfig {
                settings: String::new(),
                ..c.clone()
            };
            if settings_aside(&was) != settings_aside(&now) {
                later.push(plugin.name.clone());
            }
        }
        self.apply_config(config);
        let mut message = String::from("settings reloaded");
        for id in restart {
            let name = self.plugins()[id].name.clone();
            match self.restart_plugin(id) {
                Ok(()) => message += &format!("; {name} restarted"),
                Err(err) => message += &format!("; {name}: {err}"),
            }
        }
        let base = self.settings().base.clone();
        if self.base_in_use() != Some(base.as_str()) {
            message += &match self.switch_base(&base) {
                Ok(()) => format!("; {base} is the base now"),
                Err(err) => format!("; {err}"),
            };
        }
        if !later.is_empty() {
            message += &format!(
                "; the rest of {}'s settings take effect when nib starts again",
                later.join(", ")
            );
        }
        message
    }

    /// Starts base `name` in place of the one in use.
    fn switch_base(&mut self, name: &str) -> Result<(), String> {
        let id = self
            .plugin_id(name)
            .filter(|&id| self.plugins()[id].base)
            .ok_or_else(|| format!("no base named {name}"))?;
        self.restart_plugin(id)
            .map_err(|err| format!("{name}: {err}"))
    }

    /// Whether plugin `id` can be updated from the core menu.
    pub(crate) fn can_update(&self, id: PluginId) -> bool {
        let name = &self.plugins()[id].name;
        self.store.as_ref().is_some_and(|s| s.can_update(name))
    }

    /// Whether plugin `id` can be removed from the core menu.
    pub(crate) fn can_remove(&self, id: PluginId) -> bool {
        let name = &self.plugins()[id].name;
        self.store.as_ref().is_some_and(|s| s.can_remove(name))
    }

    /// Whether plugins can be installed from the core menu.
    pub(crate) fn can_install(&self) -> bool {
        self.store.is_some()
    }

    /// Fetches the plugin `source` names on a thread; what it found comes
    /// back through the inbox.
    pub(crate) fn start_install(&mut self, source: String) {
        let Some(store) = self.store.clone() else {
            return;
        };
        let inbox = self.state().inbox.clone();
        self.state_mut().message = Some(format!("fetching {source}..."));
        std::thread::spawn(move || {
            let result = store.prepare_install(&source);
            inbox.push(Message::Install(Prepared { source, result }));
        });
    }

    /// Takes in a plugin fetched for installing, and asks before it goes in.
    fn finish_install(&mut self, prepared: Prepared) {
        let state = self.state_mut();
        match prepared.result {
            Err(err) => state.message = Some(format!("{}: {err}", prepared.source)),
            Ok(pending) => {
                self.pending_install = Some(pending);
                let state = self.state_mut();
                state.menu = Some(Menu::ConfirmInstall);
                state.message = None;
            }
        }
    }

    pub(crate) fn install(&mut self, pending: Box<dyn PendingInstall>) -> String {
        let (name, version) = (pending.name().to_string(), pending.version().to_string());
        let dir = match pending.apply() {
            Ok(dir) => dir,
            Err(err) => return format!("{name}: installing failed: {err}"),
        };
        match self.load_plugin(&dir) {
            Ok(()) => format!("{name} {version} installed"),
            Err(err) => format!("{name} installed, but loading it failed: {err}"),
        }
    }

    /// The plugin fetched for installing, for the menu to show.
    pub(crate) fn pending_install(&self) -> Option<&dyn PendingInstall> {
        self.pending_install.as_deref()
    }

    /// Checks for a newer release of plugin `id` on a thread; what it found
    /// comes back through the inbox.
    pub(crate) fn start_update(&mut self, id: PluginId) {
        let Some(updates) = self.store.clone() else {
            return;
        };
        let name = self.plugins()[id].name.clone();
        let inbox = self.state().inbox.clone();
        self.state_mut().message = Some(format!("checking {name} for a newer release..."));
        std::thread::spawn(move || {
            let result = updates.check(&name);
            inbox.push(Message::Update(Checked {
                plugin: id,
                name,
                result,
            }));
        });
    }

    /// Takes in a check for a newer release: puts it in and loads it again,
    /// or asks first if it wants more capabilities.
    fn finish_update(&mut self, checked: Checked) {
        let Checked {
            plugin,
            name,
            result,
        } = checked;
        let message = match result {
            Err(err) => format!("{name}: updating failed: {err}"),
            Ok(None) => format!("{name} is up to date"),
            Ok(Some(pending)) if !pending.added_capabilities().is_empty() => {
                self.pending_update = Some((plugin, pending));
                // The menu asks now, in place of "checking...".
                let state = self.state_mut();
                state.menu = Some(Menu::ConfirmUpdate(plugin));
                state.message = None;
                return;
            }
            Ok(Some(pending)) => self.apply_update(plugin, pending),
        };
        self.state_mut().message = Some(message);
    }

    pub(crate) fn apply_update(&mut self, id: PluginId, pending: Box<dyn PendingUpdate>) -> String {
        let name = self.plugins()[id].name.clone();
        let version = pending.version().to_string();
        if let Err(err) = pending.apply() {
            return format!("{name}: updating failed: {err}");
        }
        match self.reload_plugin(id) {
            Ok(()) => format!("{name} updated to {version}"),
            Err(err) => format!("{name} updated to {version}, but loading it failed: {err}"),
        }
    }

    /// The update waiting for a yes, for the menu to show.
    pub(crate) fn pending_update(&self) -> Option<&dyn PendingUpdate> {
        self.pending_update.as_ref().map(|(_, p)| p.as_ref())
    }

    /// Sets what background threads call after queueing work, such as a
    /// program's output, so the frontend can wake up and call
    /// `run_background`.
    pub fn set_waker(&mut self, waker: Option<Waker>) {
        self.state_mut().inbox.set_waker(waker);
    }

    /// Hands what background threads queued to the plugins. Returns whether
    /// there was anything.
    pub fn run_background(&mut self) -> bool {
        let messages = self.state().inbox.take();
        if messages.is_empty() {
            return false;
        }
        for message in messages {
            let state = self.state_mut();
            let (owner, event) = match message {
                Message::Output { id, stream, data } => (
                    state.processes.owner(id, false),
                    Event::ProcessOutput {
                        process: id,
                        stream,
                        data,
                    },
                ),
                Message::Exit { id, code } => (
                    state.processes.owner(id, true),
                    Event::ProcessExit { process: id, code },
                ),
                Message::Files { job, paths, done } => (
                    state.files.owner(job, done),
                    Event::FilesListed { job, paths, done },
                ),
                Message::Update(checked) => {
                    self.finish_update(checked);
                    continue;
                }
                Message::Install(prepared) => {
                    self.finish_install(prepared);
                    continue;
                }
            };
            // Messages of what was cancelled or stopped are dropped.
            if let Some(owner) = owner {
                state.push_event(Some(owner), event);
            }
        }
        self.after_plugins_ran();
        true
    }

    /// Delivers the events plugins caused, then brings the syntax tree and
    /// the scroll position up to date with what they did.
    pub(crate) fn after_plugins_ran(&mut self) {
        self.reload_config_if_asked();
        self.deliver_events();
        // Scrolled first: injected languages are parsed near the screen.
        self.scroll_to_cursor();
        self.state_mut().update_syntax();
    }

    /// Scrolls the view so the cursor stays `scroll_margin` lines away from
    /// the top and bottom edges where possible.
    fn scroll_to_cursor(&mut self) {
        let rect = self.state().focused_rect(self.text_rows());
        let rows = rect.height as usize;
        let state = self.state_mut();
        if rows == 0 {
            return;
        }
        let text = state.buffers[state.view.buffer].text();
        let cursor = state.view.cursor(text);
        let line = text.byte_to_line(cursor);
        let column = layout::column_of(text, cursor, state.tab_width(state.view.buffer));
        let width = u32::from(rect.width.max(1));
        let margin = (state.settings.scroll_margin as usize).min((rows - 1) / 2);
        let view = &mut state.view;
        if line < view.top_line + margin {
            view.top_line = line.saturating_sub(margin);
        } else if line + margin >= view.top_line + rows {
            view.top_line = line + margin + 1 - rows;
        }
        if column < view.left_col {
            view.left_col = column;
        } else if column >= view.left_col + width {
            view.left_col = column + 1 - width;
        }
    }

    /// While a prompt is open, keys go to the base, which edits it its own
    /// way; the core's defaults take the keys it leaves.
    fn prompt_key(&mut self, key: KeyEvent) {
        if let Some(base) = self.running_base()
            && self.plugin_handle_key(base, key)
        {
            return;
        }
        let state = self.state_mut();
        let Some(prompt) = state.prompts.last_mut() else {
            return;
        };
        let event = match prompt.default_key(key) {
            Some(Outcome::Edited) => Event::PromptChanged {
                prompt: prompt.id,
                text: prompt.text.clone(),
                cursor: prompt.cursor,
            },
            Some(Outcome::Asked(action)) => Event::PromptAction {
                prompt: prompt.id,
                action,
            },
            None => return,
        };
        let owner = prompt.owner;
        state.push_event(Some(owner), event);
    }

    /// Sends the key down the input stack. Choices the base did not act on
    /// hear `cancel` before whatever the key caused, as they no longer fit.
    fn send_to_plugins_offering(&mut self, key: KeyEvent) {
        let state = self.state_mut();
        let Some((id, owner)) = state.choices.last().map(|c| (c.id, c.owner)) else {
            return self.send_to_plugins(key);
        };
        state.flush_changes();
        let before = state.events.len();
        state.choices_acted = false;
        self.send_to_plugins(key);
        let state = self.state_mut();
        if !state.choices_acted && state.choices.iter().any(|c| c.id == id) {
            let cancel = Event::PromptAction {
                prompt: id,
                action: Action::Cancel,
            };
            state.events.insert(before, (Some(owner), cancel));
        }
    }

    fn send_to_plugins(&mut self, key: KeyEvent) {
        let layers = self.state().layers.clone();
        for plugin in layers.into_iter().rev() {
            if self.plugin_handle_key(plugin, key) {
                return;
            }
        }
    }

    pub fn menu(&self) -> Option<Menu> {
        self.state().menu
    }

    /// Shown while no plugin takes input, when the menu is the only thing
    /// that responds.
    pub fn key_hint(&self) -> Option<String> {
        self.state()
            .layers
            .is_empty()
            .then(|| format!("{}: menu", self.menu_key()))
    }

    pub fn modified_buffers(&self) -> usize {
        self.state().modified_buffers()
    }

    /// The bases loaded, in load order.
    pub(crate) fn bases(&self) -> Vec<String> {
        self.plugins()
            .into_iter()
            .filter(|p| p.base)
            .map(|p| p.name)
            .collect()
    }

    /// Asks which base to use, as on the first start, when there is more
    /// than one. The answer goes into a new config.toml.
    pub fn ask_for_base(&mut self) {
        let bases = self.bases();
        if bases.len() < 2 {
            return;
        }
        let current = self.base_in_use().map(str::to_string);
        let state = self.state_mut();
        state.menu_cursor = bases
            .iter()
            .position(|b| Some(b) == current.as_ref())
            .unwrap_or(0);
        state.menu_input.clear();
        state.menu = Some(Menu::ChooseBase);
    }

    /// Uses base `name`, or keeps the one in use for `None`, and writes
    /// the answer into a new config.toml.
    pub(crate) fn choose_base(&mut self, name: Option<String>) {
        let name = name.unwrap_or_else(|| self.base_in_use().unwrap_or_default().to_string());
        let switched = match self.base_in_use() == Some(name.as_str()) {
            true => Ok(()),
            false => self.switch_base(&name),
        };
        let message = match switched.and_then(|()| self.write_base(&name)) {
            Ok(()) => format!("{name} is the base; base in config.toml keeps it"),
            Err(err) => err,
        };
        self.state_mut().message = Some(message);
    }

    /// Writes a new config.toml that uses base `name`.
    fn write_base(&mut self, name: &str) -> Result<(), String> {
        let dir = self
            .state()
            .config_dir
            .clone()
            .ok_or("nib does not know where the settings are")?;
        let path = dir.join("config.toml");
        let text = CONFIG_TEMPLATE.replace("# base = \"helix\"", &format!("base = \"{name}\""));
        std::fs::create_dir_all(&dir)
            .and_then(|()| std::fs::write(&path, text))
            .map_err(|err| format!("{}: {err}", path.display()))
    }

    /// Saves every modified buffer. Returns what could not be saved.
    pub(crate) fn save_all(&mut self) -> Result<(), Vec<String>> {
        let mut failures = Vec::new();
        let state = self.state_mut();
        for index in 0..state.buffers.len() {
            let buffer = &mut state.buffers[index];
            if !buffer.is_modified() {
                continue;
            }
            let Some(path) = buffer.path().map(|path| path.display().to_string()) else {
                failures.push("[scratch] has no path".to_string());
                continue;
            };
            match buffer.save() {
                Ok(()) => state.push_event(None, Event::BufferSaved(index)),
                Err(err) => failures.push(format!("{path}: {err}")),
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures)
        }
    }

    pub fn should_quit(&self) -> bool {
        self.state().quit
    }

    #[cfg(test)]
    pub(crate) fn with_text(text: &str) -> Self {
        let mut editor = Self::default();
        editor.state_mut().buffers[0] = Buffer::with_text(text);
        editor
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::Edit;
    use crate::history::UndoMode;
    use crate::input::KeyCode;
    use crate::selection::Selection;

    fn press(editor: &mut Editor, keys: &[KeyEvent]) {
        for &key in keys {
            editor.handle_key(key);
        }
    }

    fn char_key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c))
    }

    /// Opens the menu and chooses the first row `typed` narrows it to.
    fn choose(editor: &mut Editor, typed: &str) {
        editor.handle_key(KeyEvent::ctrl('g'));
        for c in typed.chars() {
            editor.handle_key(char_key(c));
        }
        editor.handle_key(KeyEvent::new(KeyCode::Enter));
    }

    fn modify(editor: &mut Editor) {
        let buffer = &mut editor.state_mut().buffers[0];
        let version = buffer.version();
        buffer
            .apply(
                version,
                vec![Edit::insert(0, "x")],
                &Selection::point(0),
                None,
                UndoMode::NewStep,
            )
            .unwrap();
    }

    #[test]
    fn view_follows_the_cursor_with_a_margin() {
        let text: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let mut editor = Editor::with_text(&text);
        editor.resize(20, 10); // 9 text rows: the margin of 5 shrinks to 4
        let line_start = |editor: &Editor, line: usize| editor.buffer().line_start(line).unwrap();

        let pos = line_start(&editor, 50);
        editor.view_mut().selection = Selection::point(pos);
        press(&mut editor, &[KeyEvent::new(KeyCode::Escape)]);
        assert_eq!(editor.view().top_line, 46);

        let pos = line_start(&editor, 44);
        editor.view_mut().selection = Selection::point(pos);
        press(&mut editor, &[KeyEvent::new(KeyCode::Escape)]);
        assert_eq!(editor.view().top_line, 40);

        let pos = line_start(&editor, 1);
        editor.view_mut().selection = Selection::point(pos);
        press(&mut editor, &[KeyEvent::new(KeyCode::Escape)]);
        assert_eq!(editor.view().top_line, 0);
    }

    #[test]
    fn menu_key_comes_from_config() {
        let mut editor = Editor::default();
        editor.apply_config(Config::parse("[core]\nmenu-key = \"C-]\"").unwrap());
        assert_eq!(editor.key_hint().as_deref(), Some("Ctrl-]: menu"));
        press(&mut editor, &[KeyEvent::ctrl('g')]);
        assert_eq!(editor.menu(), None);
        press(&mut editor, &[KeyEvent::ctrl(']')]);
        assert_eq!(editor.menu(), Some(Menu::Main));
    }

    #[test]
    fn switching_buffers_keeps_each_view() {
        let dir = std::env::temp_dir();
        let a = dir.join(format!("nib-{}-a.txt", std::process::id()));
        let b = dir.join(format!("nib-{}-b.txt", std::process::id()));
        fs::write(&a, "aaa").unwrap();
        fs::write(&b, "bbb").unwrap();
        let mut editor = Editor::default();
        editor.open(&a).unwrap();
        editor.view_mut().selection = Selection::point(2);
        editor.open(&b).unwrap();
        assert_eq!(editor.buffer().text().to_string(), "bbb");
        assert_eq!(editor.view().selection, Selection::point(0));

        let state = editor.state_mut();
        state.run_command("buffer.previous", "").unwrap();
        assert_eq!(editor.buffer().text().to_string(), "aaa");
        assert_eq!(editor.view().selection, Selection::point(2));
        editor.state_mut().run_command("buffer.next", "").unwrap();
        assert_eq!(editor.buffer().text().to_string(), "bbb");

        // Opening a file that is open switches to it.
        editor.open(&a).unwrap();
        assert_eq!(editor.state().buffers.len(), 2);
        assert_eq!(editor.view().selection, Selection::point(2));
        fs::remove_file(&a).unwrap();
        fs::remove_file(&b).unwrap();
    }

    #[test]
    fn closing_buffers_shows_another() {
        let dir = std::env::temp_dir();
        let paths: Vec<_> = ["a", "b", "c"]
            .iter()
            .map(|n| dir.join(format!("nib-{}-close-{n}.txt", std::process::id())))
            .collect();
        for (path, text) in paths.iter().zip(["aaa", "bbb", "ccc"]) {
            fs::write(path, text).unwrap();
        }
        let mut editor = Editor::default();
        for path in &paths {
            editor.open(path).unwrap();
        }
        let text = |editor: &Editor| editor.buffer().text().to_string();
        let run = |editor: &mut Editor, name: &str, args: &str| {
            editor.state_mut().run_command(name, args)
        };
        // b, shown in a split too, is closed; both views show a.
        run(&mut editor, "buffer.previous", "").unwrap();
        run(&mut editor, "view.split", "").unwrap();
        run(&mut editor, "buffer.close", "").unwrap();
        assert_eq!(text(&editor), "aaa");
        assert!(editor.state().others.iter().all(|(_, v)| v.buffer == 0));
        // Going round skips it.
        run(&mut editor, "buffer.next", "").unwrap();
        assert_eq!(text(&editor), "ccc");
        run(&mut editor, "buffer.next", "").unwrap();
        assert_eq!(text(&editor), "aaa");

        // Unsaved changes stay unless forced.
        let buffer = &mut editor.state_mut().buffers[0];
        let version = buffer.version();
        let edit = vec![Edit::new(0, 0, "x")];
        buffer
            .apply(version, edit, &Selection::point(0), None, UndoMode::NewStep)
            .unwrap();
        let err = run(&mut editor, "buffer.close", "").unwrap_err();
        assert!(err.contains("unsaved changes"), "{err}");
        run(&mut editor, "buffer.close", r#"{"force": true}"#).unwrap();
        assert_eq!(text(&editor), "ccc");
        // Its change, not yet an event, is dropped with it: plugins would
        // get a handle to a buffer that is gone.
        let pending = editor.state().events.iter().any(|(_, event)| {
            matches!(
                event,
                Event::BufferChanged { buffer: 0, .. } | Event::BufferOpened(0)
            )
        });
        assert!(!pending);

        // The last one gives way to an empty buffer, which a file then
        // replaces.
        run(&mut editor, "buffer.close", "").unwrap();
        assert_eq!(text(&editor), "");
        assert!(editor.buffer().path().is_none());
        let open = editor
            .state()
            .buffers
            .iter()
            .filter(|b| !b.is_closed())
            .count();
        assert_eq!(open, 1);
        editor.open(&paths[0]).unwrap();
        assert_eq!(text(&editor), "aaa");
        for path in &paths {
            fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn scrolling_reports_lines_and_stops_at_the_ends() {
        let text: String = (0..50).map(|i| format!("{i}\n")).collect();
        let mut editor = Editor::with_text(&text);
        editor.resize(10, 11); // 10 text rows
        let state = editor.state_mut();
        assert_eq!(state.scroll(ScrollAmount::HalfPage(1)), 5);
        assert_eq!(state.view.top_line, 5);
        assert_eq!(state.scroll(ScrollAmount::Page(-1)), -10);
        assert_eq!(state.view.top_line, 0);
        state.scroll(ScrollAmount::Lines(100));
        assert_eq!(state.view.top_line, 50);
    }

    #[test]
    fn core_commands() {
        let mut editor = Editor::with_text("a");
        let state = editor.state_mut();
        assert_eq!(
            state.run_command("buffer.save", ""),
            Err("buffer has no path".into())
        );
        assert_eq!(
            state.run_command("nope", "{}"),
            Err("no command named nope".into())
        );
        assert!(state.run_command("buffer.open", "{}").is_err());

        modify(&mut editor);
        let state = editor.state_mut();
        assert_eq!(
            state.run_command("editor.quit", ""),
            Err("1 buffer has unsaved changes".into())
        );
        assert!(!state.quit);
        assert_eq!(
            state.run_command("editor.quit", r#"{"force":true}"#),
            Ok("null".into())
        );
        assert!(state.quit);
    }

    #[test]
    fn menu_quits_or_goes_back() {
        let mut editor = Editor::default();
        press(&mut editor, &[KeyEvent::ctrl('g')]);
        assert_eq!(editor.menu(), Some(Menu::Main));
        // Typing narrows the list; nothing else happens.
        press(
            &mut editor,
            &[char_key('q'), KeyEvent::new(KeyCode::Escape)],
        );
        assert_eq!(editor.menu(), None);
        assert!(!editor.should_quit());

        choose(&mut editor, "quit");
        assert!(editor.should_quit());
    }

    #[test]
    fn quitting_with_unsaved_changes_needs_confirmation() {
        let mut editor = Editor::with_text("a");
        modify(&mut editor);
        choose(&mut editor, "quit");
        assert_eq!(editor.menu(), Some(Menu::ConfirmQuit));
        assert!(!editor.should_quit());

        // Anything but "y" goes back, e.g. a "q" from a typo.
        press(&mut editor, &[char_key('q')]);
        assert_eq!(editor.menu(), None);
        assert!(!editor.should_quit());

        choose(&mut editor, "quit");
        press(&mut editor, &[char_key('y')]);
        assert!(editor.should_quit());
    }

    #[test]
    fn menu_saves_before_quitting() {
        let path = std::env::temp_dir().join(format!("nib-{}-menu.txt", std::process::id()));
        fs::write(&path, "a").unwrap();
        let mut editor = Editor::default();
        editor.open(&path).unwrap();
        modify(&mut editor);

        choose(&mut editor, "save all");
        let saved = fs::read_to_string(&path).unwrap();
        fs::remove_file(&path).unwrap();
        assert!(editor.should_quit());
        assert_eq!(saved, "xa");
    }

    #[test]
    fn menu_does_not_quit_when_saving_fails() {
        let mut editor = Editor::with_text("a");
        modify(&mut editor);
        choose(&mut editor, "save all");
        assert!(!editor.should_quit());
        assert_eq!(
            editor.message(),
            Some("not quitting: [scratch] has no path")
        );
    }
}
