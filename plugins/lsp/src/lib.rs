//! Language Server Protocol client (docs/lsp.md). It starts a server for a
//! language when a file of it is opened, keeps the server's copy of each
//! file in sync, and shows diagnostics, hovers, definitions, and
//! completions. Servers answer through process events, so nothing here
//! waits for them.

mod complete;
mod position;
mod rpc;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use nib_plugin::exports::nib::plugin::guest::{Guest, KeyResult};
use nib_plugin::nib::plugin::buffer::{self, Buffer};
use nib_plugin::nib::plugin::events::{BufferChange, Event};
use nib_plugin::nib::plugin::process::{self, Child, Stream};
use nib_plugin::nib::plugin::prompt::{Action, Choices};
use nib_plugin::nib::plugin::types::{Edit, KeyEvent, SelRange, Selection, Span, UndoMode};
use nib_plugin::nib::plugin::ui::{self, Decoration, Note, Popup, PopupAnchor, Side};
use nib_plugin::nib::plugin::view::{self, Direction, Toward};
use nib_plugin::nib::plugin::{commands, syntax, timers};
use nib_plugin::nib::plugin::{editor, input};
use serde_json::{Value, json};

/// Servers used unless `[settings.servers]` says otherwise.
const DEFAULT_SERVERS: [(&str, &[&str]); 3] = [
    ("rust", &["rust-analyzer"]),
    ("go", &["gopls"]),
    ("python", &["pyright-langserver", "--stdio"]),
];

const SEVERITIES: [&str; 4] = ["error", "warning", "info", "hint"];

/// Longest hover shown, in lines and in chars per line.
const HOVER_LINES: usize = 20;
const HOVER_WIDTH: usize = 100;
/// How much of a server's error output is kept, to say why it stopped.
const STDERR_KEPT: usize = 4096;
/// Completions shown at once.
const COMPLETION_ROWS: usize = 10;
/// How long typing pauses before completions are asked for on their own.
const COMPLETION_DELAY_MS: u32 = 150;
/// How long typing pauses before diagnostics are asked for again, from
/// servers that give them when asked.
const PULL_DELAY_MS: u32 = 200;
/// The buffer that lists diagnostics.
const LIST_NAME: &str = "*diagnostics*";

struct Server {
    language: String,
    child: Child,
    /// Output not yet split into messages.
    output: Vec<u8>,
    /// The end of its error output.
    stderr: Vec<u8>,
    next_id: u64,
    pending: HashMap<u64, Request>,
    /// Initialized, so documents can be opened.
    ready: bool,
    /// Positions count bytes, as the core's do.
    utf8: bool,
    /// Takes changes as edits rather than whole texts.
    incremental: bool,
    /// Gives diagnostics when asked (`textDocument/diagnostic`), besides
    /// sending them.
    pull: bool,
    /// Documents open on the server, by URI.
    opened: HashSet<String>,
    /// Documents to open once it is initialized.
    waiting: Vec<String>,
}

enum Request {
    Initialize,
    Hover { uri: String, offset: u64 },
    Definition,
    Completion { uri: String, start: u64, auto: bool },
    Diagnostics { uri: String },
}

/// Diagnostics of one file from one way of getting them, and whether their
/// positions count bytes.
struct Diagnostics {
    utf8: bool,
    items: Vec<Value>,
}

/// The `*diagnostics*` buffer, and where each of its lines points.
struct List {
    buffer: Buffer,
    places: Vec<Place>,
}

struct Place {
    uri: String,
    line: u32,
    character: u32,
    utf8: bool,
}

struct Completion {
    popup: Popup,
    /// What keys can do to them, as the base in use maps its keys.
    choices: Choices,
    uri: String,
    /// Where the word being completed starts.
    start: u64,
    language: String,
    items: Vec<complete::Item>,
    /// Indices into `items` of the ones matching what is typed.
    shown: Vec<usize>,
    selected: usize,
}

struct Lsp {
    /// The command for each language.
    commands: HashMap<String, Vec<String>>,
    servers: Vec<Server>,
    /// Languages whose server could not start or stopped; not tried again.
    failed: HashSet<String>,
    cwd: String,
    /// Diagnostics by URI, as servers sent them and as they answered when
    /// asked; shown together.
    pushed: HashMap<String, Diagnostics>,
    pulled: HashMap<String, Diagnostics>,
    /// Diagnostics per severity, by URI, for the status line.
    counts: HashMap<String, [usize; 4]>,
    /// Files to ask diagnostics for once typing pauses, and the timer.
    pull_waiting: HashSet<String>,
    pull_timer: Option<u64>,
    /// Closed by the next key, as its choices take none.
    hover: Option<(Popup, Choices)>,
    completion: Option<Completion>,
    /// The keymap is in insert mode, where completions come on their own.
    inserting: bool,
    /// The timer that asks for completions after a pause in typing.
    timer: Option<u64>,
    list: Option<List>,
}

thread_local! {
    static LSP: RefCell<Option<Lsp>> = const { RefCell::new(None) };
}

fn with_lsp<R>(f: impl FnOnce(&mut Lsp) -> R) -> R {
    LSP.with_borrow_mut(|lsp| f(lsp.as_mut().expect("initialized in init")))
}

struct Plugin;

