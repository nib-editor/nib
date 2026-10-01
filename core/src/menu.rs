//! The core menu (docs/design/core/core-menu.md): a box in the middle of the screen
//! with the plugins and what can be done to them and to the editor,
//! narrowed by what is typed. The core draws it and takes its keys itself,
//! so it works when the base does not.

use crate::boxed::{Content, Frame, Input, Side, width as text_width};
use crate::editor::{Editor, Menu};
use crate::grid::{Cursor, Grid, display_width, graphemes};
use crate::input::{KeyCode, KeyEvent};
use crate::plugin::{PluginId, PluginInfo};
use crate::ui::{Span, StyledLine};

/// Something to choose in one of the menu's lists.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Item {
    Plugin(PluginId),
    Editor(EditorAction),
    ToPlugin(PluginId, PluginAction),
    Base(PluginId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditorAction {
    OpenConfig,
    OpenSettingsDirectory,
    ReloadSettings,
    AddPlugin,
    RestartAll,
    SaveAndQuit,
    Quit,
}

impl EditorAction {
    fn label(self) -> &'static str {
        match self {
            Self::OpenConfig => "Open config.toml",
            Self::OpenSettingsDirectory => "Open the settings directory",
            Self::ReloadSettings => "Reload the settings",
            Self::AddPlugin => "Add a plugin",
            Self::RestartAll => "Restart all plugins",
            Self::SaveAndQuit => "Save all and quit",
            Self::Quit => "Quit",
        }
    }

    fn about(self) -> &'static str {
        match self {
            Self::OpenConfig => "Open config.toml, nib's settings. Saving it applies them.",
            Self::OpenSettingsDirectory => {
                "Open the directory of config.toml and of plugins/, which holds a file of \
                 settings for each plugin."
            }
            Self::ReloadSettings => "Read config.toml and the plugins' settings again.",
            Self::AddPlugin => {
                "Install a plugin: by its name in the plugin list, from a GitHub \
                 repository, a URL, or a file."
            }
            Self::RestartAll => {
                "Stop every plugin and start it again, as when one has gone wrong. \
                 The buffers stay as they are."
            }
            Self::SaveAndQuit => {
                "Save every changed buffer, then quit. Stays when one cannot be saved."
            }
            Self::Quit => "Quit nib. With unsaved changes, it asks first.",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PluginAction {
    Restart,
    Toggle,
    Reload,
    Update,
    Remove,
    OpenSettings,
}

/// A row of a list: what choosing it does, and the text typing narrows by.
struct Row {
    item: Item,
    name: String,
}

impl Editor {
    /// Takes a key while the menu is open. The menu is closed meanwhile;
    /// what the key leads to opens it again.
    pub(crate) fn handle_menu_key(&mut self, menu: Menu, key: KeyEvent) {
        if key == self.menu_key() && menu != Menu::ChooseBase {
            return;
        }
        let yes = key == KeyEvent::new(KeyCode::Char('y'));
        match menu {
            Menu::Main | Menu::Plugin(_) | Menu::ChooseBase => self.list_key(menu, key),
            Menu::AddPlugin => {
                let state = self.state_mut();
                match key.code {
                    KeyCode::Char(c) if !key.modifiers.ctrl && !key.modifiers.alt => {
                        state.menu_input.push(c);
                    }
                    KeyCode::Backspace => {
                        state.menu_input.pop();
                    }
                    KeyCode::Escape => return self.open_list(Menu::Main, 0),
                    KeyCode::Enter if !state.menu_input.trim().is_empty() => {
                        let source = state.menu_input.trim().to_string();
                        self.start_install(source);
                        return;
                    }
                    _ => {}
                }
                self.state_mut().menu = Some(Menu::AddPlugin);
            }
            Menu::ConfirmInstall => {
                let Some(pending) = self.pending_install.take() else {
                    return;
                };
                let message = if yes {
                    self.install(pending)
                } else {
                    format!("{} not installed", pending.name())
                };
                self.state_mut().message = Some(message);
            }
            Menu::ConfirmUpdate(id) => {
                let Some((_, pending)) = self.pending_update.take() else {
                    return;
                };
                let name = self.plugins()[id].name.clone();
                let message = if yes {
                    self.apply_update(id, pending)
                } else {
                    format!("{name} left as it was")
                };
                self.state_mut().message = Some(message);
            }
            Menu::ConfirmRemove(id) if yes => {
                let name = self.plugins()[id].name.clone();
                let store = self.store.clone().expect("offered with a store");
                let message = match store.remove(&name) {
                    // Loaded, it stays, disabled, until nib starts again.
                    Ok(()) => {
                        self.disable_plugin(id);
                        format!("{name} removed; its settings and data are kept")
                    }
                    Err(err) => format!("{name}: removing failed: {err}"),
                };
                self.state_mut().message = Some(message);
            }
            Menu::ConfirmQuit if yes => self.state_mut().quit = true,
            // Any other key goes back, so a mistyped key is harmless.
            Menu::ConfirmRemove(_) | Menu::ConfirmQuit => {}
        }
    }

    /// Typing narrows the list, arrows move in it, Enter chooses, and Esc
    /// goes back.
    fn list_key(&mut self, menu: Menu, key: KeyEvent) {
        let count = self.shown(menu).len();
        let page = self.menu_frame().map_or(1, |f| f.body_rows().max(1)) as isize;
        let m = key.modifiers;
        let step = match key.code {
            KeyCode::Down => Some(1),
            KeyCode::Up => Some(-1),
            KeyCode::Tab if m.shift => Some(-1),
            KeyCode::Tab => Some(1),
            KeyCode::Char('n') if m.ctrl => Some(1),
            KeyCode::Char('p') if m.ctrl => Some(-1),
            KeyCode::PageDown => Some(page),
            KeyCode::PageUp => Some(-page),
            _ => None,
        };
        let state = self.state_mut();
        if let Some(step) = step {
            let last = count.saturating_sub(1) as isize;
            state.menu_cursor = (state.menu_cursor as isize + step).clamp(0, last) as usize;
            state.menu = Some(menu);
            return;
        }
        match key.code {
            KeyCode::Char('u') if m.ctrl => state.menu_input.clear(),
            KeyCode::Char(c) if !m.ctrl && !m.alt => state.menu_input.push(c),
            KeyCode::Backspace => {
                state.menu_input.pop();
            }
            KeyCode::Enter => return self.choose(menu),
            KeyCode::Escape => return self.back(menu),
            _ => {}
        }
        let state = self.state_mut();
        if matches!(key.code, KeyCode::Char(_) | KeyCode::Backspace) {
            state.menu_cursor = 0;
        }
        state.menu = Some(menu);
    }

    /// Opens a list with nothing typed, at row `cursor`.
    fn open_list(&mut self, menu: Menu, cursor: usize) {
        let state = self.state_mut();
        state.menu = Some(menu);
        state.menu_input.clear();
        state.menu_cursor = cursor;
    }

    fn back(&mut self, menu: Menu) {
        match menu {
            // The plugins come first, so its row is its id.
            Menu::Plugin(id) => self.open_list(Menu::Main, id),
            Menu::ChooseBase => self.choose_base(None),
            _ => {}
        }
    }

    fn choose(&mut self, menu: Menu) {
        let cursor = self.state().menu_cursor;
        let Some(row) = self.shown(menu).into_iter().nth(cursor) else {
            self.state_mut().menu = Some(menu);
            return;
        };
        match row.0.item {
            Item::Plugin(id) => self.open_list(Menu::Plugin(id), 0),
            Item::Editor(action) => self.do_to_editor(action),
            Item::ToPlugin(id, action) => self.do_to_plugin(id, action),
            Item::Base(id) => {
                let name = self.plugins()[id].name.clone();
                self.choose_base(Some(name));
            }
        }
    }

    fn do_to_editor(&mut self, action: EditorAction) {
        let result = match action {
            EditorAction::OpenConfig => self.call_command("config.open", "").map(drop),
            EditorAction::OpenSettingsDirectory => {
                self.call_command("config.open-directory", "").map(drop)
            }
            EditorAction::ReloadSettings => self.call_command("config.reload", "").map(drop),
            EditorAction::AddPlugin => {
                self.open_list(Menu::AddPlugin, 0);
                Ok(())
            }
            EditorAction::RestartAll => {
                self.restart_plugins();
                Ok(())
            }
            EditorAction::SaveAndQuit => match self.save_all() {
                Ok(()) => {
                    self.state_mut().quit = true;
                    Ok(())
                }
                Err(failures) => Err(format!("not quitting: {}", failures.join("; "))),
            },
            EditorAction::Quit => {
                if self.modified_buffers() == 0 {
                    self.state_mut().quit = true;
                } else {
                    self.state_mut().menu = Some(Menu::ConfirmQuit);
                }
                Ok(())
            }
        };
        if let Err(err) = result {
            self.state_mut().message = Some(err);
        }
    }

    fn do_to_plugin(&mut self, id: PluginId, action: PluginAction) {
        let plugin = &self.plugins()[id];
        let name = plugin.name.clone();
        let message = match action {
            PluginAction::Restart => match self.restart_plugin(id) {
                Ok(()) => format!("{name} restarted"),
                Err(err) => format!("{name}: restarting failed: {err}"),
            },
            PluginAction::Toggle if plugin.enabled => {
                self.disable_plugin(id);
                format!("{name} disabled")
            }
            PluginAction::Toggle if plugin.base => match self.restart_plugin(id) {
                Ok(()) => format!(
                    "{name} is the base now; base = \"{name}\" in config.toml's [core] keeps it"
                ),
                Err(err) => format!("{name}: starting failed: {err}"),
            },
            PluginAction::Toggle => match self.restart_plugin(id) {
                Ok(()) => format!("{name} enabled"),
                Err(err) => format!("{name}: enabling failed: {err}"),
            },
            PluginAction::Reload => match self.reload_plugin(id) {
                Ok(()) => format!("{name} reloaded"),
                Err(err) => format!("{name}: reloading failed: {err}"),
            },
            PluginAction::Update => return self.start_update(id),
            PluginAction::Remove => {
                self.state_mut().menu = Some(Menu::ConfirmRemove(id));
                return;
            }
            PluginAction::OpenSettings => {
                let args = serde_json::json!({ "plugin": name }).to_string();
                match self.call_command("config.open", &args) {
                    Ok(_) => return,
                    Err(err) => err,
                }
            }
        };
        self.state_mut().message = Some(message);
    }

    /// Every row of a list, before narrowing.
    fn rows(&self, menu: Menu) -> Vec<Row> {
        let plugins = self.plugins();
        let named = |id: PluginId, item| Row {
            item,
            name: plugins[id].name.clone(),
        };
        match menu {
            Menu::Main => {
                let mut rows: Vec<Row> = (0..plugins.len())
                    .map(|id| named(id, Item::Plugin(id)))
                    .collect();
                let actions = [
                    EditorAction::OpenConfig,
                    EditorAction::OpenSettingsDirectory,
                    EditorAction::ReloadSettings,
                    EditorAction::AddPlugin,
                    EditorAction::RestartAll,
                    EditorAction::SaveAndQuit,
                    EditorAction::Quit,
                ];
                rows.extend(
                    actions
                        .into_iter()
                        .filter(|&a| a != EditorAction::AddPlugin || self.can_install())
                        .map(|a| Row {
                            item: Item::Editor(a),
                            name: a.label().into(),
                        }),
                );
                rows
            }
            Menu::Plugin(id) => {
                let plugin = &plugins[id];
                [
                    (PluginAction::Restart, true),
                    (PluginAction::Toggle, true),
                    (PluginAction::Reload, plugin.reloadable),
                    (PluginAction::Update, self.can_update(id)),
                    (PluginAction::Remove, self.can_remove(id)),
                    (PluginAction::OpenSettings, true),
                ]
                .into_iter()
                .filter(|(_, offered)| *offered)
                .map(|(action, _)| Row {
                    item: Item::ToPlugin(id, action),
                    name: plugin_action_label(action, plugin).into(),
                })
                .collect()
            }
            Menu::ChooseBase => (0..plugins.len())
                .filter(|&id| plugins[id].base)
                .map(|id| named(id, Item::Base(id)))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The rows matching what is typed, with the chars of their names that
    /// matched: those starting with it first, then those containing it,
    /// then the rest, each in list order.
    fn shown(&self, menu: Menu) -> Vec<(Row, Vec<usize>)> {
        let typed = self.state().menu_input.to_lowercase();
        let mut shown: Vec<_> = self
            .rows(menu)
            .into_iter()
            .filter_map(|row| {
                let matched = matches(&row.name, &typed)?;
                Some((row, matched))
            })
            .collect();
        shown.sort_by_key(|(row, _)| {
            let name = row.name.to_lowercase();
            if name.starts_with(&typed) {
                0
            } else if name.contains(&typed) {
                1
            } else {
                2
            }
        });
        shown
    }

    fn menu_frame(&self) -> Option<Frame> {
        let (width, height) = self.size();
        Frame::new(width, height)
    }

    /// Draws the open menu over everything else. Returns the cursor, in
    /// its input line.
    pub(crate) fn render_menu(&self, grid: &mut Grid) -> Option<Cursor> {
        let menu = self.menu()?;
        let side_width = Frame::new(grid.width(), grid.height()).map_or(0, |frame| {
            let side_x = match frame.split {
                true => frame.divider() + 2,
                false => frame.x + 2,
            };
            usize::from((frame.right() - 1).saturating_sub(side_x))
        });
        let title = match menu {
            Menu::Plugin(id) | Menu::ConfirmRemove(id) | Menu::ConfirmUpdate(id) => {
                format!("nib › {}", self.plugins()[id].name)
            }
            Menu::ChooseBase => "Choose a base".into(),
            Menu::AddPlugin | Menu::ConfirmInstall => "nib › add a plugin".into(),
            Menu::Main | Menu::ConfirmQuit => "nib".into(),
        };
        let keys = match menu {
            Menu::Main | Menu::Plugin(_) => " ↑↓ move · enter choose · esc back ",
            Menu::ChooseBase => " ↑↓ move · enter choose · esc keep the one in use ",
            Menu::AddPlugin => " enter fetch · esc back ",
            _ => " y yes · any other key back ",
        };
        let line = |label: &str| Input::Line {
            label: label.into(),
            text: self.state().menu_input.clone(),
            cursor: self.state().menu_input.len(),
        };
        let (input, count, rows, side) = match menu {
            Menu::Main | Menu::Plugin(_) | Menu::ChooseBase => {
                let shown = self.shown(menu);
                let count = format!("{}/{}", shown.len(), self.rows(menu).len());
                let cursor = self.state().menu_cursor;
                let side = match shown.get(cursor) {
                    Some((row, _)) => self.item_details(&row.item, side_width),
                    None => Vec::new(),
                };
                let rows = self.list_rows(&shown);
                (line("> "), count, Some((rows, Some(cursor))), side)
            }
            Menu::AddPlugin => {
                let help = text_lines(
                    "Type what to install, then Enter:\n\
                     - a name from `nib plugin search`\n\
                     - owner/repo, or owner/repo@tag, on GitHub\n\
                     - the URL of a .nib.tar.gz\n\
                     - the path of a .nib.tar.gz",
                    side_width,
                );
                (line("add: "), String::new(), None, help)
            }
            Menu::ConfirmQuit => {
                let mut lines = text_lines("Not saved:", side_width);
                for buffer in self.state().buffers.iter().filter(|b| b.is_modified()) {
                    lines.push(vec![span(&format!("  {}", buffer.name()), "")]);
                }
                (
                    Input::Question(self.question(menu)),
                    String::new(),
                    None,
                    lines,
                )
            }
            Menu::ConfirmRemove(id) | Menu::ConfirmUpdate(id) => (
                Input::Question(self.question(menu)),
                String::new(),
                None,
                self.plugin_details(id, side_width),
            ),
            Menu::ConfirmInstall => {
                let can = match self.pending_install() {
                    Some(pending) => match pending.capabilities() {
                        [] => field("can", "nothing beyond the editor", "", side_width),
                        can => field("can", &can.join(", "), "", side_width),
                    },
                    None => Vec::new(),
                };
                (
                    Input::Question(self.question(menu)),
                    String::new(),
                    None,
                    can,
                )
            }
        };
        let content = Content {
            title,
            input,
            count,
            rows: rows
                .as_ref()
                .map(|(rows, selected)| (rows.as_slice(), *selected)),
            side: Side::Lines(side),
            keys,
        };
        self.draw_box(grid, &content)
    }

    /// The rows of a list as the box shows them: plugins with their version
    /// and state, and the chars that matched in the match color.
    fn list_rows(&self, shown: &[(Row, Vec<usize>)]) -> Vec<StyledLine> {
        let plugins = self.plugins();
        let names = shown
            .iter()
            .filter(|(row, _)| matches!(row.item, Item::Plugin(_) | Item::Base(_)))
            .map(|(row, _)| text_width(&row.name))
            .max()
            .unwrap_or(0)
            .min(16);
        let versions = plugins
            .iter()
            .map(|p| text_width(&p.version))
            .max()
            .unwrap_or(0)
            .min(10);
        let in_use = self.base_in_use();
        shown
            .iter()
            .map(|(row, matched)| {
                let mut line = highlighted(&row.name, matched);
                match &row.item {
                    Item::Plugin(id) => {
                        let plugin = &plugins[*id];
                        let (state, look) = self.plugin_state(plugin);
                        pad(&mut line, names);
                        line.push(span(
                            &format!("  {:<versions$}  ", plugin.version),
                            "ui.window",
                        ));
                        line.push(span(&state, look));
                    }
                    Item::Base(id) if Some(plugins[*id].name.as_str()) == in_use => {
                        pad(&mut line, names);
                        line.push(span("  in use", "ui.window"));
                    }
                    _ => {}
                }
                line
            })
            .collect()
    }

    /// The question a confirmation asks.
    fn question(&self, menu: Menu) -> String {
        match menu {
            Menu::ConfirmQuit => {
                let n = self.modified_buffers();
                format!(
                    "Quit without saving {n} modified buffer{}?",
                    if n == 1 { "" } else { "s" }
                )
            }
            Menu::ConfirmRemove(id) => format!("Remove {}?", self.plugins()[id].name),
            Menu::ConfirmUpdate(id) => match self.pending_update() {
                Some(pending) => format!(
                    "{} {} also wants: {}. Update?",
                    self.plugins()[id].name,
                    pending.version(),
                    pending.added_capabilities().join(", ")
                ),
                None => String::new(),
            },
            Menu::ConfirmInstall => match self.pending_install() {
                Some(p) => {
                    let can = match p.capabilities() {
                        [] => "no capabilities".to_string(),
                        can => format!("can: {}", can.join(", ")),
                    };
                    format!(
                        "install {} {} from {}? {can}",
                        p.name(),
                        p.version(),
                        p.source()
                    )
                }
                None => String::new(),
            },
            _ => String::new(),
        }
    }

    /// What the row under the cursor is, beside the list.
    fn item_details(&self, item: &Item, width: usize) -> Vec<StyledLine> {
        match *item {
            Item::Plugin(id) | Item::Base(id) => self.plugin_details(id, width),
            Item::Editor(action) => text_lines(action.about(), width),
            Item::ToPlugin(id, action) => {
                let plugin = &self.plugins()[id];
                let mut lines = text_lines(&plugin_action_about(action, plugin, self), width);
                lines.push(Vec::new());
                lines.extend(self.plugin_details(id, width));
                lines
            }
        }
    }

    fn plugin_details(&self, id: PluginId, width: usize) -> Vec<StyledLine> {
        let plugin = &self.plugins()[id];
        let mut lines = vec![vec![
            span(&plugin.name, "ui.popup.title"),
            span(&format!(" {}", plugin.version), "ui.window"),
        ]];
        if let Some(description) = &plugin.description {
            lines.extend(text_lines(description, width));
        }
        lines.push(Vec::new());
        let (state, look) = self.plugin_state(plugin);
        lines.extend(field("state", &state, look, width));
        if let Some(err) = &plugin.last_error {
            lines.extend(field("error", err, "ui.error", width));
        }
        let can = match plugin.capabilities.as_slice() {
            [] => "nothing beyond the editor".to_string(),
            can => can.join(", "),
        };
        lines.extend(field("can", &can, "", width));
        let limit = plugin
            .timeout
            .map_or("none".into(), |t| format!("{}ms per call", t.as_millis()));
        lines.extend(field("limit", &limit, "", width));
        lines.extend(field(
            "slow",
            &format!("{} calls", plugin.slow_calls),
            "",
            width,
        ));
        if let Some(dir) = &self.state().config_dir {
            // Beside config.toml, which "Open config.toml" shows the way to.
            let relative = format!("plugins/{}.toml", plugin.name);
            let settings = if dir.join(&relative).is_file() {
                relative
            } else {
                "none yet".into()
            };
            lines.extend(field("settings", &settings, "", width));
        }
        lines
    }

    /// A few words on how `plugin` is, and the theme entry to show them
    /// in.
    fn plugin_state(&self, plugin: &PluginInfo) -> (String, &'static str) {
        let failed = plugin.last_error.is_some();
        match (plugin.enabled, failed) {
            (true, _) if !plugin.has_code => ("languages".into(), "ui.window"),
            (true, _) if plugin.waiting => ("waiting".into(), "ui.window"),
            // Such as a call stopped with the menu key just now.
            (true, true) => ("running, last call failed".into(), "ui.error"),
            (true, false) if plugin.base => ("running, the base".into(), ""),
            (true, false) => ("running".into(), ""),
            (false, false) if plugin.base && self.base_in_use() != Some(&plugin.name) => {
                ("base, not in use".into(), "ui.window")
            }
            (false, true) => ("disabled, failed".into(), "ui.error"),
            (false, false) => ("disabled".into(), "ui.window"),
        }
    }
}

fn plugin_action_label(action: PluginAction, plugin: &PluginInfo) -> &'static str {
    match action {
        PluginAction::Restart => "Restart",
        PluginAction::Toggle if plugin.enabled => "Disable",
        PluginAction::Toggle if plugin.base => "Use as the base",
        PluginAction::Toggle => "Enable",
        PluginAction::Reload => "Reload from disk",
        PluginAction::Update => "Update",
        PluginAction::Remove => "Remove",
        PluginAction::OpenSettings => "Open its settings",
    }
}

fn plugin_action_about(action: PluginAction, plugin: &PluginInfo, editor: &Editor) -> String {
    let name = &plugin.name;
    match action {
        PluginAction::Restart => format!("Stop {name} and start it again."),
        PluginAction::Toggle if plugin.enabled => {
            format!("Stop {name}: its keys and commands go until it is enabled.")
        }
        PluginAction::Toggle if plugin.base => format!(
            "Edit the {name} way, in place of {}. base = \"{name}\" in config.toml keeps it.",
            editor.base_in_use().unwrap_or("the base in use")
        ),
        PluginAction::Toggle => format!("Start {name} again."),
        PluginAction::Reload => {
            format!("Read {name} again from its directory, as after building it.")
        }
        PluginAction::Update => format!("Look for a newer release of {name} and install it."),
        PluginAction::Remove => format!("Uninstall {name}. Its settings and data are kept."),
        PluginAction::OpenSettings => format!("Open plugins/{name}.toml, {name}'s settings."),
    }
}

/// The char indices of `text` where the chars of `pattern` appear in
/// order, ignoring case; `None` when they do not all appear.
fn matches(text: &str, pattern: &str) -> Option<Vec<usize>> {
    let mut found = Vec::new();
    let mut chars = text.chars().enumerate();
    for wanted in pattern.chars().flat_map(char::to_lowercase) {
        let (i, _) = chars.find(|(_, c)| c.to_lowercase().any(|c| c == wanted))?;
        found.push(i);
    }
    Some(found)
}

/// `text` with the chars at `matched` in the match color.
fn highlighted(text: &str, matched: &[usize]) -> StyledLine {
    let mut line: StyledLine = Vec::new();
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

/// Pads `line` with spaces to `width` columns.
fn pad(line: &mut StyledLine, width: usize) {
    let used: usize = line.iter().map(|s| text_width(&s.text)).sum();
    if used < width {
        line.push(span(&" ".repeat(width - used), ""));
    }
}

/// A label and its value, the value broken into lines beside the label.
fn field(label: &str, value: &str, style: &str, width: usize) -> Vec<StyledLine> {
    const LABEL: usize = 10;
    let lines = wrap(value, width.saturating_sub(LABEL).max(1));
    lines
        .into_iter()
        .enumerate()
        .map(|(i, text)| {
            let label = if i == 0 { label } else { "" };
            vec![
                span(&format!("{label:<LABEL$}"), "ui.window"),
                span(&text, style),
            ]
        })
        .collect()
}

fn text_lines(text: &str, width: usize) -> Vec<StyledLine> {
    wrap(text, width.max(1))
        .into_iter()
        .map(|line| vec![span(&line, "")])
        .collect()
}

/// `text` broken into lines of at most `width` columns, at spaces where it
/// can.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        let mut used = 0;
        for word in paragraph.split(' ') {
            let size = text_width(word);
            if used > 0 && used + 1 + size > width {
                lines.push(std::mem::take(&mut line));
                used = 0;
            }
            if used > 0 {
                line.push(' ');
                used += 1;
            }
            for grapheme in graphemes(word) {
                let size = display_width(grapheme) as usize;
                if used > 0 && used + size > width {
                    lines.push(std::mem::take(&mut line));
                    used = 0;
                }
                line.push_str(grapheme);
                used += size;
            }
        }
        lines.push(line);
    }
    lines
}

fn span(text: &str, style: &str) -> Span {
    Span {
        text: text.into(),
        style: style.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_chars_match_in_order_ignoring_case() {
        assert_eq!(matches("Restart all", "rsa"), Some(vec![0, 2, 4]));
        assert_eq!(matches("helix", ""), Some(vec![]));
        assert_eq!(matches("helix", "xh"), None);
        assert_eq!(matches("Quit", "QU"), Some(vec![0, 1]));
    }

    #[test]
    fn text_wraps_at_spaces_and_breaks_long_words() {
        assert_eq!(wrap("a bb ccc", 4), ["a bb", "ccc"]);
        assert_eq!(wrap("abcdef", 4), ["abcd", "ef"]);
        assert_eq!(wrap("a\nb", 4), ["a", "b"]);
    }
}
