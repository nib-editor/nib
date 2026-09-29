//! Host side of `nib:plugin`: what plugins can call.

use std::ops::Range as ByteRange;
use std::time::{Duration, Instant};

use wasmtime::component::Resource;
use wasmtime_wasi::{WasiCtxView, WasiView};

use super::PluginData;
use crate::buffer::Buffer;
use crate::config::Indent;
use crate::editor::{CORE_COMMANDS, ScrollAmount, State, Toward};
use crate::events::{Command, Event, Mode, Timer};
use crate::grapheme;
use crate::grid::CursorShape;
use crate::history::UndoMode;
use crate::input::{KeyCode, KeyEvent};
use crate::layout;
use crate::process::Stream;
use crate::prompt::{Action, Choices, Prompt};
use crate::selection::{Range, Selection};
use crate::syntax::{self as trees, NodeInfo};
use crate::ui::{Panel, Popup, PopupAnchor, Side, Span, StatusItem, StyledLine};
use crate::{Edit, Error};

pub(crate) mod bindings {
    wasmtime::component::bindgen!({
        // A copy of api/wit, which CI checks, as the crate on crates.io
        // cannot reach outside itself.
        path: "wit",
        world: "plugin",
        // Host functions trap when a plugin misuses the API, e.g. with a
        // handle to a buffer that no longer exists.
        imports: { default: trappable },
        with: {
            "nib:plugin/buffer.buffer": super::BufferHandle,
            "nib:plugin/view.view": super::ViewHandle,
            "nib:plugin/ui.panel": super::PanelHandle,
            "nib:plugin/ui.popup": super::PopupHandle,
            "nib:plugin/prompt.line": super::PromptHandle,
            "nib:plugin/prompt.choices": super::ChoicesHandle,
            "nib:plugin/process.child": super::ChildHandle,
        },
    });
}

use bindings::nib::plugin::{
    buffer as wit_buffer, clipboard, commands, editor, events, files, input, process,
    prompt as wit_prompt, settings, syntax, timers, types as wit, ui as wit_ui, view as wit_view,
};

/// A buffer as seen by a plugin. The resource's rep is the buffer index.
pub struct BufferHandle;

/// A view as seen by a plugin: the focused view, with rep 0.
pub struct ViewHandle;

/// A panel as seen by a plugin. The resource's rep is the panel id.
pub struct PanelHandle;

/// A popup as seen by a plugin. The resource's rep is the popup id.
pub struct PopupHandle;

/// A prompt as seen by the plugin that opened it. The resource's rep is the
/// prompt id.
pub struct PromptHandle;

/// Choices as seen by the plugin that opened them. The resource's rep is
/// their id.
pub struct ChoicesHandle;

/// A program as seen by the plugin that started it. The resource's rep is
/// the process id.
pub struct ChildHandle;

type HostResult<T> = wasmtime::Result<T>;

impl WasiView for PluginData {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl PluginData {
    fn state(&mut self) -> HostResult<&mut State> {
        self.state
            .as_mut()
            .ok_or_else(|| wasmtime::Error::msg("the editor is only reachable during a call"))
    }

    fn buffer(&mut self, handle: &Resource<BufferHandle>) -> HostResult<&mut Buffer> {
        self.state()?
            .buffers
            .get_mut(handle.rep() as usize)
            .filter(|b| !b.is_closed())
            .ok_or_else(|| wasmtime::Error::msg("the buffer no longer exists"))
    }

    /// The buffer's index and `start..end` clamped to its text, for the
    /// syntax API, which answers out-of-range questions with nothing rather
    /// than errors.
    fn buffer_range(
        &mut self,
        handle: &Resource<BufferHandle>,
        start: u64,
        end: u64,
    ) -> HostResult<(usize, ByteRange<usize>)> {
        let len = self.buffer(handle)?.len();
        let clamp = |offset: u64| usize::try_from(offset).unwrap_or(usize::MAX).min(len);
        let start = clamp(start);
        Ok((handle.rep() as usize, start..clamp(end).max(start)))
    }

    fn view_state(&mut self, handle: &Resource<ViewHandle>) -> HostResult<&mut State> {
        if handle.rep() != 0 {
            return Err(wasmtime::Error::msg("the view no longer exists"));
        }
        self.state()
    }
}

impl wit::Host for PluginData {}

impl editor::Host for PluginData {
    fn working_directory(&mut self) -> HostResult<String> {
        let dir = std::env::current_dir().unwrap_or_default();
        Ok(dir.to_string_lossy().into_owned())
    }

    fn quit(&mut self, force: bool) -> HostResult<Result<(), String>> {
        Ok(self.state()?.quit(force))
    }

    fn open_config(&mut self, plugin: Option<String>) -> HostResult<Result<(), String>> {
        Ok(self.state()?.open_config(plugin.as_deref()))
    }

    fn reload_config(&mut self) -> HostResult<()> {
        self.state()?.reload_config = true;
        Ok(())
    }

    fn open_menu(&mut self) -> HostResult<()> {
        self.state()?.open_menu();
        Ok(())
    }
}

impl wit_buffer::Host for PluginData {
    fn open(&mut self, path: String) -> HostResult<Result<Resource<BufferHandle>, String>> {
        Ok(self
            .state()?
            .open_buffer(&path)
            .map(|index| Resource::new_own(index as u32))
            .map_err(|err| format!("{path}: {err}")))
    }

    fn create(&mut self, name: String) -> HostResult<Resource<BufferHandle>> {
        let owner = self.plugin;
        let state = self.state()?;
        state.buffers.push(Buffer::owned(owner, name));
        let index = state.buffers.len() - 1;
        state.push_event(None, Event::BufferOpened(index));
        Ok(Resource::new_own(index as u32))
    }

    fn all(&mut self) -> HostResult<Vec<Resource<BufferHandle>>> {
        let buffers = &self.state()?.buffers;
        Ok((0..buffers.len())
            .filter(|&i| !buffers[i].is_closed())
            .map(|i| Resource::new_own(i as u32))
            .collect())
    }
}

impl wit_buffer::HostBuffer for PluginData {
    fn version(&mut self, buffer: Resource<BufferHandle>) -> HostResult<u64> {
        Ok(self.buffer(&buffer)?.version())
    }

    fn len(&mut self, buffer: Resource<BufferHandle>) -> HostResult<u64> {
        Ok(self.buffer(&buffer)?.len() as u64)
    }