impl Guest for Plugin {
    fn init(config: String) -> Result<(), String> {
        let config: Value = serde_json::from_str(&config).map_err(|err| err.to_string())?;
        let mut servers: HashMap<String, Vec<String>> = DEFAULT_SERVERS
            .iter()
            .map(|(language, command)| {
                let command = command.iter().map(|s| s.to_string()).collect();
                (language.to_string(), command)
            })
            .collect();
        if let Some(configured) = config["servers"].as_object() {
            for (language, server) in configured {
                let command = match &server["command"] {
                    Value::String(line) => line.split_whitespace().map(String::from).collect(),
                    Value::Array(words) => words
                        .iter()
                        .map(|w| w.as_str().map(String::from))
                        .collect::<Option<Vec<_>>>()
                        .ok_or_else(|| format!("servers.{language}.command: strings only"))?,
                    _ => return Err(format!("servers.{language} needs a command")),
                };
                servers.insert(language.clone(), command);
            }
        }
        commands::register(
            "hover",
            "Show what the language server says about the cursor",
        );
        commands::register("definition", "Go to the definition at the cursor");
        commands::register("complete", "Show completions for the word at the cursor");
        commands::register("status", "Say which language servers run");
        commands::register("diagnostics", "List the diagnostics of open files");
        commands::register("goto-diagnostic", "Go to the diagnostic on this line");
        commands::register("close-diagnostics", "Close the list of diagnostics");
        LSP.with_borrow_mut(|lsp| {
            *lsp = Some(Lsp {
                commands: servers,
                servers: Vec::new(),
                failed: HashSet::new(),
                cwd: editor::working_directory(),
                pushed: HashMap::new(),
                pulled: HashMap::new(),
                counts: HashMap::new(),
                pull_waiting: HashSet::new(),
                pull_timer: None,
                hover: None,
                completion: None,
                inserting: input::current_mode().typing,
                timer: None,
                list: None,
            })
        });
        Ok(())
    }

    fn handle_key(_ev: KeyEvent) -> KeyResult {
        KeyResult::Pass
    }

    fn handle_paste(_text: String) -> KeyResult {
        KeyResult::Pass
    }

    fn run_command(name: String, _args: String) -> Result<String, String> {
        with_lsp(|lsp| match name.as_str() {
            "hover" => lsp
                .ask_at_cursor("textDocument/hover", true)
                .map(|()| "null".into()),
            "definition" => lsp
                .ask_at_cursor("textDocument/definition", false)
                .map(|()| "null".into()),
            "complete" => lsp.complete(false).map(|()| "null".into()),
            "status" => Ok(lsp.status()),
            "diagnostics" => {
                lsp.open_list();
                Ok("null".into())
            }
            "goto-diagnostic" => lsp.goto_listed().map(|()| "null".into()),
            "close-diagnostics" => {
                lsp.close_list();
                Ok("null".into())
            }
            _ => Err(format!("no command {name}")),
        })
    }

    fn on_event(ev: Event) {
        with_lsp(|lsp| match ev {
            Event::BufferOpened(buffer) => lsp.opened(&buffer),
            Event::BufferChanged(change) => {
                lsp.changed(&change);
                lsp.typed(&change);
            }
            Event::BufferSaved(buffer) => lsp.saved(&buffer),
            Event::ProcessOutput(output) => {
                let Some(i) = lsp.server_of(output.process) else {
                    return;
                };
                let server = &mut lsp.servers[i];
                if output.stream == Stream::Stderr {
                    server.stderr.extend(output.data);
                    let excess = server.stderr.len().saturating_sub(STDERR_KEPT);
                    server.stderr.drain(..excess);
                    return;
                }
                server.output.extend(output.data);
                for message in rpc::decode(&mut lsp.servers[i].output) {
                    lsp.receive(i, message);
                }
            }
            Event::ProcessExit(exit) => {
                let Some(i) = lsp.server_of(exit.process) else {
                    return;
                };
                let server = lsp.servers.remove(i);
                let stderr = String::from_utf8_lossy(&server.stderr);
                let reason = stderr.lines().find(|line| !line.trim().is_empty());
                let command = lsp.commands[&server.language].join(" ");
                ui::show_message(&match reason {
                    Some(reason) => format!("lsp: {command} stopped: {}", reason.trim()),
                    None => format!("lsp: {command} stopped"),
                });
                lsp.failed.insert(server.language);
            }
            Event::BufferClosed(closed) => match closed.path {
                Some(path) => lsp.closed(&path),
                None if closed.name == LIST_NAME => lsp.list = None,
                None => {}
            },
            // Whichever base runs: completions come while typing.
            Event::ModeChanged(mode) => {
                lsp.inserting = mode.typing;
                if !lsp.inserting {
                    lsp.close_popups();
                }
            }
            Event::Timer(id) if lsp.pull_timer == Some(id) => {
                lsp.pull_timer = None;
                for uri in std::mem::take(&mut lsp.pull_waiting) {
                    lsp.pull(&uri);
                }
            }
            Event::Timer(id) => {
                if lsp.timer == Some(id) {
                    lsp.timer = None;
                    if lsp.inserting && lsp.completion.is_none() && lsp.worth_completing() {
                        let _ = lsp.complete(true);
                    }
                }
            }
            // A key the base turned into an action, or any other key, which
            // closes what is shown and still does what it would have done.
            Event::PromptAction(act) => lsp.acted(act.id, act.action),
            Event::SyntaxUpdated(_)
            | Event::Custom(_)
            | Event::FilesListed(_)
            | Event::PromptChanged(_) => {}
        })
    }
}

