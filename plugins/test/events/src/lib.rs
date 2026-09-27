//! Test plugin for commands, events, and timers. It writes down the events
//! it gets, and its commands do what a test asks. Arguments are plain text,
//! not JSON, to keep the tests short.

use std::cell::RefCell;

use nib_plugin::exports::nib::plugin::guest::{Guest, KeyResult};
use nib_plugin::nib::plugin::events::{self, Event};
use nib_plugin::nib::plugin::process::{self, Child, Stream};
use nib_plugin::nib::plugin::prompt::Line;
use nib_plugin::nib::plugin::types::{Edit, KeyEvent, UndoMode};
use nib_plugin::nib::plugin::{commands, editor, files, timers};

/// A program it started, and what it printed so far.
struct Program {
    child: Child,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

thread_local! {
    static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static PROGRAMS: RefCell<Vec<Program>> = const { RefCell::new(Vec::new()) };
    static PROMPT: RefCell<Option<Line>> = const { RefCell::new(None) };
}

const COMMANDS: [&str; 18] = [
    "echo",
    "call",
    "log",
    "emit",
    "timer",
    "cancel",
    "edit",
    "buffers",
    "spawn",
    "write",
    "close",
    "kill",
    "walk",
    "stop-walk",
    "save",
    "load",
    "prompt",
    "close-prompt",
];

struct Events;

impl Guest for Events {
    fn init(_config: String) -> Result<(), String> {
        for name in COMMANDS {
            commands::register(name, &format!("test: {name}"));
        }
        Ok(())
    }

    fn handle_key(_ev: KeyEvent) -> KeyResult {
        KeyResult::Pass
    }

    fn run_command(name: String, args: String) -> Result<String, String> {
        match name.as_str() {
            "echo" => Ok(args),
            // Its data directory, which outlives restarts.
            "save" => std::fs::write("/data/note", args)
                .map(|()| String::new())
                .map_err(|err| err.to_string()),
            "load" => std::fs::read_to_string("/data/note").map_err(|err| err.to_string()),
            // "<command> <args>": the result, marked ok or err.
            "call" => {
                let (command, args) = args.split_once(' ').unwrap_or((&args, ""));
                Ok(match commands::call(command, args) {
                    Ok(result) => format!("ok:{result}"),
                    Err(err) => format!("err:{err}"),
                })
            }
            "log" => Ok(LOG.with_borrow_mut(|log| std::mem::take(log).join("\n"))),
            "emit" => {
                events::emit(&args, "");
                Ok(String::new())
            }
            "timer" => {
                let ms = args.parse().map_err(|_| "timer needs milliseconds")?;
                Ok(timers::set(ms).to_string())
            }
            "cancel" => {
                timers::cancel(args.parse().map_err(|_| "cancel needs an id")?);
                Ok(String::new())
            }
            // Inserts the text at the start of the shown buffer.
            "edit" => {
                let view = editor::active_view();
                let edit = Edit {
                    start: 0,
                    end: 0,
                    text: args,
                };
                let version = view.buffer().version();
                view.apply(version, &[edit], None, UndoMode::NewStep)
                    .map_err(|err| format!("{err:?}"))?;
                Ok(String::new())
            }
            "buffers" => Ok(editor::buffers().len().to_string()),
            // The command and its arguments, one per line.
            "spawn" => {
                let mut words = args.lines().map(String::from);
                let command = words.next().ok_or("spawn needs a command")?;
                let args: Vec<String> = words.collect();
                let child = process::spawn(&command, &args, None)?;
                let id = child.id();
                PROGRAMS.with_borrow_mut(|programs| {
                    programs.push(Program {
                        child,
                        stdout: Vec::new(),
                        stderr: Vec::new(),
                    })
                });
                Ok(id.to_string())
            }
            // "<id> <text>"
            "write" => {
                let (id, text) = args.split_once(' ').ok_or("write needs an id and text")?;
                with_program(id, |p| p.child.write(text.as_bytes()))??;
                Ok(String::new())
            }
            "close" => with_program(&args, |p| p.child.close_stdin()).map(|()| String::new()),
            "kill" => with_program(&args, |p| p.child.kill()).map(|()| String::new()),
            // Lists the files under the directory given, or the working one.
            "walk" => {
                let dir = (!args.is_empty()).then_some(args.as_str());
                Ok(files::walk(dir)?.to_string())
            }
            "stop-walk" => {
                files::cancel(args.parse().map_err(|_| "stop-walk needs an id")?);
                Ok(String::new())
            }
            // Opens a prompt labeled `args`, and says its id.
            "prompt" => {
                let line = Line::new(&args);
                let id = line.id();
                PROMPT.set(Some(line));
                Ok(id.to_string())
            }
            "close-prompt" => {
                PROMPT.set(None);
                Ok(String::new())
            }
            _ => Err(format!("no command {name}")),
        }
    }