    fn slice(
        &mut self,
        buffer: Resource<BufferHandle>,
        start: u64,
        end: u64,
    ) -> HostResult<Result<String, wit::Error>> {
        let buffer = self.buffer(&buffer)?;
        wit_result(pos(start).and_then(|start| buffer.slice(start, pos(end)?)))
    }

    fn line_count(&mut self, buffer: Resource<BufferHandle>) -> HostResult<u64> {
        Ok(self.buffer(&buffer)?.line_count() as u64)
    }

    fn line_start(&mut self, buffer: Resource<BufferHandle>, line: u64) -> HostResult<Option<u64>> {
        let buffer = self.buffer(&buffer)?;
        Ok(usize::try_from(line)
            .ok()
            .and_then(|line| buffer.line_start(line))
            .map(|pos| pos as u64))
    }

    fn line_of(
        &mut self,
        buffer: Resource<BufferHandle>,
        at: u64,
    ) -> HostResult<Result<u64, wit::Error>> {
        let buffer = self.buffer(&buffer)?;
        wit_result(
            pos(at)
                .and_then(|at| buffer.line_of(at))
                .map(|line| line as u64),
        )
    }

    fn next_grapheme(
        &mut self,
        buffer: Resource<BufferHandle>,
        at: u64,
    ) -> HostResult<Result<u64, wit::Error>> {
        let buffer = self.buffer(&buffer)?;
        wit_result(
            pos(at)
                .and_then(|at| buffer.next_grapheme(at))
                .map(|p| p as u64),
        )
    }

    fn prev_grapheme(
        &mut self,
        buffer: Resource<BufferHandle>,
        at: u64,
    ) -> HostResult<Result<u64, wit::Error>> {
        let buffer = self.buffer(&buffer)?;
        wit_result(
            pos(at)
                .and_then(|at| buffer.prev_grapheme(at))
                .map(|p| p as u64),
        )
    }

    fn path(&mut self, buffer: Resource<BufferHandle>) -> HostResult<Option<String>> {
        let buffer = self.buffer(&buffer)?;
        Ok(buffer
            .path()
            .map(|path| path.to_string_lossy().into_owned()))
    }

    fn name(&mut self, buffer: Resource<BufferHandle>) -> HostResult<String> {
        Ok(self.buffer(&buffer)?.name())
    }

    fn set_editable(&mut self, buffer: Resource<BufferHandle>, editable: bool) -> HostResult<()> {
        let caller = self.plugin;
        if let Some(plugin) = self.buffer(&buffer)?.plugin.as_mut()
            && plugin.owner == caller
        {
            plugin.editable = editable;
        }
        Ok(())
    }

    fn set_keys(
        &mut self,
        buffer: Resource<BufferHandle>,
        keys: Vec<(String, String)>,
    ) -> HostResult<Result<(), String>> {
        let caller = self.plugin;
        let prefix = format!("{}.", self.plugin_name()?);
        let Some(plugin) = self.buffer(&buffer)?.plugin.as_mut() else {
            return Ok(Err("only a plugin's own buffers have keys".into()));
        };
        if plugin.owner != caller {
            return Ok(Err(
                "only the plugin that made a buffer gives it keys".into()
            ));
        }
        for (key, command) in &keys {
            if let Err(err) = key
                .split_whitespace()
                .map(str::parse::<KeyEvent>)
                .collect::<Result<Vec<_>, _>>()
            {
                return Ok(Err(format!("{key}: {err}")));
            }
            if !command.starts_with(&prefix) {
                return Ok(Err(format!(
                    "{key}: {command} is not one of this plugin's commands"
                )));
            }
        }
        plugin.keys = keys;
        Ok(Ok(()))
    }

    fn keys(&mut self, buffer: Resource<BufferHandle>) -> HostResult<Vec<(String, String)>> {
        Ok(self
            .buffer(&buffer)?
            .plugin
            .as_ref()
            .map(|p| p.keys.clone())
            .unwrap_or_default())
    }

    fn modified(&mut self, buffer: Resource<BufferHandle>) -> HostResult<bool> {
        Ok(self.buffer(&buffer)?.is_modified())
    }

    fn find(
        &mut self,
        buffer: Resource<BufferHandle>,
        pattern: String,
        start: u64,
        backward: bool,
    ) -> HostResult<Result<Option<wit::Range>, wit::Error>> {
        let buffer = self.buffer(&buffer)?;
        wit_result(
            pos(start)
                .and_then(|start| buffer.find(&pattern, start, backward))
                .map(|found| found.map(|(s, e)| range(s, e))),
        )
    }

    fn find_groups(
        &mut self,
        buffer: Resource<BufferHandle>,
        pattern: String,
        start: u64,
        backward: bool,
    ) -> HostResult<Result<Option<Vec<Option<wit::Range>>>, wit::Error>> {
        let buffer = self.buffer(&buffer)?;
        wit_result(
            pos(start)
                .and_then(|start| buffer.find_groups(&pattern, start, backward))
                .map(|found| {
                    found.map(|groups| {
                        groups
                            .into_iter()
                            .map(|group| group.map(|(s, e)| range(s, e)))
                            .collect()
                    })
                }),
        )
    }

    fn find_all(
        &mut self,
        buffer: Resource<BufferHandle>,
        pattern: String,
        start: u64,
        end: u64,
    ) -> HostResult<Result<Vec<wit::Range>, wit::Error>> {
        let buffer = self.buffer(&buffer)?;
        wit_result(
            pos(start)
                .and_then(|start| buffer.find_all(&pattern, start, pos(end)?))
                .map(|found| found.into_iter().map(|(s, e)| range(s, e)).collect()),
        )
    }

    fn apply(
        &mut self,
        buffer: Resource<BufferHandle>,
        base_version: u64,
        edits: Vec<wit::Edit>,
        undo: wit::UndoMode,
    ) -> HostResult<Result<(), wit::Error>> {
        let caller = self.plugin;
        self.buffer(&buffer)?;
        let index = buffer.rep() as usize;
        let state = self.state()?;
        let buffer = &state.buffers[index];
        if !buffer.editable() && buffer.owner() != Some(caller) {
            state.message = Some(format!("{} is read-only", state.buffers[index].name()));
            return Ok(Err(wit::Error::ReadOnly));
        }
        let shown = state.view.buffer == index;
        let selection = if shown {
            state.view.selection.clone()
        } else {
            Selection::point(0)
        };
        let result = (|| {
            let edits = edits
                .into_iter()
                .map(|edit| Ok(Edit::new(pos(edit.start)?, pos(edit.end)?, edit.text)))
                .collect::<Result<Vec<_>, Error>>()?;
            let mode = match undo {
                wit::UndoMode::NewStep => UndoMode::NewStep,
                wit::UndoMode::Merge => UndoMode::Merge,
            };
            state.buffers[index].apply(base_version, edits, &selection, None, mode)
        })();
        let result = result.map(|change| {
            state.sync_views(index, &change.changes);
            if shown {
                state.view.selection = change.selection;
            }
        });
        wit_result(result)
    }