impl Lsp {
    fn server_of(&self, process: u64) -> Option<usize> {
        self.servers.iter().position(|s| s.child.id() == process)
    }

    /// The running server for `buffer`'s language, started if need be.
    fn server_for(&mut self, buffer: &Buffer) -> Option<usize> {
        let language = syntax::language(buffer)?;
        if let Some(i) = self.servers.iter().position(|s| s.language == language) {
            return Some(i);
        }
        if self.failed.contains(&language) {
            return None;
        }
        let (program, args) = self.commands.get(&language)?.split_first()?;
        let child = match process::spawn(program, args, None) {
            Ok(child) => child,
            Err(err) => {
                ui::show_message(&format!("lsp: {err}"));
                self.failed.insert(language);
                return None;
            }
        };
        self.servers.push(Server {
            language,
            child,
            output: Vec::new(),
            stderr: Vec::new(),
            next_id: 0,
            pending: HashMap::new(),
            ready: false,
            utf8: false,
            incremental: false,
            pull: false,
            opened: HashSet::new(),
            waiting: Vec::new(),
        });
        let i = self.servers.len() - 1;
        let root = path_to_uri(&self.cwd);
        let name = self.cwd.rsplit(['/', '\\']).next().unwrap_or("root");
        let params = json!({
            "processId": null,
            "clientInfo": {"name": "nib"},
            "rootUri": root,
            "rootPath": self.cwd,
            "workspaceFolders": [{"uri": root, "name": name}],
            "capabilities": {
                "general": {"positionEncodings": ["utf-8", "utf-16"]},
                "textDocument": {
                    "synchronization": {"didSave": true},
                    "hover": {"contentFormat": ["plaintext", "markdown"]},
                    "definition": {"linkSupport": true},
                    "publishDiagnostics": {},
                    "diagnostic": {"dynamicRegistration": false},
                },
                "workspace": {"workspaceFolders": true, "configuration": true},
                "window": {"workDoneProgress": false},
            },
        });
        self.servers[i].request("initialize", params, Request::Initialize);
        Some(i)
    }

    fn opened(&mut self, buffer: &Buffer) {
        let Some(uri) = self.uri(buffer) else {
            return;
        };
        let Some(i) = self.server_for(buffer) else {
            return;
        };
        let server = &mut self.servers[i];
        if server.ready {
            server.open(&uri, buffer);
        } else if !server.waiting.contains(&uri) {
            server.waiting.push(uri);
        }
    }

    /// A buffer closed: servers that have it open close it too, and what
    /// was known of it goes.
    fn closed(&mut self, path: &str) {
        let uri = self.uri_of(path);
        for server in &mut self.servers {
            server.waiting.retain(|u| *u != uri);
            if server.opened.remove(&uri) {
                server.notify(
                    "textDocument/didClose",
                    json!({"textDocument": {"uri": uri}}),
                );
            }
        }
        self.pushed.remove(&uri);
        self.pulled.remove(&uri);
        self.counts.remove(&uri);
        self.pull_waiting.remove(&uri);
        self.write_list();
    }

    fn changed(&mut self, change: &BufferChange) {
        let buffer = &change.buffer;
        let Some((i, uri)) = self.open_document(buffer) else {
            return;
        };
        let server = &mut self.servers[i];
        let changes: Vec<Value> = if server.utf8 && server.incremental {
            change
                .changes
                .iter()
                .map(|c| {
                    json!({
                        "range": {
                            "start": {"line": c.start_line, "character": c.start_column},
                            "end": {"line": c.end_line, "character": c.end_column},
                        },
                        "text": c.text,
                    })
                })
                .collect()
        } else {
            // Positions in UTF-16 need the text before the change, which is
            // gone, so send all of it.
            vec![json!({"text": buffer.slice(0, buffer.len()).unwrap_or_default()})]
        };
        let params = json!({
            "textDocument": {"uri": uri, "version": change.version},
            "contentChanges": changes,
        });
        server.notify("textDocument/didChange", params);
        if server.pull {
            self.pull_later(uri);
        }
    }

    /// Asks for `uri`'s diagnostics once typing pauses.
    fn pull_later(&mut self, uri: String) {
        self.pull_waiting.insert(uri);
        if self.pull_timer.is_none() {
            self.pull_timer = Some(timers::set(PULL_DELAY_MS));
        }
    }

    /// Asks the server that has `uri` open for its diagnostics, if it gives
    /// them when asked.
    fn pull(&mut self, uri: &str) {
        let Some(server) = self
            .servers
            .iter_mut()
            .find(|s| s.pull && s.opened.contains(uri))
        else {
            return;
        };
        let params = json!({"textDocument": {"uri": uri}});
        let request = Request::Diagnostics {
            uri: uri.to_string(),
        };
        server.request("textDocument/diagnostic", params, request);
    }

    fn saved(&mut self, buffer: &Buffer) {
        if let Some((i, uri)) = self.open_document(buffer) {
            let params = json!({"textDocument": {"uri": uri}});
            self.servers[i].notify("textDocument/didSave", params);
        }
    }

    /// The server `buffer` is open on, and its URI.
    fn open_document(&self, buffer: &Buffer) -> Option<(usize, String)> {
        let uri = self.uri(buffer)?;
        let i = self.servers.iter().position(|s| s.opened.contains(&uri))?;
        Some((i, uri))
    }