    fn on_event(ev: Event) {
        let entry = match ev {
            Event::BufferOpened(buffer) => format!("opened {}", name(buffer.path())),
            Event::BufferSaved(buffer) => format!("saved {}", name(buffer.path())),
            Event::BufferChanged(change) => {
                let changes: Vec<String> = change
                    .changes
                    .iter()
                    .map(|c| {
                        format!(
                            "{}-{}@{}:{}-{}:{}={}",
                            c.start,
                            c.end,
                            c.start_line,
                            c.start_column,
                            c.end_line,
                            c.end_column,
                            c.text
                        )
                    })
                    .collect();
                format!("changed v{} {}", change.version, changes.join(";"))
            }
            Event::Custom(custom) => {
                // Answers itself forever, to check the host stops it.
                if custom.name == "test-events.loop" {
                    events::emit("loop", "");
                }
                format!("custom {} {}", custom.name, custom.data)
            }
            Event::Timer(id) => format!("timer {id}"),
            Event::ProcessOutput(output) => {
                PROGRAMS.with_borrow_mut(|programs| {
                    if let Some(p) = programs.iter_mut().find(|p| p.child.id() == output.process) {
                        match output.stream {
                            Stream::Stdout => p.stdout.extend(&output.data),
                            Stream::Stderr => p.stderr.extend(&output.data),
                        }
                    }
                });
                return;
            }
            // Everything a program printed, written down when it ends.
            Event::ProcessExit(exit) => PROGRAMS.with_borrow_mut(|programs| {
                let Some(at) = programs.iter().position(|p| p.child.id() == exit.process) else {
                    return format!("exit of unknown {}", exit.process);
                };
                let program = programs.remove(at);
                format!(
                    "exit {} {:?} stdout={} stderr={}",
                    exit.process,
                    exit.code,
                    printed(&program.stdout),
                    printed(&program.stderr)
                )
            }),
            Event::PromptChanged(change) => {
                format!("prompt {} {:?} {}", change.id, change.text, change.cursor)
            }
            Event::PromptAction(act) => format!("prompt {} {:?}", act.id, act.action),
            Event::FilesListed(listed) => {
                let mut paths = listed.paths;
                paths.sort();
                format!(
                    "files {} {} done={}",
                    listed.job,
                    paths.join(","),
                    listed.done
                )
            }
        };
        LOG.with_borrow_mut(|log| log.push(entry));
    }
}

/// Runs `f` on the program with id `id`.
fn with_program<R>(id: &str, f: impl FnOnce(&mut Program) -> R) -> Result<R, String> {
    let id: u64 = id.trim().parse().map_err(|_| "not an id")?;
    PROGRAMS.with_borrow_mut(|programs| {
        let program = programs
            .iter_mut()
            .find(|p| p.child.id() == id)
            .ok_or("no such program")?;
        Ok(f(program))
    })
}

/// Output on one line, the same on every system.
fn printed(data: &[u8]) -> String {
    String::from_utf8_lossy(data)
        .replace('\r', "")
        .trim_end()
        .replace('\n', "|")
}

/// The file name of a buffer's path.
fn name(path: Option<String>) -> String {
    path.map_or("[scratch]".into(), |path| {
        path.rsplit(['/', '\\']).next().unwrap_or("").to_string()
    })
}

nib_plugin::export!(Events);