    fn save(
        &mut self,
        buffer: Resource<BufferHandle>,
        path: Option<String>,
    ) -> HostResult<Result<(), String>> {
        self.buffer(&buffer)?;
        let index = buffer.rep() as usize;
        Ok(self.state()?.save_buffer(index, path.as_deref()))
    }

    fn close(
        &mut self,
        buffer: Resource<BufferHandle>,
        force: bool,
    ) -> HostResult<Result<(), String>> {
        self.buffer(&buffer)?;
        let index = buffer.rep() as usize;
        Ok(self.state()?.close_buffer_at(index, force))
    }

    fn set_marks(
        &mut self,
        buffer: Resource<BufferHandle>,
        namespace: String,
        marks: Vec<u64>,
    ) -> HostResult<()> {
        let owner = self.plugin;
        let offset = |o: u64| usize::try_from(o).unwrap_or(usize::MAX);
        self.buffer(&buffer)?
            .set_marks(owner, &namespace, marks.into_iter().map(offset).collect());
        Ok(())
    }

    fn marks(&mut self, buffer: Resource<BufferHandle>, namespace: String) -> HostResult<Vec<u64>> {
        let owner = self.plugin;
        let buffer = self.buffer(&buffer)?;
        Ok(buffer
            .marks(owner, &namespace)
            .iter()
            .map(|&p| p as u64)
            .collect())
    }

    fn drop(&mut self, _buffer: Resource<BufferHandle>) -> HostResult<()> {
        Ok(())
    }
}

impl wit_view::Host for PluginData {
    fn active(&mut self) -> HostResult<Resource<ViewHandle>> {
        Ok(Resource::new_own(0))
    }

    fn split(&mut self, direction: wit_view::Direction) -> HostResult<()> {
        let side_by_side = direction == wit_view::Direction::Vertical;
        self.state()?.split(side_by_side);
        Ok(())
    }

    fn close(&mut self) -> HostResult<Result<(), String>> {
        Ok(self.state()?.close_view())
    }

    fn only(&mut self) -> HostResult<()> {
        self.state()?.only_view();
        Ok(())
    }

    fn focus(&mut self, toward: wit_view::Toward) -> HostResult<()> {
        let toward = match toward {
            wit_view::Toward::Next => Toward::Next,
            wit_view::Toward::Left => Toward::Left,
            wit_view::Toward::Right => Toward::Right,
            wit_view::Toward::Up => Toward::Up,
            wit_view::Toward::Down => Toward::Down,
        };
        self.state()?.focus_toward(toward);
        Ok(())
    }
}

impl wit_view::HostView for PluginData {
    fn buffer(&mut self, view: Resource<ViewHandle>) -> HostResult<Resource<BufferHandle>> {
        let state = self.view_state(&view)?;
        Ok(Resource::new_own(state.view.buffer as u32))
    }

    fn show(
        &mut self,
        view: Resource<ViewHandle>,
        buffer: Resource<BufferHandle>,
    ) -> HostResult<()> {
        self.buffer(&buffer)?;
        let index = buffer.rep() as usize;
        self.view_state(&view)?.show(index);
        Ok(())
    }

    fn show_next(&mut self, view: Resource<ViewHandle>) -> HostResult<()> {
        self.view_state(&view)?.show_next(true);
        Ok(())
    }

    fn show_previous(&mut self, view: Resource<ViewHandle>) -> HostResult<()> {
        self.view_state(&view)?.show_next(false);
        Ok(())
    }

    fn selection(&mut self, view: Resource<ViewHandle>) -> HostResult<wit::Selection> {
        let state = self.view_state(&view)?;
        Ok(to_wit_selection(&state.view.selection))
    }

    fn set_selection(
        &mut self,
        view: Resource<ViewHandle>,
        selection: wit::Selection,
    ) -> HostResult<Result<(), wit::Error>> {
        let state = self.view_state(&view)?;
        let buffer = &state.buffers[state.view.buffer];
        let result = from_wit_selection(selection)
            .and_then(|(ranges, primary)| Selection::new(ranges, primary, buffer.text()));
        if let Ok(selection) = &result {
            state.view.selection = selection.clone();
        }
        wit_result(result.map(|_| ()))
    }

    fn apply(
        &mut self,
        view: Resource<ViewHandle>,
        base_version: u64,
        edits: Vec<wit::Edit>,
        after: Option<wit::Selection>,
        undo: wit::UndoMode,
    ) -> HostResult<Result<(), wit::Error>> {
        let state = self.view_state(&view)?;
        let buffer = &mut state.buffers[state.view.buffer];
        if !buffer.editable() {
            state.message = Some(format!("{} is read-only", buffer.name()));
            return Ok(Err(wit::Error::ReadOnly));
        }
        let result = (|| {
            let edits = edits
                .into_iter()
                .map(|edit| Ok(Edit::new(pos(edit.start)?, pos(edit.end)?, edit.text)))
                .collect::<Result<Vec<_>, Error>>()?;
            let after = after.map(from_wit_selection).transpose()?;
            let mode = match undo {
                wit::UndoMode::NewStep => UndoMode::NewStep,
                wit::UndoMode::Merge => UndoMode::Merge,
            };
            buffer.apply(base_version, edits, &state.view.selection, after, mode)
        })();
        let index = state.view.buffer;
        let result = result.map(|change| {
            state.sync_views(index, &change.changes);
            state.view.selection = change.selection;
        });
        wit_result(result)
    }

    fn undo(&mut self, view: Resource<ViewHandle>) -> HostResult<Option<wit::Range>> {
        self.undo_or_redo(&view, true)
    }

    fn redo(&mut self, view: Resource<ViewHandle>) -> HostResult<Option<wit::Range>> {
        self.undo_or_redo(&view, false)
    }

    fn move_vertically(
        &mut self,
        view: Resource<ViewHandle>,
        at: u64,
        lines: i32,
        column: Option<u32>,
    ) -> HostResult<Result<(u64, u32), wit::Error>> {
        let state = self.view_state(&view)?;
        let tab_width = state.tab_width(state.view.buffer);
        let text = state.buffers[state.view.buffer].text();
        wit_result(
            pos(at)
                .and_then(|at| {
                    grapheme::check_position(text, at)?;
                    Ok(layout::move_vertically(text, at, lines, column, tab_width))
                })
                .map(|(at, column)| (at as u64, column)),
        )
    }