    /// Sends a hover or definition request for the primary cursor.
    fn ask_at_cursor(&mut self, method: &str, hover: bool) -> Result<(), String> {
        let view = view::active();
        let buffer = view.buffer();
        let (i, uri) = self
            .open_document(&buffer)
            .ok_or("no language server for this file")?;
        let selection = view.selection();
        let range = selection.ranges[selection.primary as usize];
        let offset = if range.head > range.anchor {
            buffer.prev_grapheme(range.head).unwrap_or(range.head)
        } else {
            range.head
        };
        let server = &mut self.servers[i];
        let (line, character) = position::to_lsp(&buffer, offset, server.utf8);
        let params = json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character},
        });
        let request = if hover {
            Request::Hover { uri, offset }
        } else {
            Request::Definition
        };
        server.request(method, params, request);
        Ok(())
    }

    fn receive(&mut self, i: usize, message: Value) {
        let method = message["method"].as_str();
        let id = message.get("id").cloned();
        match (method, id) {
            // A request from the server: answer it, or it may wait for us.
            (Some(method), Some(id)) => {
                let result = match method {
                    "workspace/configuration" => {
                        let items = message["params"]["items"].as_array().map_or(0, Vec::len);
                        Value::Array(vec![Value::Null; items])
                    }
                    // Its diagnostics may have changed without an edit, such
                    // as after another file changed.
                    "workspace/diagnostic/refresh" => {
                        let open: Vec<String> = self.servers[i].opened.iter().cloned().collect();
                        for uri in open {
                            self.pull_later(uri);
                        }
                        Value::Null
                    }
                    _ => Value::Null,
                };
                self.servers[i].send(&json!({"jsonrpc": "2.0", "id": id, "result": result}));
            }
            (Some(method), None) => self.notification(i, method, &message["params"]),
            (None, Some(id)) => {
                let Some(request) = id
                    .as_u64()
                    .and_then(|id| self.servers[i].pending.remove(&id))
                else {
                    return;
                };
                if let Some(error) = message["error"]["message"].as_str() {
                    ui::show_message(&format!("lsp: {error}"));
                    return;
                }
                self.response(i, request, &message["result"]);
            }
            (None, None) => {}
        }
    }

    fn notification(&mut self, i: usize, method: &str, params: &Value) {
        match method {
            "textDocument/publishDiagnostics" => {
                let Some(uri) = params["uri"].as_str() else {
                    return;
                };
                let diagnostics = Diagnostics {
                    utf8: self.servers[i].utf8,
                    items: params["diagnostics"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default(),
                };
                self.pushed.insert(uri.to_string(), diagnostics);
                self.show_diagnostics(uri);
            }
            // Errors and warnings only; the rest is chatter.
            "window/showMessage" if params["type"].as_u64().is_some_and(|t| t <= 2) => {
                let text = params["message"].as_str().unwrap_or_default();
                let language = &self.servers[i].language;
                ui::show_message(&format!("{language}: {}", first_line(text)));
            }
            _ => {}
        }
    }

    fn response(&mut self, i: usize, request: Request, result: &Value) {
        match request {
            Request::Initialize => {
                let capabilities = &result["capabilities"];
                let server = &mut self.servers[i];
                server.utf8 = capabilities["positionEncoding"] == "utf-8";
                let sync = &capabilities["textDocumentSync"];
                let kind = sync.as_u64().or_else(|| sync["change"].as_u64());
                server.incremental = kind == Some(2);
                server.pull = !capabilities["diagnosticProvider"].is_null();
                server.ready = true;
                server.notify("initialized", json!({}));
                let waiting = std::mem::take(&mut server.waiting);
                for uri in waiting {
                    if let Some(buffer) = self.buffer(&uri) {
                        self.servers[i].open(&uri, &buffer);
                        self.pull(&uri);
                    }
                }
            }
            Request::Hover { uri, offset } => self.show_hover(&uri, offset, result),
            Request::Definition => self.go_to_definition(i, result),
            Request::Completion { uri, start, auto } => {
                let language = self.servers[i].language.clone();
                self.show_completions(uri, start, language, auto, result);
            }
            Request::Diagnostics { uri } => {
                // "unchanged" keeps what was shown.
                if result["kind"] == "full" {
                    let diagnostics = Diagnostics {
                        utf8: self.servers[i].utf8,
                        items: result["items"].as_array().cloned().unwrap_or_default(),
                    };
                    self.pulled.insert(uri.clone(), diagnostics);
                    self.show_diagnostics(&uri);
                }
            }
        }
    }

    /// Shows `uri`'s diagnostics, sent and asked for together.
    fn show_diagnostics(&mut self, uri: &str) {
        let mut counts = [0; 4];
        let mut decorations = Vec::new();
        let mut notes = Vec::new();
        let buffer = self.buffer(uri);
        let all = [self.pushed.get(uri), self.pulled.get(uri)];
        let items = all
            .into_iter()
            .flatten()
            .flat_map(|d| d.items.iter().map(move |item| (d.utf8, item)));
        for (utf8, diagnostic) in items {
            let severity = diagnostic["severity"]
                .as_u64()
                .map_or(0, |s| (s.clamp(1, 4) - 1) as usize);
            counts[severity] += 1;
            let Some(buffer) = &buffer else {
                continue;
            };
            let at = |end: &str| {
                let position = &diagnostic["range"][end];
                let line = position["line"].as_u64().unwrap_or(0) as u32;
                let character = position["character"].as_u64().unwrap_or(0) as u32;
                position::from_lsp(buffer, line, character, utf8)
            };
            let start = at("start");
            // An empty range still marks the char it is at.
            let end = at("end").max(buffer.next_grapheme(start).unwrap_or(start));
            let name = SEVERITIES[severity];
            decorations.push(Decoration {
                start,
                end,
                style: format!("diagnostic.underline.{name}"),
            });
            let message = diagnostic["message"].as_str().unwrap_or_default();
            notes.push(Note {
                at: start,
                text: first_line(message).to_string(),
                style: format!("diagnostic.{name}"),
            });
        }
        if let Some(buffer) = &buffer {
            ui::set_decorations(buffer, "lsp", &decorations);
            ui::set_notes(buffer, "lsp", &notes);
        }
        self.counts.insert(uri.to_string(), counts);
        self.show_counts();
        self.write_list();
    }

    /// Shows the list of diagnostics below the focused view, made if need
    /// be.
    fn open_list(&mut self) {
        if self.list.is_none() {
            let buffer = buffer::create(LIST_NAME);
            let keys = [
                ("ret", "lsp.goto-diagnostic"),
                ("q", "lsp.close-diagnostics"),
            ];
            let keys: Vec<(String, String)> = keys
                .iter()
                .map(|(key, command)| (key.to_string(), command.to_string()))
                .collect();
            buffer
                .set_keys(&keys)
                .expect("keys of this plugin's commands");
            self.list = Some(List {
                buffer,
                places: Vec::new(),
            });
            self.write_list();
        }
        let list = self.list.as_ref().expect("made above");
        if view::active().buffer().name() != LIST_NAME {
            view::split(Direction::Horizontal);
            view::active().show(&list.buffer);
        }
    }

    /// Writes every diagnostic into the list, if it is open: a line each,
    /// by file and position.
    fn write_list(&mut self) {
        let Some(list) = &mut self.list else {
            return;
        };
        let mut entries = Vec::new();
        for diagnostics in [&self.pushed, &self.pulled] {
            for (uri, found) in diagnostics {
                for item in &found.items {
                    let start = &item["range"]["start"];
                    let place = Place {
                        uri: uri.clone(),
                        line: start["line"].as_u64().unwrap_or(0) as u32,
                        character: start["character"].as_u64().unwrap_or(0) as u32,
                        utf8: found.utf8,
                    };
                    let severity = item["severity"]
                        .as_u64()
                        .map_or(0, |s| (s.clamp(1, 4) - 1) as usize);
                    let message = first_line(item["message"].as_str().unwrap_or_default());
                    entries.push((place, severity, message.to_string()));
                }
            }
        }
        entries.sort_by(|(a, ..), (b, ..)| {
            (&a.uri, a.line, a.character).cmp(&(&b.uri, b.line, b.character))
        });
        let mut text = String::new();
        let mut decorations = Vec::new();
        for (place, severity, message) in &entries {
            let path = uri_to_path(&place.uri, &self.cwd);
            let path = path
                .strip_prefix(&format!("{}/", self.cwd))
                .unwrap_or(&path);
            text.push_str(&format!(
                "{path}:{}:{}: ",
                place.line + 1,
                place.character + 1
            ));
            let name = SEVERITIES[*severity];
            let start = text.len() as u64;
            text.push_str(name);
            decorations.push(Decoration {
                start,
                end: text.len() as u64,
                style: format!("diagnostic.{name}"),
            });
            text.push_str(&format!(": {message}\n"));
        }
        if entries.is_empty() {
            text.push_str("no diagnostics\n");
        }
        list.places = entries.into_iter().map(|(place, ..)| place).collect();
        let buffer = &list.buffer;
        let old = buffer.slice(0, buffer.len()).unwrap_or_default();
        // Only what differs, so the cursor stays on its line.
        let (start, end, new) = difference(&old, &text);
        if start != end || !new.is_empty() {
            let edit = Edit {
                start: start as u64,
                end: end as u64,
                text: new.to_string(),
            };
            let _ = buffer.apply(buffer.version(), &[edit], UndoMode::NewStep);
        }
        ui::set_decorations(buffer, "lsp", &decorations);
    }

    /// Goes to the diagnostic on the cursor's line of the list, in the view
    /// the list was opened from.
    fn goto_listed(&mut self) -> Result<(), String> {
        let list = self.list.as_ref().ok_or("no list of diagnostics")?;
        let (buffer, cursor) = Self::cursor();
        if buffer.name() != LIST_NAME {
            return Err("not in the list of diagnostics".into());
        }
        let line = buffer.line_of(cursor).unwrap_or(0) as usize;
        let Some(place) = list.places.get(line) else {
            return Ok(());
        };
        let (uri, line, character, utf8) =
            (place.uri.clone(), place.line, place.character, place.utf8);
        view::focus(Toward::Next);
        let target = buffer::open(&uri_to_path(&uri, &self.cwd))?;
        let view = view::active();
        view.show(&target);
        let at = position::from_lsp(&target, line, character, utf8);
        let head = target.next_grapheme(at).unwrap_or(at);
        let _ = view.set_selection(&Selection {
            ranges: vec![SelRange { anchor: at, head }],
            primary: 0,
        });
        Ok(())
    }

    /// Closes the list and, when it is in a view of its own, the view.
    fn close_list(&mut self) {
        let Some(list) = self.list.take() else {
            return;
        };
        if view::active().buffer().name() == LIST_NAME {
            let _ = view::close();
        }
        let _ = list.buffer.close(true);
    }

    /// Errors and warnings in every file, in the status line.
    fn show_counts(&self) {
        let mut total = [0; 4];
        for counts in self.counts.values() {
            for (sum, n) in total.iter_mut().zip(counts) {
                *sum += n;
            }
        }
        let mut spans = Vec::new();
        for (severity, letter) in [(0, "E"), (1, "W")] {
            if total[severity] > 0 {
                let style = format!("diagnostic.{}", SEVERITIES[severity]);
                spans.push(span(&format!("{letter}{} ", total[severity]), &style));
            }
        }
        if spans.is_empty() {
            ui::remove_status("diagnostics");
        } else {
            ui::set_status("diagnostics", Side::Right, 10, &spans);
        }
    }

    fn show_hover(&mut self, uri: &str, offset: u64, result: &Value) {
        let lines = hover_lines(&result["contents"]);
        if lines.is_empty() {
            ui::show_message("no hover information");
            return;
        }
        // The cursor may have left the file meanwhile.
        if self.uri(&view::active().buffer()).as_deref() != Some(uri) {
            return;
        }
        let lines: Vec<Vec<Span>> = lines.iter().map(|line| vec![span(line, "")]).collect();
        let popup = Popup::new(PopupAnchor::Position(offset), &lines);
        self.hover = Some((popup, Choices::new(&[])));
    }

    /// One line per language: "<language> ready", "starting", or
    /// "stopped".
    fn status(&self) -> String {
        let running = self.servers.iter().map(|server| {
            let state = if server.ready { "ready" } else { "starting" };
            format!("{} {state}", server.language)
        });
        let stopped = self
            .failed
            .iter()
            .map(|language| format!("{language} stopped"));
        running.chain(stopped).collect::<Vec<_>>().join("\n")
    }

    fn close_popups(&mut self) {
        self.hover = None;
        self.completion = None;
    }

    fn acted(&mut self, id: u64, action: Action) {
        if self.hover.as_ref().is_some_and(|(_, c)| c.id() == id) {
            self.hover = None;
        }
        let Some(completion) = self.completion.as_mut().filter(|c| c.choices.id() == id) else {
            return;
        };
        let step = match action {
            Action::Next => 1,
            Action::Previous => -1,
            Action::Accept => return self.accept(),
            _ => return self.close_popups(),
        };
        let count = completion.shown.len().min(COMPLETION_ROWS) as isize;
        completion.selected = (completion.selected as isize + step).rem_euclid(count) as usize;
        completion.show();
    }

    /// The primary cursor in the shown buffer, where insert mode types.
    fn cursor() -> (Buffer, u64) {
        let view = view::active();
        let selection = view.selection();
        (
            view.buffer(),
            selection.ranges[selection.primary as usize].head,
        )
    }

    /// Asks for completions of the word before the cursor.
    fn complete(&mut self, auto: bool) -> Result<(), String> {
        let (buffer, cursor) = Self::cursor();
        let (i, uri) = self
            .open_document(&buffer)
            .ok_or("no language server for this file")?;
        let start = word_start(&buffer, cursor);
        let server = &mut self.servers[i];
        let (line, character) = position::to_lsp(&buffer, cursor, server.utf8);
        let params = json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character},
            "context": {"triggerKind": 1},
        });
        let request = Request::Completion { uri, start, auto };
        server.request("textDocument/completion", params, request);
        Ok(())
    }

    /// After typing: narrows shown completions, or waits for a pause to ask
    /// for them.
    fn typed(&mut self, change: &BufferChange) {
        if self.completion.is_some() {
            self.narrow();
            return;
        }
        let typed_word = change.changes.last().is_some_and(|c| {
            c.start == c.end
                && c.text
                    .chars()
                    .all(|ch| complete::is_word(ch) || ch == '.' || ch == ':')
        });
        if !self.inserting || !typed_word || self.open_document(&change.buffer).is_none() {
            return;
        }
        if let Some(timer) = self.timer.take() {
            timers::cancel(timer);
        }
        self.timer = Some(timers::set(COMPLETION_DELAY_MS));
    }

    /// Whether the cursor is after enough of a word, or after `.` or `::`,
    /// for completions to be worth showing unasked.
    fn worth_completing(&self) -> bool {
        let (buffer, cursor) = Self::cursor();
        let start = word_start(&buffer, cursor);
        let before = buffer
            .slice(start.saturating_sub(2), start)
            .unwrap_or_default();
        cursor - start >= 2 || before.ends_with('.') || before.ends_with("::")
    }

    fn show_completions(
        &mut self,
        uri: String,
        start: u64,
        language: String,
        auto: bool,
        result: &Value,
    ) {
        let (buffer, _) = Self::cursor();
        // Typing may have left the file or the word meanwhile.
        if self.uri(&buffer).as_deref() != Some(uri.as_str()) {
            return;
        }
        let items = complete::items(result);
        if items.is_empty() {
            if !auto {
                ui::show_message("no completions");
            }
            return;
        }
        self.completion = Some(Completion {
            popup: Popup::new(PopupAnchor::Position(start), &[]),
            choices: Choices::new(&[Action::Next, Action::Previous, Action::Accept]),
            uri,
            start,
            language,
            items,
            shown: Vec::new(),
            selected: 0,
        });
        self.narrow();
    }

    /// Shows the completions matching the word typed so far, or closes
    /// them when none match or the cursor left the word.
    fn narrow(&mut self) {
        let (buffer, cursor) = Self::cursor();
        let uri = self.uri(&buffer);
        let Some(completion) = &mut self.completion else {
            return;
        };
        let typed = buffer.slice(completion.start, cursor).unwrap_or_default();
        let in_word = uri.as_ref() == Some(&completion.uri)
            && cursor >= completion.start
            && typed.chars().all(complete::is_word);
        completion.shown = if in_word {
            complete::matching(&completion.items, &typed)
        } else {
            Vec::new()
        };
        if completion.shown.is_empty() {
            self.close_popups();
            return;
        }
        completion.selected = completion.selected.min(completion.shown.len() - 1);
        completion.show();
    }

    /// Replaces the word with the selected completion.
    fn accept(&mut self) {
        let Some(completion) = self.completion.take() else {
            return;
        };
        self.close_popups();
        let Some(&i) = completion.shown.get(completion.selected) else {
            return;
        };
        let item = &completion.items[i];
        let (buffer, cursor) = Self::cursor();
        let utf8 = self
            .servers
            .iter()
            .find(|s| s.language == completion.language)
            .is_some_and(|s| s.utf8);
        // The server's range ends where the cursor was when it answered;
        // what was typed since is replaced too.
        let (start, end) = match item.range {
            Some(((line, character), (end_line, end_character))) => (
                position::from_lsp(&buffer, line, character, utf8),
                position::from_lsp(&buffer, end_line, end_character, utf8).max(cursor),
            ),
            None => (completion.start, cursor),
        };
        let edit = Edit {
            start,
            end,
            text: item.text.clone(),
        };
        let view = view::active();
        if let Err(err) = view.apply(buffer.version(), &[edit], None, UndoMode::Merge) {
            ui::show_message(&format!("lsp: {err:?}"));
        }
    }

    fn go_to_definition(&mut self, i: usize, result: &Value) {
        // A location, a list of them, or a list of links.
        let first = match result {
            Value::Array(items) => items.first().cloned().unwrap_or(Value::Null),
            other => other.clone(),
        };
        let (uri, position) = if first["targetUri"].is_string() {
            (&first["targetUri"], &first["targetSelectionRange"]["start"])
        } else {
            (&first["uri"], &first["range"]["start"])
        };
        let Some(uri) = uri.as_str() else {
            ui::show_message("no definition found");
            return;
        };
        let utf8 = self.servers[i].utf8;
        if self.uri(&view::active().buffer()).as_deref() != Some(uri) {
            let path = uri_to_path(uri, &self.cwd);
            match buffer::open(&path) {
                Ok(buffer) => view::active().show(&buffer),
                Err(err) => {
                    ui::show_message(&err);
                    return;
                }
            }
        }
        let view = view::active();
        let buffer = view.buffer();
        let line = position["line"].as_u64().unwrap_or(0) as u32;
        let character = position["character"].as_u64().unwrap_or(0) as u32;
        let at = position::from_lsp(&buffer, line, character, utf8);
        let head = buffer.next_grapheme(at).unwrap_or(at);
        let _ = view.set_selection(&Selection {
            ranges: vec![SelRange { anchor: at, head }],
            primary: 0,
        });
    }

    fn uri(&self, buffer: &Buffer) -> Option<String> {
        Some(self.uri_of(&buffer.path()?))
    }

    fn uri_of(&self, path: &str) -> String {
        let absolute = if is_absolute(path) {
            path.to_string()
        } else {
            let relative = path.strip_prefix("./").unwrap_or(path);
            format!("{}/{relative}", self.cwd)
        };
        path_to_uri(&absolute)
    }

    /// The open buffer with URI `uri`.
    fn buffer(&self, uri: &str) -> Option<Buffer> {
        buffer::all()
            .into_iter()
            .find(|buffer| self.uri(buffer).as_deref() == Some(uri))
    }
}

