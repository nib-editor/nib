//! Host side of `nib:plugin`: what plugins can call.

use std::ops::Range as ByteRange;
use std::time::{Duration, Instant};

use wasmtime::component::Resource;
use wasmtime_wasi::{WasiCtxView, WasiView};

use super::PluginData;
use crate::buffer::Buffer;
use crate::editor::{CORE_COMMANDS, ScrollAmount, State};
use crate::events::{Command, Event, Timer};
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
            "nib:plugin/editor.buffer": super::BufferHandle,
            "nib:plugin/editor.view": super::ViewHandle,
            "nib:plugin/ui.panel": super::PanelHandle,
            "nib:plugin/ui.popup": super::PopupHandle,
            "nib:plugin/prompt.line": super::PromptHandle,
            "nib:plugin/prompt.choices": super::ChoicesHandle,
            "nib:plugin/process.child": super::ChildHandle,
        },
    });
}

use bindings::nib::plugin::{
    clipboard, commands, editor, events, files, input, process, prompt as wit_prompt, settings,
    syntax, timers, types as wit, ui as wit_ui,
};

/// A buffer as seen by a plugin. The resource's rep is the buffer index.
pub struct BufferHandle;

/// A view as seen by a plugin. There is one view for now, with rep 0.
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
    fn active_view(&mut self) -> HostResult<Resource<ViewHandle>> {
        Ok(Resource::new_own(0))
    }

    fn buffers(&mut self) -> HostResult<Vec<Resource<BufferHandle>>> {
        let buffers = &self.state()?.buffers;
        Ok((0..buffers.len())
            .filter(|&i| !buffers[i].is_closed())
            .map(|i| Resource::new_own(i as u32))
            .collect())
    }

    fn working_directory(&mut self) -> HostResult<String> {
        let dir = std::env::current_dir().unwrap_or_default();
        Ok(dir.to_string_lossy().into_owned())
    }
}

impl editor::HostBuffer for PluginData {
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
    ) -> HostResult<Result<String, editor::Error>> {
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
    ) -> HostResult<Result<u64, editor::Error>> {
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
    ) -> HostResult<Result<u64, editor::Error>> {
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
    ) -> HostResult<Result<u64, editor::Error>> {
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

    fn find(
        &mut self,
        buffer: Resource<BufferHandle>,
        pattern: String,
        start: u64,
        backward: bool,
    ) -> HostResult<Result<Option<(u64, u64)>, editor::Error>> {
        let buffer = self.buffer(&buffer)?;
        wit_result(
            pos(start)
                .and_then(|start| buffer.find(&pattern, start, backward))
                .map(|found| found.map(|(s, e)| (s as u64, e as u64))),
        )
    }

    fn find_all(
        &mut self,
        buffer: Resource<BufferHandle>,
        pattern: String,
        start: u64,
        end: u64,
    ) -> HostResult<Result<Vec<(u64, u64)>, editor::Error>> {
        let buffer = self.buffer(&buffer)?;
        wit_result(
            pos(start)
                .and_then(|start| buffer.find_all(&pattern, start, pos(end)?))
                .map(|found| {
                    found
                        .into_iter()
                        .map(|(s, e)| (s as u64, e as u64))
                        .collect()
                }),
        )
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

impl editor::HostView for PluginData {
    fn buffer(&mut self, view: Resource<ViewHandle>) -> HostResult<Resource<BufferHandle>> {
        let state = self.view_state(&view)?;
        Ok(Resource::new_own(state.view.buffer as u32))
    }

    fn selection(&mut self, view: Resource<ViewHandle>) -> HostResult<wit::Selection> {
        let state = self.view_state(&view)?;
        Ok(to_wit_selection(&state.view.selection))
    }

    fn set_selection(
        &mut self,
        view: Resource<ViewHandle>,
        selection: wit::Selection,
    ) -> HostResult<Result<(), editor::Error>> {
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
    ) -> HostResult<Result<(), editor::Error>> {
        let state = self.view_state(&view)?;
        let buffer = &mut state.buffers[state.view.buffer];
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

    fn undo(&mut self, view: Resource<ViewHandle>) -> HostResult<bool> {
        let state = self.view_state(&view)?;
        let index = state.view.buffer;
        let change = state.buffers[index].undo();
        Ok(change
            .map(|change| {
                state.sync_views(index, &change.changes);
                state.view.selection = change.selection;
            })
            .is_some())
    }

    fn redo(&mut self, view: Resource<ViewHandle>) -> HostResult<bool> {
        let state = self.view_state(&view)?;
        let index = state.view.buffer;
        let change = state.buffers[index].redo();
        Ok(change
            .map(|change| {
                state.sync_views(index, &change.changes);
                state.view.selection = change.selection;
            })
            .is_some())
    }

    fn move_vertically(
        &mut self,
        view: Resource<ViewHandle>,
        at: u64,
        lines: i32,
        column: Option<u32>,
    ) -> HostResult<Result<(u64, u32), editor::Error>> {
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
        amount: editor::ScrollAmount,
    ) -> HostResult<i32> {
        let state = self.view_state(&view)?;
        Ok(state.scroll(match amount {
            editor::ScrollAmount::Lines(n) => ScrollAmount::Lines(n),
            editor::ScrollAmount::HalfPage(n) => ScrollAmount::HalfPage(n),
            editor::ScrollAmount::Page(n) => ScrollAmount::Page(n),
        }))
    }

    fn visible_range(&mut self, view: Resource<ViewHandle>) -> HostResult<(u64, u64)> {
        let (start, end) = self.view_state(&view)?.visible_range();
        Ok((start as u64, end as u64))
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
    ) -> HostResult<Vec<(u64, u64)>> {
        let (index, range) = self.buffer_range(&buffer, start, end)?;
        let state = self.state()?;
        let found = state.with_syntax(index, |languages, syntax, text| {
            languages.captures_in(syntax, &query, &capture, text, range)
        });
        match found {
            Some(Ok(found)) => Ok(found
                .into_iter()
                .map(|r| (r.start as u64, r.end as u64))
                .collect()),
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
    fn get(&mut self, key: String) -> HostResult<Option<String>> {
        let state = self.state()?;
        Ok(state.setting_json(state.view.buffer, &key))
    }

    fn get_for(
        &mut self,
        buffer: Resource<BufferHandle>,
        key: String,
    ) -> HostResult<Option<String>> {
        let (index, _) = self.buffer_range(&buffer, 0, 0)?;
        Ok(self.state()?.setting_json(index, &key))
    }

    fn set_for(
        &mut self,
        buffer: Resource<BufferHandle>,
        key: String,
        value: Option<String>,
    ) -> HostResult<Result<(), String>> {
        let (index, _) = self.buffer_range(&buffer, 0, 0)?;
        let owner = self.plugin;
        Ok(self
            .state()?
            .set_setting(index, owner, &key, value.as_deref()))
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
fn wit_result<T>(result: Result<T, Error>) -> HostResult<Result<T, editor::Error>> {
    match result {
        Ok(value) => Ok(Ok(value)),
        Err(err) => Ok(Err(match err {
            Error::StaleVersion { .. } => editor::Error::StaleVersion,
            Error::InvalidPosition(_) => editor::Error::InvalidPosition,
            Error::OverlappingEdits => editor::Error::OverlappingEdits,
            Error::InvalidSelection => editor::Error::InvalidSelection,
            Error::InvalidPattern(message) => editor::Error::InvalidPattern(message),
            other => return Err(wasmtime::Error::msg(other.to_string())),
        })),
    }
}