    fn scroll(
        &mut self,
        view: Resource<ViewHandle>,
        amount: wit_view::ScrollAmount,
    ) -> HostResult<i32> {
        let state = self.view_state(&view)?;
        Ok(state.scroll(match amount {
            wit_view::ScrollAmount::Lines(n) => ScrollAmount::Lines(n),
            wit_view::ScrollAmount::HalfPage(n) => ScrollAmount::HalfPage(n),
            wit_view::ScrollAmount::Page(n) => ScrollAmount::Page(n),
        }))
    }

    fn visible_range(&mut self, view: Resource<ViewHandle>) -> HostResult<wit::Range> {
        let (start, end) = self.view_state(&view)?.visible_range();
        Ok(range(start, end))
    }

    fn set_cursor_shape(
        &mut self,
        view: Resource<ViewHandle>,
        shape: wit::CursorShape,
    ) -> HostResult<()> {
        let state = self.view_state(&view)?;
        state.view.cursor_shape = match shape {
            wit::CursorShape::Block => CursorShape::Block,
            wit::CursorShape::Bar => CursorShape::Bar,
            wit::CursorShape::Underline => CursorShape::Underline,
        };
        Ok(())
    }

    fn drop(&mut self, _view: Resource<ViewHandle>) -> HostResult<()> {
        Ok(())
    }
}

impl syntax::Host for PluginData {
    fn language(&mut self, buffer: Resource<BufferHandle>) -> HostResult<Option<String>> {
        let (index, _) = self.buffer_range(&buffer, 0, 0)?;
        Ok(self.state()?.with_syntax(index, |languages, syntax, _| {
            languages.name(syntax.language).to_string()
        }))
    }

    fn node_at(
        &mut self,
        buffer: Resource<BufferHandle>,
        start: u64,
        end: u64,
        named: bool,
    ) -> HostResult<Option<syntax::Node>> {
        let (index, range) = self.buffer_range(&buffer, start, end)?;
        let found = self.state()?.with_syntax(index, |_, syntax, _| {
            trees::node_at_in(syntax, range, named)
        });
        Ok(found.flatten().map(wit_node))
    }

    fn parent(
        &mut self,
        buffer: Resource<BufferHandle>,
        of: syntax::Node,
    ) -> HostResult<Option<syntax::Node>> {
        let (index, _) = self.buffer_range(&buffer, 0, 0)?;
        let of = node_info(of);
        let found = self
            .state()?
            .with_syntax(index, |_, syntax, _| trees::parent_in(syntax, &of));
        Ok(found.flatten().map(wit_node))
    }

    fn children(
        &mut self,
        buffer: Resource<BufferHandle>,
        of: syntax::Node,
    ) -> HostResult<Vec<syntax::Node>> {
        let (index, _) = self.buffer_range(&buffer, 0, 0)?;
        let of = node_info(of);
        let found = self
            .state()?
            .with_syntax(index, |_, syntax, _| trees::children_in(syntax, &of));
        Ok(found
            .unwrap_or_default()
            .into_iter()
            .map(wit_node)
            .collect())
    }

    fn captures(
        &mut self,
        buffer: Resource<BufferHandle>,
        query: String,
        capture: String,
        start: u64,
        end: u64,
    ) -> HostResult<Vec<wit::Range>> {
        let (index, span) = self.buffer_range(&buffer, start, end)?;
        let state = self.state()?;
        let found = state.with_syntax(index, |languages, syntax, text| {
            languages.captures_in(syntax, &query, &capture, text, span)
        });
        match found {
            Some(Ok(found)) => Ok(found.into_iter().map(|r| range(r.start, r.end)).collect()),
            // A broken query is the language plugin's bug, not the caller's.
            Some(Err(err)) => {
                state.message = Some(format!("syntax: {err}"));
                Ok(Vec::new())
            }
            None => Ok(Vec::new()),
        }
    }
}

fn wit_node(node: NodeInfo) -> syntax::Node {
    syntax::Node {
        id: node.id,
        kind: node.kind,
        named: node.named,
        start: node.range.start as u64,
        end: node.range.end as u64,
    }
}

fn node_info(node: syntax::Node) -> NodeInfo {
    let offset = |o: u64| usize::try_from(o).unwrap_or(usize::MAX);
    NodeInfo {
        id: node.id,
        kind: node.kind,
        named: node.named,
        range: offset(node.start)..offset(node.end),
    }
}

impl input::Host for PluginData {
    fn push_layer(&mut self) -> HostResult<()> {
        let plugin = self.plugin;
        self.state()?.layers.push(plugin);
        Ok(())
    }

    fn leader_keys(&mut self) -> HostResult<Vec<input::LeaderKey>> {
        Ok(self
            .state()?
            .leader_keys
            .iter()
            .map(|key| input::LeaderKey {
                plugin: key.name.clone(),
                keys: key.keys.clone(),
                command: key.command.clone(),
            })
            .collect())
    }

    fn set_mode(&mut self, name: String, typing: bool) -> HostResult<()> {
        if !self.is_base {
            return Ok(());
        }
        let base = self.plugin_name()?.to_string();
        let state = self.state()?;
        let mode = Mode { base, name, typing };
        if state.mode != mode {
            state.mode = mode.clone();
            state.push_event(None, Event::ModeChanged(mode));
        }
        Ok(())
    }

    fn current_mode(&mut self) -> HostResult<input::Mode> {
        Ok(wit_mode(&self.state()?.mode))
    }

    fn pop_layer(&mut self) -> HostResult<()> {
        let plugin = self.plugin;
        let layers = &mut self.state()?.layers;
        if let Some(i) = layers.iter().rposition(|&layer| layer == plugin) {
            layers.remove(i);
        }
        Ok(())
    }
}

impl commands::Host for PluginData {
    fn register(&mut self, name: String, description: String) -> HostResult<()> {
        if name.is_empty() || name.contains(char::is_whitespace) {
            return Err(wasmtime::Error::msg(format!(
                "command name {name:?} must be a word"
            )));
        }
        let owner = self.plugin;
        let full = format!("{}.{name}", self.plugin_name()?);
        let commands = &mut self.state()?.commands;
        commands.retain(|command| command.name != full);
        commands.push(Command {
            owner,
            name: full,
            short: name,
            description,
        });
        Ok(())
    }