impl Completion {
    fn show(&self) {
        let rows: Vec<&complete::Item> = self
            .shown
            .iter()
            .take(COMPLETION_ROWS)
            .map(|&i| &self.items[i])
            .collect();
        let width = rows
            .iter()
            .map(|item| item.label.chars().count())
            .max()
            .unwrap_or(0);
        let lines: Vec<Vec<Span>> = rows
            .iter()
            .enumerate()
            .map(|(row, item)| {
                let style = if row == self.selected {
                    "ui.menu.selected"
                } else {
                    ""
                };
                let detail: String = item.detail.chars().take(40).collect();
                vec![
                    span(&format!("{:width$}", item.label), style),
                    span(&format!("  {detail}"), "comment"),
                ]
            })
            .collect();
        self.popup.update(&lines);
    }
}

/// Where the word ending at `cursor` starts.
fn word_start(buffer: &Buffer, cursor: u64) -> u64 {
    let line = buffer.line_of(cursor).unwrap_or(0);
    let line_start = buffer.line_start(line).unwrap_or(0);
    let before = buffer.slice(line_start, cursor).unwrap_or_default();
    let word: usize = before
        .chars()
        .rev()
        .take_while(|&c| complete::is_word(c))
        .map(char::len_utf8)
        .sum();
    cursor - word as u64
}

