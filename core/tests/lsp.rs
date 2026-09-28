//! The LSP plugin in `plugins/lsp`, with the Helix keymap, against a fake
//! language server: this test binary, started with `--fake-lsp`. Build the
//! plugins first with `cargo xtask build-plugins`.
//!
//! The fake server reports "found error" at every "error" in a file, says
//! where the cursor is when hovered, and puts every definition at line 0,
//! character 3 of the same file.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use std::{env, fs, panic, process, thread};

use nib_core::{Config, Editor, KeyEvent};
use serde_json::{Value, json};

mod common;
use common::{plugin_dir, screen, type_keys};

fn main() {
    if env::args().any(|arg| arg == "--fake-lsp") {
        fake_server(false);
        return;
    }
    // Gives diagnostics only when asked, as rust-analyzer does for its own.
    if env::args().any(|arg| arg == "--fake-lsp-pull") {
        fake_server(true);
        return;
    }
    // A server that is not really there, as rustup's proxy when the
    // component is missing.
    if env::args().any(|arg| arg == "--fake-lsp-gone") {
        eprintln!("\nerror: not installed\nmore detail");
        process::exit(1);
    }
    let tests: [(&str, fn()); 10] = [
        ("diagnostics_follow_edits", diagnostics_follow_edits),
        (
            "diagnostics_asked_for_follow_edits",
            diagnostics_asked_for_follow_edits,
        ),
        (
            "hover_shows_until_the_next_key",
            hover_shows_until_the_next_key,
        ),
        ("definition_moves_the_cursor", definition_moves_the_cursor),
        ("missing_servers_are_reported", missing_servers_are_reported),
        ("servers_that_stop_say_why", servers_that_stop_say_why),
        ("completes_when_asked", completes_when_asked),
        (
            "completes_on_its_own_and_narrows",
            completes_on_its_own_and_narrows,
        ),
        (
            "closed_buffers_close_on_the_server",
            closed_buffers_close_on_the_server,
        ),
        (
            "the_diagnostics_list_goes_to_each",
            the_diagnostics_list_goes_to_each,
        ),
    ];
    let mut failed = 0;
    for (name, test) in tests {
        let passed = panic::catch_unwind(test).is_ok();
        println!("test {name} ... {}", if passed { "ok" } else { "FAILED" });
        failed += usize::from(!passed);
    }
    println!(
        "\ntest result: {}. {} passed; {failed} failed",
        if failed == 0 { "ok" } else { "FAILED" },
        tests.len() - failed
    );
    if failed > 0 {
        process::exit(1);
    }
}