    fn call(&mut self, name: String, args: String) -> HostResult<Result<String, String>> {
        let (name, args) = match self.state()?.redirect(&name) {
            Some(Ok(redirected)) => redirected,
            Some(Err(err)) => return Ok(Err(err)),
            None => (name, args),
        };
        self.wake_for_command(&name)?;
        let state = self.state()?;
        match state.commands.iter().find(|command| command.name == name) {
            Some(command) => {
                let (owner, short) = (command.owner, command.short.clone());
                self.call_plugin_command(owner, &name, &short, &args)
            }
            None => Ok(state.run_command(&name, &args)),
        }
    }

    fn all(&mut self) -> HostResult<Vec<(String, String)>> {
        let core = CORE_COMMANDS
            .iter()
            .map(|&(name, description)| (name.to_string(), description.to_string()));
        let registered = self
            .state()?
            .commands
            .iter()
            .map(|command| (command.name.clone(), command.description.clone()));
        Ok(core.chain(registered).collect())
    }
}

impl events::Host for PluginData {
    fn emit(&mut self, name: String, data: String) -> HostResult<()> {
        let name = format!("{}.{name}", self.plugin_name()?);
        self.state()?.push_event(None, Event::Custom { name, data });
        Ok(())
    }
}

impl timers::Host for PluginData {
    fn set(&mut self, ms: u32) -> HostResult<u64> {
        let owner = self.plugin;
        let state = self.state()?;
        state.last_timer_id += 1;
        let id = state.last_timer_id;
        state.timers.push(Timer {
            id,
            owner,
            due: Instant::now() + Duration::from_millis(u64::from(ms)),
        });
        Ok(id)
    }

    fn cancel(&mut self, id: u64) -> HostResult<()> {
        let owner = self.plugin;
        self.state()?
            .timers
            .retain(|timer| !(timer.id == id && timer.owner == owner));
        Ok(())
    }
}

impl process::Host for PluginData {
    fn spawn(
        &mut self,
        command: String,
        args: Vec<String>,
        cwd: Option<String>,
    ) -> HostResult<Result<Resource<ChildHandle>, String>> {
        if !self.can_spawn {
            return Ok(Err(format!(
                "{command}: starting programs needs the \"process\" capability"
            )));
        }
        let owner = self.plugin;
        let spawned = self
            .state()?
            .processes
            .spawn(owner, &command, &args, cwd.map(Into::into));
        Ok(spawned.map(Resource::new_own))
    }
}

impl clipboard::Host for PluginData {
    fn get(&mut self) -> HostResult<Result<String, String>> {
        if !self.can_use_clipboard {
            return Ok(Err(NO_CLIPBOARD.into()));
        }
        Ok(self.state()?.clipboard.get())
    }

    fn set(&mut self, text: String) -> HostResult<Result<(), String>> {
        if !self.can_use_clipboard {
            return Ok(Err(NO_CLIPBOARD.into()));
        }
        Ok(self.state()?.clipboard.set(&text))
    }
}

const NO_CLIPBOARD: &str = "the clipboard needs the \"clipboard\" capability";

impl files::Host for PluginData {
    fn list(&mut self, dir: String) -> HostResult<Result<Vec<files::DirEntry>, String>> {
        if !self.can_read_files {
            return Ok(Err("listing files needs the \"fs-read\" capability".into()));
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) => return Ok(Err(format!("{dir}: {err}"))),
        };
        let mut listed: Vec<files::DirEntry> = entries
            .filter_map(Result::ok)
            .map(|entry| {
                // Through links, so a link to a directory can be entered.
                let metadata = std::fs::metadata(entry.path()).ok();
                let directory = metadata.as_ref().is_some_and(|m| m.is_dir());
                files::DirEntry {
                    name: entry.file_name().to_string_lossy().into_owned(),
                    directory,
                    size: metadata.filter(|_| !directory).map_or(0, |m| m.len()),
                }
            })
            .collect();
        listed.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(Ok(listed))
    }

    fn walk(&mut self, dir: Option<String>) -> HostResult<Result<u64, String>> {
        if !self.can_read_files {
            return Ok(Err("listing files needs the \"fs-read\" capability".into()));
        }
        let dir = match dir {
            Some(dir) => dir.into(),
            None => std::env::current_dir().unwrap_or_default(),
        };
        let owner = self.plugin;
        Ok(self.state()?.files.list(owner, dir).map(u64::from))
    }

    fn cancel(&mut self, id: u64) -> HostResult<()> {
        let owner = self.plugin;
        if let Ok(id) = u32::try_from(id) {
            self.state()?.files.cancel(id, owner);
        }
        Ok(())
    }
}

impl process::HostChild for PluginData {
    fn id(&mut self, child: Resource<ChildHandle>) -> HostResult<u64> {
        Ok(u64::from(child.rep()))
    }

    fn write(
        &mut self,
        child: Resource<ChildHandle>,
        data: Vec<u8>,
    ) -> HostResult<Result<(), String>> {
        let owner = self.plugin;
        Ok(self.state()?.processes.write(child.rep(), owner, &data))
    }

    fn close_stdin(&mut self, child: Resource<ChildHandle>) -> HostResult<()> {
        let owner = self.plugin;
        self.state()?.processes.close_stdin(child.rep(), owner);
        Ok(())
    }

    fn kill(&mut self, child: Resource<ChildHandle>) -> HostResult<()> {
        let owner = self.plugin;
        self.state()?.processes.kill(child.rep(), owner);
        Ok(())
    }

    fn drop(&mut self, child: Resource<ChildHandle>) -> HostResult<()> {
        let owner = self.plugin;
        // Outside a call, the plugin is being stopped and its programs are
        // killed anyway.
        if let Some(state) = self.state.as_mut() {
            state.processes.forget(child.rep(), owner);
        }
        Ok(())
    }
}