impl Server {
    fn send(&self, message: &Value) {
        // A server that stopped reading ends soon; its exit is reported.
        let _ = self.child.write(&rpc::encode(message));
    }

    fn request(&mut self, method: &str, params: Value, request: Request) {
        self.next_id += 1;
        self.pending.insert(self.next_id, request);
        let message =
            json!({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params});
        self.send(&message);
    }

    fn notify(&self, method: &str, params: Value) {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    fn open(&mut self, uri: &str, buffer: &Buffer) {
        let params = json!({
            "textDocument": {
                "uri": uri,
                "languageId": self.language,
                "version": buffer.version(),
                "text": buffer.slice(0, buffer.len()).unwrap_or_default(),
            },
        });
        self.notify("textDocument/didOpen", params);
        self.opened.insert(uri.to_string());
    }
}

/// A hover's contents as lines of plain text: a string, a markup or marked
/// string, or a list of them. Code fences are dropped.
fn hover_lines(contents: &Value) -> Vec<String> {
    let text = match contents {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| item.as_str().or_else(|| item["value"].as_str()))
            .collect::<Vec<_>>()
            .join("\n\n"),
        other => other["value"].as_str().unwrap_or_default().to_string(),
    };
    let mut lines: Vec<String> = text
        .lines()
        .filter(|line| !line.trim_start().starts_with("```"))
        .map(|line| line.chars().take(HOVER_WIDTH).collect())
        .take(HOVER_LINES)
        .collect();
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    lines
}