/// An editor with the keymap, Rust, and the LSP plugin talking to `server`,
/// on a file with `text`.
fn editor(name: &str, text: &str, server: &[String]) -> (Editor, PathBuf) {
    let dir = env::temp_dir().join(format!("nib-lsp-{}-{name}", process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("main.rs");
    fs::write(&path, text).unwrap();
    let mut editor = Editor::default();
    editor.open(&path).unwrap();
    let command = serde_json::to_string(server).unwrap();
    let settings = format!("[settings.servers.rust]\ncommand = {command}\n");
    let mut config = Config::default();
    config.plugins.insert(
        "lsp".into(),
        Config::parse_plugin("lsp", &settings).unwrap(),
    );
    editor.apply_config(config);
    for plugin in ["helix", "rust", "lsp"] {
        editor.load_plugin(&plugin_dir(plugin)).unwrap();
    }
    editor.resize(60, 10);
    editor.catch_up();
    (editor, dir)
}

fn fake(name: &str, text: &str) -> (Editor, PathBuf) {
    let exe = env::current_exe().unwrap().to_string_lossy().into_owned();
    editor(name, text, &[exe, "--fake-lsp".into()])
}

/// Hands the servers' output to the plugin until `done` holds.
fn wait_until(editor: &mut Editor, what: &str, mut done: impl FnMut(&mut Editor) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done(editor) {
        if Instant::now() >= deadline {
            // Rare and on CI only so far: say what was there, to tell why.
            let status = editor.call_command("lsp.status", "");
            panic!(
                "waited 20 seconds for {what}\nmessage: {:?}\nlsp.status: {status:?}\nscreen:\n{}",
                editor.message(),
                screen(editor).join("\n")
            );
        }
        editor.run_background();
        editor.run_timers();
        thread::sleep(Duration::from_millis(10));
    }
}

/// Waits until the server is initialized and has the file.
fn wait_for_server(editor: &mut Editor) {
    wait_until(editor, "the server", |e| {
        e.call_command("lsp.status", "").unwrap() == "rust ready"
    });
}

fn shows(editor: &Editor, text: &str) -> bool {
    screen(editor).iter().any(|row| row.contains(text))
}

fn cursor(editor: &Editor) -> usize {
    editor.view().cursor(editor.buffer().text())
}

fn diagnostics_follow_edits() {
    check_diagnostics("--fake-lsp");
}

fn diagnostics_asked_for_follow_edits() {
    check_diagnostics("--fake-lsp-pull");
}

fn check_diagnostics(server: &str) {
    let exe = env::current_exe().unwrap().to_string_lossy().into_owned();
    let (mut editor, dir) = editor(
        &format!("diagnostics{server}"),
        "fn main() {}\nlet error = 1;\n",
        &[exe, server.into()],
    );
    wait_until(&mut editor, "the diagnostic", |e| shows(e, "found error"));
    let rows = screen(&editor);
    assert!(
        rows[1].starts_with("let error = 1;  found error"),
        "{rows:#?}"
    );
    assert!(rows.last().unwrap().contains("E1"), "{rows:#?}");
    // The server gets the deletion as an edit and finds nothing left.
    type_keys(&mut editor, "jxd");
    wait_until(&mut editor, "the diagnostic to go", |e| {
        !shows(e, "found error") && !screen(e).last().unwrap().contains("E1")
    });
    fs::remove_dir_all(dir).unwrap();
}

fn the_diagnostics_list_goes_to_each() {
    let (mut editor, dir) = fake("list", "fn main() {}\nlet error = 1;\n");
    wait_until(&mut editor, "the diagnostic", |e| shows(e, "found error"));
    editor.call_command("lsp.diagnostics", "").unwrap();
    let listed = editor.buffer().text().to_string();
    assert!(
        listed.ends_with("main.rs:2:5: error: found error\n"),
        "{listed}"
    );
    // Enter goes there in the view below.
    type_keys(&mut editor, "<ret>");
    assert_eq!(line(&editor, 1), "let error = 1;\n");
    assert_eq!(cursor(&editor), 17);
    // Rewritten as the diagnostics change.
    type_keys(&mut editor, "xd");
    wait_until(&mut editor, "the list to empty", |e| {
        shows(e, "no diagnostics")
    });
    // q closes it and its view.
    editor.handle_key(KeyEvent::ctrl('w'));
    type_keys(&mut editor, "wq");
    assert!(!shows(&editor, "diagnostics"), "{:#?}", screen(&editor));
    assert_eq!(line(&editor, 0), "fn main() {}\n");
    fs::remove_dir_all(dir).unwrap();
}

fn hover_shows_until_the_next_key() {
    let (mut editor, dir) = fake("hover", "fn main() {}\n");
    type_keys(&mut editor, "l");
    wait_for_server(&mut editor);
    type_keys(&mut editor, " k");
    wait_until(&mut editor, "the hover", |e| shows(e, "hover at 0:1"));
    // The key closes it and still moves the cursor.
    type_keys(&mut editor, "l");
    assert!(!shows(&editor, "hover at"));
    assert_eq!(cursor(&editor), 2);
    fs::remove_dir_all(dir).unwrap();
}

fn definition_moves_the_cursor() {
    let (mut editor, dir) = fake("definition", "fn main() {}\nmain();\n");
    wait_for_server(&mut editor);
    type_keys(&mut editor, "jgd");
    wait_until(&mut editor, "the definition", |e| cursor(e) == 3);
    fs::remove_dir_all(dir).unwrap();
}

fn missing_servers_are_reported() {
    let (editor, dir) = editor("missing", "fn main() {}\n", &["nib-no-such-server".into()]);
    let message = editor.message().unwrap_or_default().to_string();
    assert!(
        message.starts_with("lsp: nib-no-such-server: "),
        "{message}"
    );
    fs::remove_dir_all(dir).unwrap();
}

fn servers_that_stop_say_why() {
    let exe = env::current_exe().unwrap().to_string_lossy().into_owned();
    let server = [exe.clone(), "--fake-lsp-gone".into()];
    let (mut editor, dir) = editor("gone", "fn main() {}\n", &server);
    let expected = format!("lsp: {exe} --fake-lsp-gone stopped: error: not installed");
    wait_until(&mut editor, "the server to stop", |e| {
        e.message() == Some(expected.as_str())
    });
    fs::remove_dir_all(dir).unwrap();
}

fn line(editor: &Editor, n: usize) -> String {
    editor.buffer().text().line(n).to_string()
}

fn completes_when_asked() {
    let (mut editor, dir) = fake("complete", "fn main() {}\n");
    wait_for_server(&mut editor);
    type_keys(&mut editor, "oal");
    editor.handle_key(KeyEvent::ctrl('x'));
    wait_until(&mut editor, "completions", |e| shows(e, "alphabet"));
    assert!(!shows(&editor, "beta"), "narrowed to what is typed");
    // The second, then in, as a snippet without its marks.
    editor.handle_key(KeyEvent::ctrl('n'));
    type_keys(&mut editor, "<ret>");
    assert_eq!(line(&editor, 1), "alphabet()\n");
    assert!(!shows(&editor, "letters"));
    assert!(screen(&editor).last().unwrap().starts_with(" INS "));
    fs::remove_dir_all(dir).unwrap();
}

fn completes_on_its_own_and_narrows() {
    let (mut editor, dir) = fake("auto", "fn main() {}\n");
    wait_for_server(&mut editor);
    type_keys(&mut editor, "obe");
    wait_until(&mut editor, "completions", |e| shows(e, "beta"));
    // Nothing matches any more: it closes, and typing goes on.
    type_keys(&mut editor, "x");
    assert!(!shows(&editor, "beta"));
    type_keys(&mut editor, "<esc>");
    assert_eq!(line(&editor, 1), "bex\n");
    fs::remove_dir_all(dir).unwrap();
}

/// A language server that knows just enough for the tests.
fn fake_server(pull: bool) {
    let mut input = BufReader::new(io::stdin().lock());
    let mut documents: HashMap<String, String> = HashMap::new();
    while let Some(message) = read_message(&mut input) {
        let params = &message["params"];
        let uri = params["textDocument"]["uri"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let method = message["method"].as_str().unwrap_or_default();
        // What documents it was told about, for tests to read.
        if let Ok(log) = env::var("NIB_FAKE_LSP_LOG")
            && method.starts_with("textDocument/did")
            && method != "textDocument/didChange"
        {
            let name = uri.rsplit('/').next().unwrap_or_default();
            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log)
                .unwrap();
            writeln!(file, "{method} {name}").unwrap();
        }
        match method {
            "initialize" => reply(
                &message,
                json!({"capabilities": {
                    "positionEncoding": "utf-8",
                    "textDocumentSync": 2,
                    "hoverProvider": true,
                    "definitionProvider": true,
                    "diagnosticProvider": if pull {
                        json!({"interFileDependencies": false, "workspaceDiagnostics": false})
                    } else {
                        Value::Null
                    },
                }}),
            ),
            "textDocument/diagnostic" => {
                let items = diagnostics(documents.get(&uri).map_or("", String::as_str));
                reply(&message, json!({"kind": "full", "items": items}));
            }
            "textDocument/didOpen" => {
                let text = params["textDocument"]["text"].as_str().unwrap_or_default();
                documents.insert(uri.clone(), text.to_string());
                if !pull {
                    publish(&uri, &documents[&uri]);
                }
            }
            "textDocument/didChange" => {
                let text = documents.entry(uri.clone()).or_default();
                for change in params["contentChanges"].as_array().unwrap() {
                    let new = change["text"].as_str().unwrap();
                    match change.get("range") {
                        Some(range) => {
                            let start = offset(text, &range["start"]);
                            let end = offset(text, &range["end"]);
                            text.replace_range(start..end, new);
                        }
                        None => *text = new.to_string(),
                    }
                }
                if !pull {
                    publish(&uri, &documents[&uri]);
                }
            }
            "textDocument/hover" => {
                let at = &params["position"];
                let value = format!("hover at {}:{}", at["line"], at["character"]);
                reply(
                    &message,
                    json!({"contents": {"kind": "plaintext", "value": value}}),
                );
            }
            "textDocument/definition" => reply(
                &message,
                json!({"uri": uri, "range": {
                    "start": {"line": 0, "character": 3},
                    "end": {"line": 0, "character": 7},
                }}),
            ),
            "textDocument/completion" => reply(
                &message,
                json!({"isIncomplete": false, "items": [
                    {"label": "beta", "detail": "b", "sortText": "3"},
                    {"label": "alpha", "detail": "a letter", "sortText": "1"},
                    {
                        "label": "alphabet",
                        "detail": "letters",
                        "sortText": "2",
                        "insertText": "alphabet($1)",
                        "insertTextFormat": 2,
                    },
                ]}),
            ),
            "shutdown" => reply(&message, Value::Null),
            "exit" => return,
            _ => {}
        }
    }
}

fn read_message(input: &mut impl BufRead) -> Option<Value> {
    let mut length = 0;
    loop {
        let mut line = String::new();
        if input.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length: ") {
            length = value.parse().ok()?;
        }
    }
    let mut body = vec![0; length];
    input.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

fn send(message: &Value) {
    let body = message.to_string();
    let mut out = io::stdout().lock();
    write!(out, "Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
    out.flush().unwrap();
}

fn reply(request: &Value, result: Value) {
    send(&json!({"jsonrpc": "2.0", "id": request["id"], "result": result}));
}

fn publish(uri: &str, text: &str) {
    send(&json!({
        "jsonrpc": "2.0",
        "method": "textDocument/publishDiagnostics",
        "params": {"uri": uri, "diagnostics": diagnostics(text)},
    }));
}

/// A diagnostic at every "error".
fn diagnostics(text: &str) -> Vec<Value> {
    text.lines()
        .enumerate()
        .filter_map(|(line, content)| {
            let character = content.find("error")?;
            Some(json!({
                "range": {
                    "start": {"line": line, "character": character},
                    "end": {"line": line, "character": character + 5},
                },
                "severity": 1,
                "message": "found error",
            }))
        })
        .collect()
}

/// The byte offset of an LSP position counted in bytes.
fn offset(text: &str, position: &Value) -> usize {
    let line = position["line"].as_u64().unwrap() as usize;
    let character = position["character"].as_u64().unwrap() as usize;
    let line_start: usize = text.split_inclusive('\n').take(line).map(str::len).sum();
    line_start + character
}

fn closed_buffers_close_on_the_server() {
    let log = env::temp_dir().join(format!("nib-lsp-{}-closing.log", process::id()));
    // The server is started by the plugin, and inherits this.
    // SAFETY: the tests here run one after another, on this thread.
    unsafe { env::set_var("NIB_FAKE_LSP_LOG", &log) };
    let (mut editor, dir) = fake("closing", "fn main() {}\n");
    wait_for_server(&mut editor);
    let other = dir.join("other.rs");
    fs::write(&other, "fn other() {}\n").unwrap();
    let lines = || fs::read_to_string(&log).unwrap_or_default();
    editor.open(&other).unwrap();
    wait_until(&mut editor, "the server to open it", |_| {
        lines().contains("didOpen other.rs")
    });
    editor.call_command("buffer.close", "").unwrap();
    // Opened again, it is opened again on the server.
    editor.open(&other).unwrap();
    wait_until(&mut editor, "the server to hear of it", |_| {
        lines().matches("didOpen other.rs").count() == 2
    });
    assert_eq!(
        lines().lines().collect::<Vec<_>>(),
        [
            "textDocument/didOpen main.rs",
            "textDocument/didOpen other.rs",
            "textDocument/didClose other.rs",
            "textDocument/didOpen other.rs",
        ]
    );
    unsafe { env::remove_var("NIB_FAKE_LSP_LOG") };
    fs::remove_file(&log).unwrap();
    fs::remove_dir_all(dir).unwrap();
}