/// An event as a plugin gets it, with handles to the buffers it names.
pub(crate) fn wit_event(event: &Event) -> events::Event {
    let buffer = |index: usize| Resource::new_own(index as u32);
    let offset = |o: usize| o as u64;
    match event {
        Event::BufferOpened(index) => events::Event::BufferOpened(buffer(*index)),
        Event::BufferSaved(index) => events::Event::BufferSaved(buffer(*index)),
        Event::BufferClosed { path, name } => events::Event::BufferClosed(events::ClosedBuffer {
            path: path.clone(),
            name: name.clone(),
        }),
        Event::SyntaxUpdated {
            buffer: index,
            version,
        } => events::Event::SyntaxUpdated(events::SyntaxUpdate {
            buffer: buffer(*index),
            version: *version,
        }),
        Event::ModeChanged(mode) => events::Event::ModeChanged(wit_mode(mode)),
        Event::BufferChanged {
            buffer: index,
            version,
            changes,
        } => events::Event::BufferChanged(events::BufferChange {
            buffer: buffer(*index),
            version: *version,
            changes: changes
                .iter()
                .map(|change| events::TextChange {
                    start: offset(change.start),
                    end: offset(change.end),
                    start_line: change.start_line as u32,
                    start_column: change.start_column as u32,
                    end_line: change.end_line as u32,
                    end_column: change.end_column as u32,
                    text: change.text.clone(),
                })
                .collect(),
        }),
        Event::Custom { name, data } => events::Event::Custom(events::CustomEvent {
            name: name.clone(),
            data: data.clone(),
        }),
        Event::Timer(id) => events::Event::Timer(*id),
        Event::ProcessOutput {
            process,
            stream,
            data,
        } => events::Event::ProcessOutput(events::ProcessOutput {
            process: u64::from(*process),
            stream: match stream {
                Stream::Stdout => process::Stream::Stdout,
                Stream::Stderr => process::Stream::Stderr,
            },
            data: data.clone(),
        }),
        Event::ProcessExit { process, code } => events::Event::ProcessExit(events::ProcessExit {
            process: u64::from(*process),
            code: *code,
        }),
        Event::FilesListed { job, paths, done } => {
            events::Event::FilesListed(events::FilesListed {
                job: u64::from(*job),
                paths: paths.clone(),
                done: *done,
            })
        }
        Event::PromptChanged {
            prompt,
            text,
            cursor,
        } => events::Event::PromptChanged(events::PromptChange {
            id: u64::from(*prompt),
            text: text.clone(),
            cursor: *cursor as u32,
        }),
        Event::PromptAction { prompt, action } => events::Event::PromptAction(events::PromptAct {
            id: u64::from(*prompt),
            action: wit_action(*action),
        }),
    }
}

impl settings::Host for PluginData {
    fn tab_width(&mut self, buffer: Resource<BufferHandle>) -> HostResult<u8> {
        let (index, _) = self.buffer_range(&buffer, 0, 0)?;
        Ok(self.state()?.tab_width(index) as u8)
    }

    fn indent(&mut self, buffer: Resource<BufferHandle>) -> HostResult<settings::Indentation> {
        let (index, _) = self.buffer_range(&buffer, 0, 0)?;
        Ok(match self.state()?.indent(index) {
            Indent::Spaces(n) => settings::Indentation::Spaces(n),
            Indent::Tab => settings::Indentation::Tab,
        })
    }

    fn scroll_margin(&mut self) -> HostResult<u32> {
        Ok(u32::from(self.state()?.settings.scroll_margin))
    }

    fn set_tab_width(
        &mut self,
        buffer: Resource<BufferHandle>,
        width: Option<u8>,
    ) -> HostResult<Result<(), String>> {
        let (index, _) = self.buffer_range(&buffer, 0, 0)?;
        let owner = self.plugin;
        Ok(self
            .state()?
            .set_tab_width(index, owner, width.map(u16::from)))
    }

    fn set_indent(
        &mut self,
        buffer: Resource<BufferHandle>,
        indent: Option<settings::Indentation>,
    ) -> HostResult<Result<(), String>> {
        let (index, _) = self.buffer_range(&buffer, 0, 0)?;
        let owner = self.plugin;
        let indent = indent.map(|indent| match indent {
            settings::Indentation::Spaces(n) => Indent::Spaces(n),
            settings::Indentation::Tab => Indent::Tab,
        });
        Ok(self.state()?.set_indent(index, owner, indent))
    }
}

impl wit_ui::Host for PluginData {
    fn set_status(
        &mut self,
        id: String,
        side: wit_ui::Side,
        priority: i32,
        content: Vec<wit::Span>,
    ) -> HostResult<()> {
        let owner = self.plugin;
        let status = &mut self.state()?.status;
        status.retain(|item| !(item.owner == owner && item.id == id));
        status.push(StatusItem {
            owner,
            id,
            side: match side {
                wit_ui::Side::Left => Side::Left,
                wit_ui::Side::Right => Side::Right,
            },
            priority,
            content: styled_line(content),
        });
        Ok(())
    }

    fn remove_status(&mut self, id: String) -> HostResult<()> {
        let owner = self.plugin;
        self.state()?
            .status
            .retain(|item| !(item.owner == owner && item.id == id));
        Ok(())
    }

    fn show_message(&mut self, text: String) -> HostResult<()> {
        self.state()?.message = Some(text);
        Ok(())
    }

    fn set_decorations(
        &mut self,
        buffer: Resource<BufferHandle>,
        namespace: String,
        decorations: Vec<wit_ui::Decoration>,
    ) -> HostResult<()> {
        let owner = self.plugin;
        let offset = |o: u64| usize::try_from(o).unwrap_or(usize::MAX);
        self.buffer(&buffer)?.set_decorations(
            owner,
            &namespace,
            decorations
                .into_iter()
                .map(|d| (offset(d.start)..offset(d.end), d.style)),
        );
        Ok(())
    }

    fn set_notes(
        &mut self,
        buffer: Resource<BufferHandle>,
        namespace: String,
        notes: Vec<wit_ui::Note>,
    ) -> HostResult<()> {
        let owner = self.plugin;
        let offset = |o: u64| usize::try_from(o).unwrap_or(usize::MAX);
        self.buffer(&buffer)?.set_notes(
            owner,
            &namespace,
            notes.into_iter().map(|n| (offset(n.at), n.text, n.style)),
        );
        Ok(())
    }
}

impl wit_ui::HostPopup for PluginData {
    fn new(
        &mut self,
        anchor: wit_ui::PopupAnchor,
        lines: Vec<Vec<wit::Span>>,
    ) -> HostResult<Resource<PopupHandle>> {
        let owner = self.plugin;
        let state = self.state()?;
        let anchor = match anchor {
            wit_ui::PopupAnchor::Position(offset) => PopupAnchor::Position {
                buffer: state.view.buffer,
                offset: usize::try_from(offset).unwrap_or(usize::MAX),
            },
            wit_ui::PopupAnchor::Corner => PopupAnchor::Corner,
        };
        state.last_popup_id += 1;
        let id = state.last_popup_id;
        state.popups.push(Popup {
            id,
            owner,
            anchor,
            lines: lines.into_iter().map(styled_line).collect(),
        });
        Ok(Resource::new_own(id))
    }