/// Where `old` and `new` differ: the byte range in `old` and what replaces
/// it.
fn difference<'a>(old: &str, new: &'a str) -> (usize, usize, &'a str) {
    let mut start = old
        .bytes()
        .zip(new.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(start) || !new.is_char_boundary(start) {
        start -= 1;
    }
    let most = old.len().min(new.len()) - start;
    let mut common = old
        .bytes()
        .rev()
        .zip(new.bytes().rev())
        .take(most)
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(old.len() - common) || !new.is_char_boundary(new.len() - common) {
        common -= 1;
    }
    (start, old.len() - common, &new[start..new.len() - common])
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or_default()
}

fn is_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    path.starts_with('/')
        || path.starts_with('\\')
        || (bytes.len() > 2 && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/'))
}

/// A `file:` URI, with the path's special chars escaped.
fn path_to_uri(path: &str) -> String {
    let path = path.replace('\\', "/");
    let mut uri = String::from("file://");
    if !path.starts_with('/') {
        // A Windows drive, as in file:///C:/...
        uri.push('/');
    }
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/:".contains(&byte) {
            uri.push(byte as char);
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

/// The path of a `file:` URI, relative to `cwd` when it is inside it, as
/// files are usually opened.
fn uri_to_path(uri: &str, cwd: &str) -> String {
    let encoded = uri.strip_prefix("file://").unwrap_or(uri);
    let mut bytes = Vec::with_capacity(encoded.len());
    let mut rest = encoded.as_bytes();
    while let Some((&byte, tail)) = rest.split_first() {
        let decoded = (byte == b'%')
            .then(|| std::str::from_utf8(tail.get(..2)?).ok())
            .flatten()
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match decoded {
            Some(value) => {
                bytes.push(value);
                rest = &tail[2..];
            }
            None => {
                bytes.push(byte);
                rest = tail;
            }
        }
    }
    let mut path = String::from_utf8_lossy(&bytes).into_owned();
    // "/C:/..." on Windows.
    if path.as_bytes().get(2) == Some(&b':') {
        path.remove(0);
    }
    let cwd = cwd.replace('\\', "/");
    match path.strip_prefix(&format!("{cwd}/")) {
        Some(relative) => relative.to_string(),
        None => path,
    }
}

fn span(text: &str, style: &str) -> Span {
    Span {
        text: text.into(),
        style: style.into(),
    }
}

nib_plugin::export!(Plugin);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_round_trip() {
        let uri = path_to_uri("/home/me/a b/c#.rs");
        assert_eq!(uri, "file:///home/me/a%20b/c%23.rs");
        assert_eq!(uri_to_path(&uri, "/home/me"), "a b/c#.rs");
        assert_eq!(uri_to_path(&uri, "/elsewhere"), "/home/me/a b/c#.rs");
        let windows = path_to_uri("C:\\work\\x.rs");
        assert_eq!(windows, "file:///C:/work/x.rs");
        assert_eq!(uri_to_path(&windows, "C:\\work"), "x.rs");
    }

    #[test]
    fn differences_cover_what_changed() {
        assert_eq!(difference("a\nb\nc\n", "a\nx\nc\n"), (2, 3, "x"));
        assert_eq!(difference("ab", "ab"), (2, 2, ""));
        assert_eq!(difference("", "ab"), (0, 0, "ab"));
        assert_eq!(difference("aab", "ab"), (1, 2, ""));
        assert_eq!(difference("é", "è"), (0, 2, "è"));
    }

    #[test]
    fn hovers_become_lines() {
        let markup = json!({"kind": "markdown", "value": "```rust\nfn f()\n```\ndoes it\n\n"});
        assert_eq!(hover_lines(&markup), ["fn f()", "does it"]);
        let list = json!(["one", {"language": "rust", "value": "two"}]);
        assert_eq!(hover_lines(&list), ["one", "", "two"]);
    }
}