    fn update(
        &mut self,
        popup: Resource<PopupHandle>,
        lines: Vec<Vec<wit::Span>>,
    ) -> HostResult<()> {
        let popup = self
            .state()?
            .popups
            .iter_mut()
            .find(|p| p.id == popup.rep())
            .ok_or_else(|| wasmtime::Error::msg("the popup is closed"))?;
        popup.lines = lines.into_iter().map(styled_line).collect();
        Ok(())
    }

    fn drop(&mut self, popup: Resource<PopupHandle>) -> HostResult<()> {
        // Outside a call, the plugin is being stopped and its popups are
        // removed anyway.
        if let Some(state) = self.state.as_mut() {
            state.popups.retain(|p| p.id != popup.rep());
        }
        Ok(())
    }
}

impl wit_ui::HostPanel for PluginData {
    fn new(&mut self, lines: Vec<Vec<wit::Span>>) -> HostResult<Resource<PanelHandle>> {
        let owner = self.plugin;
        let state = self.state()?;
        state.last_panel_id += 1;
        let id = state.last_panel_id;
        state.panels.push(Panel {
            id,
            owner,
            lines: lines.into_iter().map(styled_line).collect(),
            cursor: None,
        });
        Ok(Resource::new_own(id))
    }

    fn update(
        &mut self,
        panel: Resource<PanelHandle>,
        lines: Vec<Vec<wit::Span>>,
    ) -> HostResult<()> {
        self.panel(&panel)?.lines = lines.into_iter().map(styled_line).collect();
        Ok(())
    }

    fn set_cursor(
        &mut self,
        panel: Resource<PanelHandle>,
        cursor: Option<(u32, u32)>,
    ) -> HostResult<()> {
        self.panel(&panel)?.cursor = cursor;
        Ok(())
    }

    fn drop(&mut self, panel: Resource<PanelHandle>) -> HostResult<()> {
        // Outside a call, the plugin is being stopped and its panels are
        // removed anyway.
        if let Some(state) = self.state.as_mut() {
            state.panels.retain(|p| p.id != panel.rep());
        }
        Ok(())
    }
}

impl wit_prompt::HostLine for PluginData {
    fn new(&mut self, label: String) -> HostResult<Resource<PromptHandle>> {
        let owner = self.plugin;
        let state = self.state()?;
        state.last_prompt_id += 1;
        let id = state.last_prompt_id;
        state.prompts.push(Prompt::new(id, owner, label));
        Ok(Resource::new_own(id))
    }

    fn id(&mut self, prompt: Resource<PromptHandle>) -> HostResult<u64> {
        Ok(u64::from(prompt.rep()))
    }

    fn text(&mut self, prompt: Resource<PromptHandle>) -> HostResult<String> {
        Ok(self.prompt(&prompt)?.text.clone())
    }

    fn cursor(&mut self, prompt: Resource<PromptHandle>) -> HostResult<u32> {
        Ok(self.prompt(&prompt)?.cursor as u32)
    }

    fn set(&mut self, prompt: Resource<PromptHandle>, text: String, cursor: u32) -> HostResult<()> {
        self.prompt(&prompt)?.set(text, cursor as usize);
        Ok(())
    }

    fn set_label(&mut self, prompt: Resource<PromptHandle>, label: String) -> HostResult<()> {
        self.prompt(&prompt)?.label = label;
        Ok(())
    }

    fn set_hint(&mut self, prompt: Resource<PromptHandle>, hint: String) -> HostResult<()> {
        self.prompt(&prompt)?.hint = hint;
        Ok(())
    }

    fn drop(&mut self, prompt: Resource<PromptHandle>) -> HostResult<()> {
        // Outside a call, the plugin is being stopped and its prompts are
        // removed anyway.
        if let Some(state) = self.state.as_mut() {
            state.prompts.retain(|p| p.id != prompt.rep());
        }
        Ok(())
    }
}

impl wit_prompt::HostChoices for PluginData {
    fn new(&mut self, actions: Vec<wit_prompt::Action>) -> HostResult<Resource<ChoicesHandle>> {
        let owner = self.plugin;
        let state = self.state()?;
        state.last_prompt_id += 1;
        let id = state.last_prompt_id;
        let actions = actions.into_iter().map(action).collect();
        state.choices.push(Choices { id, owner, actions });
        Ok(Resource::new_own(id))
    }

    fn id(&mut self, choices: Resource<ChoicesHandle>) -> HostResult<u64> {
        Ok(u64::from(choices.rep()))
    }

    fn drop(&mut self, choices: Resource<ChoicesHandle>) -> HostResult<()> {
        // Outside a call, the plugin is being stopped and its choices are
        // removed anyway.
        if let Some(state) = self.state.as_mut() {
            state.choices.retain(|c| c.id != choices.rep());
        }
        Ok(())
    }
}

fn action(action: wit_prompt::Action) -> Action {
    match action {
        wit_prompt::Action::Accept => Action::Accept,
        wit_prompt::Action::Cancel => Action::Cancel,
        wit_prompt::Action::Next => Action::Next,
        wit_prompt::Action::Previous => Action::Previous,
        wit_prompt::Action::PageNext => Action::PageNext,
        wit_prompt::Action::PagePrevious => Action::PagePrevious,
        wit_prompt::Action::Complete => Action::Complete,
        wit_prompt::Action::CompleteBack => Action::CompleteBack,
    }
}

fn wit_action(action: Action) -> wit_prompt::Action {
    match action {
        Action::Accept => wit_prompt::Action::Accept,
        Action::Cancel => wit_prompt::Action::Cancel,
        Action::Next => wit_prompt::Action::Next,
        Action::Previous => wit_prompt::Action::Previous,
        Action::PageNext => wit_prompt::Action::PageNext,
        Action::PagePrevious => wit_prompt::Action::PagePrevious,
        Action::Complete => wit_prompt::Action::Complete,
        Action::CompleteBack => wit_prompt::Action::CompleteBack,
    }
}

impl wit_prompt::Host for PluginData {
    fn active(&mut self) -> HostResult<Option<wit_prompt::State>> {
        let caller = self.plugin;
        Ok(self.state()?.prompts.last().map(|p| wit_prompt::State {
            id: u64::from(p.id),
            label: p.label.clone(),
            text: p.text.clone(),
            cursor: p.cursor as u32,
            mine: p.owner == caller,
        }))
    }

    fn edit(&mut self, text: String, cursor: u32) -> HostResult<()> {
        let state = self.state()?;
        let Some(prompt) = state.prompts.last_mut() else {
            return Ok(());
        };
        prompt.set(text, cursor as usize);
        let event = Event::PromptChanged {
            prompt: prompt.id,
            text: prompt.text.clone(),
            cursor: prompt.cursor,
        };
        let owner = prompt.owner;
        state.push_event(Some(owner), event);
        Ok(())
    }

    fn offered(&mut self) -> HostResult<Option<wit_prompt::Offer>> {
        let state = self.state()?;
        if !state.prompts.is_empty() {
            return Ok(None);
        }
        Ok(state.choices.last().map(|c| wit_prompt::Offer {
            id: u64::from(c.id),
            actions: c.actions.iter().copied().map(wit_action).collect(),
        }))
    }

    fn act(&mut self, wanted: wit_prompt::Action) -> HostResult<()> {
        let state = self.state()?;
        let (id, owner) = match (state.prompts.last(), state.choices.last()) {
            (Some(prompt), _) => (prompt.id, prompt.owner),
            (None, Some(choices)) => {
                state.choices_acted = true;
                (choices.id, choices.owner)
            }
            (None, None) => return Ok(()),
        };
        let event = Event::PromptAction {
            prompt: id,
            action: action(wanted),
        };
        state.push_event(Some(owner), event);
        Ok(())
    }
}

impl PluginData {
    fn prompt(&mut self, handle: &Resource<PromptHandle>) -> HostResult<&mut Prompt> {
        self.state()?
            .prompts
            .iter_mut()
            .find(|prompt| prompt.id == handle.rep())
            .ok_or_else(|| wasmtime::Error::msg("the prompt is closed"))
    }

    fn panel(&mut self, handle: &Resource<PanelHandle>) -> HostResult<&mut Panel> {
        self.state()?
            .panels
            .iter_mut()
            .find(|panel| panel.id == handle.rep())
            .ok_or_else(|| wasmtime::Error::msg("the panel is closed"))
    }
}

fn styled_line(spans: Vec<wit::Span>) -> StyledLine {
    spans
        .into_iter()
        .map(|span| Span {
            text: span.text,
            style: span.style,
        })
        .collect()
}

pub(crate) fn key_event(key: KeyEvent) -> wit::KeyEvent {
    let code = match key.code {
        KeyCode::Char(c) => wit::KeyCode::Char(c),
        KeyCode::Enter => wit::KeyCode::Enter,
        KeyCode::Escape => wit::KeyCode::Escape,
        KeyCode::Tab => wit::KeyCode::Tab,
        KeyCode::Backspace => wit::KeyCode::Backspace,
        KeyCode::Delete => wit::KeyCode::Delete,
        KeyCode::Insert => wit::KeyCode::Insert,
        KeyCode::Up => wit::KeyCode::Up,
        KeyCode::Down => wit::KeyCode::Down,
        KeyCode::Left => wit::KeyCode::Left,
        KeyCode::Right => wit::KeyCode::Right,
        KeyCode::Home => wit::KeyCode::Home,
        KeyCode::End => wit::KeyCode::End,
        KeyCode::PageUp => wit::KeyCode::PageUp,
        KeyCode::PageDown => wit::KeyCode::PageDown,
        KeyCode::F(n) => wit::KeyCode::F(n),
    };
    let mut modifiers = wit::Modifiers::empty();
    for (on, flag) in [
        (key.modifiers.ctrl, wit::Modifiers::CTRL),
        (key.modifiers.alt, wit::Modifiers::ALT),
        (key.modifiers.shift, wit::Modifiers::SHIFT),
        (key.modifiers.super_, wit::Modifiers::SUPER),
    ] {
        if on {
            modifiers |= flag;
        }
    }
    wit::KeyEvent { code, modifiers }
}

fn range(start: usize, end: usize) -> wit::Range {
    wit::Range {
        start: start as u64,
        end: end as u64,
    }
}

fn wit_mode(mode: &Mode) -> input::Mode {
    input::Mode {
        base: mode.base.clone(),
        name: mode.name.clone(),
        typing: mode.typing,
    }
}

impl PluginData {
    /// Undoes or redoes a step of the view's buffer, and returns where the
    /// text changed first.
    fn undo_or_redo(
        &mut self,
        view: &Resource<ViewHandle>,
        undo: bool,
    ) -> HostResult<Option<wit::Range>> {
        let state = self.view_state(view)?;
        let index = state.view.buffer;
        let buffer = &mut state.buffers[index];
        if !buffer.editable() {
            state.message = Some(format!("{} is read-only", buffer.name()));
            return Ok(None);
        }
        let Some(change) = (if undo { buffer.undo() } else { buffer.redo() }) else {
            return Ok(None);
        };
        state.sync_views(index, &change.changes);
        let first = change.first_range();
        state.view.selection = change.selection;
        Ok(first.map(|r| range(r.start, r.end)))
    }
}

fn pos(offset: u64) -> Result<usize, Error> {
    usize::try_from(offset).map_err(|_| Error::InvalidPosition(usize::MAX))
}

fn to_wit_selection(selection: &Selection) -> wit::Selection {
    wit::Selection {
        ranges: selection
            .ranges()
            .iter()
            .map(|range| wit::SelRange {
                anchor: range.anchor as u64,
                head: range.head as u64,
            })
            .collect(),
        primary: selection.primary_index() as u32,
    }
}

fn from_wit_selection(selection: wit::Selection) -> Result<(Vec<Range>, usize), Error> {
    let ranges = selection
        .ranges
        .into_iter()
        .map(|range| Ok(Range::new(pos(range.anchor)?, pos(range.head)?)))
        .collect::<Result<_, Error>>()?;
    Ok((ranges, selection.primary as usize))
}

/// Errors a plugin can act on become WIT errors. Anything else is a bug in
/// the host and traps.
fn wit_result<T>(result: Result<T, Error>) -> HostResult<Result<T, wit::Error>> {
    match result {
        Ok(value) => Ok(Ok(value)),
        Err(err) => Ok(Err(match err {
            Error::StaleVersion { .. } => wit::Error::StaleVersion,
            Error::InvalidPosition(_) => wit::Error::InvalidPosition,
            Error::OverlappingEdits => wit::Error::OverlappingEdits,
            Error::InvalidSelection => wit::Error::InvalidSelection,
            Error::InvalidPattern(message) => wit::Error::InvalidPattern(message),
            Error::ReadOnly => wit::Error::ReadOnly,
            other => return Err(wasmtime::Error::msg(other.to_string())),
        })),
    }
}
