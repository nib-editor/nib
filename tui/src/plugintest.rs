//! `nib plugin test`: runs a plugin's tests, written in TOML, in an editor
//! without a terminal (docs/plugin-dev.md).

use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use nib_core::{
    Config, Editor, Grid, PluginConfig, PluginSource, Selection, Symbol, marks, parse_keys,
    read_manifest,
};
use serde::Deserialize;

use crate::builtin;
use crate::settings;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TestFile {
    /// The standard plugins to load; all but `lsp` when not given.
    with: Option<Vec<String>>,
    /// The tested plugin's `[settings]` for every test that has none.
    settings: Option<toml::Table>,
    #[serde(default)]
    test: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    name: String,
    #[serde(default = "default_file")]
    file: String,
    #[serde(default)]
    text: String,
    settings: Option<toml::Table>,
    /// More files in the working directory, by path.
    #[serde(default)]
    files: toml::Table,
    /// A test of one step can put it here instead of in `step`.
    #[serde(flatten)]
    only: Step,
    #[serde(default)]
    step: Vec<Step>,
}

/// Keys to send, then text to paste, then a command to call, then
/// expectations to check, waiting up to `wait` ms for them.
#[derive(Deserialize, Default)]
struct Step {
    #[serde(default)]
    keys: String,
    /// Pasted into the terminal after the keys, as one piece.
    paste: Option<String>,
    command: Option<String>,
    args: Option<String>,
    wait: Option<u64>,
    #[serde(default)]
    expect: Expect,
}

impl Step {
    fn is_empty(&self) -> bool {
        self.keys.is_empty()
            && self.paste.is_none()
            && self.command.is_none()
            && self.wait.is_none()
            && self.expect.is_empty()
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Expect {
    text: Option<String>,
    selections: Option<String>,
    message: Option<String>,
    screen: Option<Vec<String>>,
    /// Strings on no row of the screen.
    absent: Option<Vec<String>>,
    result: Option<String>,
    error: Option<String>,
}

impl Expect {
    fn is_empty(&self) -> bool {
        self.text.is_none()
            && self.selections.is_none()
            && self.message.is_none()
            && self.screen.is_none()
            && self.absent.is_none()
            && self.result.is_none()
            && self.error.is_none()
    }
}

fn default_file() -> String {
    "test.txt".into()
}

/// The standard plugins tests load unless they say: all but `lsp`, which
/// starts language servers.
const LEFT_OUT: &[&str] = &["lsp"];

const SIZE: (u16, u16) = (80, 24);

/// Runs the tests in `files`, or in `dir/tests/*.toml`, against the plugin
/// in `dir`. Prints a line per test and fails if any did.
pub fn run(dir: &Path, files: &[PathBuf]) -> Result<(), String> {
    // Absolute, as each test runs in a directory of its own.
    let absolute = |path: &Path| {
        path.canonicalize()
            .map(plain)
            .map_err(|err| format!("{}: {err}", path.display()))
    };
    let started_in = std::env::current_dir().map_err(|err| err.to_string())?;
    let dir = &absolute(dir)?;
    let manifest = read_manifest(dir).map_err(|err| err.to_string())?;
    if manifest.has_code && !dir.join("plugin.wasm").is_file() {
        return Err(format!(
            "{} has no plugin.wasm; build it first with `nib plugin build`",
            dir.display()
        ));
    }
    let files = match files {
        [] => test_files(dir)?,
        files => files
            .iter()
            .map(|f| absolute(f))
            .collect::<Result<_, _>>()?,
    };
    let scratch = Scratch::new()?;
    let (mut passed, mut failed) = (0, 0);
    let mut count = 0;
    for file in &files {
        let shown = file
            .strip_prefix(dir)
            .or_else(|_| file.strip_prefix(&started_in))
            .unwrap_or(file)
            .display()
            .to_string();
        let tests = match read(file) {
            Ok(tests) => tests,
            Err(err) => {
                println!("{shown}: {err}");
                failed += 1;
                continue;
            }
        };
        let folder = file.parent().unwrap_or(Path::new("."));
        let folder = folder.canonicalize().map_or(folder.to_path_buf(), plain);
        for case in &tests.test {
            count += 1;
            let work = scratch.0.join(count.to_string());
            let setup = Setup {
                dir,
                name: &manifest.name,
                with: tests.with.as_deref(),
                settings: case.settings.as_ref().or(tests.settings.as_ref()),
                folder: &folder,
                work: &work,
            };
            let outcome = run_case(&setup, case);
            match outcome {
                Ok(()) => {
                    println!("test {shown}: {} ... ok", case.name);
                    passed += 1;
                }
                Err(problems) => {
                    println!("test {shown}: {} ... FAILED", case.name);
                    for problem in problems {
                        println!("    {}", problem.replace('\n', "\n    "));
                    }
                    failed += 1;
                }
            }
        }
    }
    // Out of the tests' directories before they are removed, which Windows
    // refuses for the current one.
    std::env::set_current_dir(&started_in).map_err(|err| err.to_string())?;
    println!("{passed} passed, {failed} failed");
    match failed {
        0 => Ok(()),
        _ => Err(format!("{failed} of {} tests failed", passed + failed)),
    }
}

fn test_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let tests = dir.join("tests");
    let mut files: Vec<PathBuf> = fs::read_dir(&tests)
        .map_err(|err| format!("{}: {err}", tests.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|e| e == "toml"))
        .collect();
    files.sort();
    if files.is_empty() {
        return Err(format!("no tests in {}", tests.display()));
    }
    Ok(files)
}

fn read(file: &Path) -> Result<TestFile, String> {
    let text = fs::read_to_string(file).map_err(|err| err.to_string())?;
    let tests: TestFile = toml::from_str(&text).map_err(|err| err.to_string())?;
    if let Some(with) = &tests.with
        && let Some(unknown) = with
            .iter()
            .find(|name| !builtin::PLUGINS.iter().any(|(n, _, _)| n == name))
    {
        return Err(format!(
            "`with` names {unknown:?}, which is not a standard plugin"
        ));
    }
    Ok(tests)
}

/// What a test runs against.
struct Setup<'a> {
    /// The tested plugin's directory and name.
    dir: &'a Path,
    name: &'a str,
    with: Option<&'a [String]>,
    settings: Option<&'a toml::Table>,
    /// The test file's directory, for `{dir}` in settings.
    folder: &'a Path,
    /// The test's own working directory, made for it.
    work: &'a Path,
}

/// Runs one test in a new editor, and says what did not come out as
/// expected.
fn run_case(setup: &Setup, case: &Case) -> Result<(), Vec<String>> {
    let fail = |problem: String| vec![problem];
    let steps: Vec<&Step> = match (case.only.is_empty(), case.step.is_empty()) {
        (_, true) => vec![&case.only],
        (true, false) => case.step.iter().collect(),
        (false, false) => {
            return Err(fail(
                "put keys, paste, command, wait, and expect in the steps when there are steps"
                    .into(),
            ));
        }
    };
    let marked = marks::parse(&case.text).map_err(|err| fail(format!("text: {err}")))?;
    let file_name = Path::new(&case.file)
        .file_name()
        .ok_or_else(|| fail(format!("file: {:?} is not a file name", case.file)))?;
    let path = setup.work.join(file_name);
    let mut written = vec![(path.clone(), marked.text.clone())];
    for (name, text) in &case.files {
        let Some(text) = text.as_str() else {
            return Err(fail(format!("files: {name:?} needs a string")));
        };
        written.push((setup.work.join(name), text.to_string()));
    }
    for (path, text) in &written {
        let made = path
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| fs::write(path, text));
        made.map_err(|err| fail(format!("{}: {err}", path.display())))?;
    }
    // The editor and plugins take the process's directory as the working
    // one; each test gets its own, so listing files gives the same answer
    // wherever tests run.
    std::env::set_current_dir(setup.work)
        .map_err(|err| fail(format!("{}: {err}", setup.work.display())))?;

    let mut editor = Editor::default();
    editor.set_plugin_cache_dir(settings::cache_dir());
    // Empty for each test, and never the user's.
    editor.set_plugin_data_dir(Some(setup.work.with_extension("data")));
    editor.resize(SIZE.0, SIZE.1);
    let mut config = Config::default();
    // A base under test is the one in use.
    if read_manifest(setup.dir).is_ok_and(|manifest| manifest.base) {
        config.core.base = setup.name.to_string();
    }
    if let Some(table) = setup.settings {
        let plugin = PluginConfig {
            settings: plugin_settings(table, setup.folder).to_string(),
            ..PluginConfig::default()
        };
        config.plugins.insert(setup.name.to_string(), plugin);
    }
    editor.apply_config(config);
    // Opened before the plugins load, as when nib starts with a file.
    editor
        .open(&path)
        .map_err(|err| fail(format!("{}: {err}", path.display())))?;
    let standard = builtin::PLUGINS.iter().filter(|(n, _, _)| {
        *n != setup.name
            && match setup.with {
                Some(with) => with.iter().any(|w| w == n),
                None => !LEFT_OUT.contains(n),
            }
    });
    let mut sources: Vec<_> = standard
        .map(|&(_, manifest, files)| PluginSource::Bytes { manifest, files })
        .collect();
    sources.push(PluginSource::Dir(setup.dir));
    let failures: Vec<String> = editor
        .load_plugins(&sources)
        .into_iter()
        .filter_map(Result::err)
        .map(|err| format!("loading: {err}"))
        .collect();
    if !failures.is_empty() {
        return Err(failures);
    }
    if !marked.ranges.is_empty() {
        let selection = Selection::new(marked.ranges, marked.primary, editor.buffer().text())
            .map_err(|err| fail(format!("text: the selection does not fit: {err}")))?;
        editor.view_mut().selection = selection;
    }
    settle(&mut editor);
    for (n, step) in steps.iter().enumerate() {
        run_step(&mut editor, step).map_err(|problems| match steps.len() {
            1 => problems,
            _ => problems
                .into_iter()
                .map(|p| format!("step {}: {p}", n + 1))
                .collect(),
        })?;
    }
    Ok(())
}

fn run_step(editor: &mut Editor, step: &Step) -> Result<(), Vec<String>> {
    let keys = parse_keys(&step.keys).map_err(|err| vec![format!("keys: {err}")])?;
    for key in keys {
        editor.handle_key(key);
        settle(editor);
    }
    if let Some(text) = &step.paste {
        editor.handle_paste(text);
        settle(editor);
    }
    let call = |editor: &mut Editor| {
        step.command.as_ref().map(|command| {
            let result = editor.call_command(command, step.args.as_deref().unwrap_or("{}"));
            settle(editor);
            result
        })
    };
    let mut result = call(editor);
    let Some(wait) = step.wait else {
        return check(editor, &step.expect, result);
    };
    // Waiting for a command's answer calls it again, as for a status.
    let again = step.expect.result.is_some() || step.expect.error.is_some();
    // Runs the editor as the terminal does, until the expectations hold
    // or the time is up.
    let deadline = Instant::now() + Duration::from_millis(wait);
    let mut first = true;
    loop {
        if again && !first {
            result = call(editor);
        }
        first = false;
        let checked = check(editor, &step.expect, result.clone());
        let now = Instant::now();
        let waiting = step.expect.is_empty() || checked.is_err();
        if !waiting || now >= deadline {
            return checked;
        }
        editor.run_background();
        editor.run_timers();
        settle(editor);
        let next = editor
            .next_timer()
            .map_or(deadline, |due| due.min(deadline));
        let pause = next
            .saturating_duration_since(now)
            .min(Duration::from_millis(5));
        thread::sleep(pause);
    }
}

/// `path` without Windows' `\\?\` prefix, which `canonicalize` adds and
/// programs given the path, such as a test's fake server, may not take.
fn plain(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => path,
    }
}

/// `[settings]` from TOML as the JSON plugins get, with `{dir}` in strings
/// replaced by `folder`.
fn plugin_settings(table: &toml::Table, folder: &Path) -> serde_json::Value {
    fn replace(value: serde_json::Value, folder: &str) -> serde_json::Value {
        use serde_json::Value;
        match value {
            Value::String(s) => Value::String(s.replace("{dir}", folder)),
            Value::Array(items) => {
                Value::Array(items.into_iter().map(|v| replace(v, folder)).collect())
            }
            Value::Object(map) => Value::Object(
                map.into_iter()
                    .map(|(k, v)| (k, replace(v, folder)))
                    .collect(),
            ),
            other => other,
        }
    }
    let value = serde_json::to_value(table).expect("TOML is JSON");
    replace(value, &folder.to_string_lossy())
}

/// Does what the terminal loop does after each frame.
fn settle(editor: &mut Editor) {
    while editor.catch_up() {}
}

fn check(
    editor: &Editor,
    expect: &Expect,
    result: Option<Result<String, String>>,
) -> Result<(), Vec<String>> {
    let mut problems = Vec::new();
    let mut differ = |what: &str, expected: &str, actual: &str| {
        problems.push(format!(
            "{what}:\n  expected: {expected:?}\n  actual:   {actual:?}"
        ));
    };
    let text = editor.buffer().text().to_string();
    if let Some(expected) = &expect.text
        && *expected != text
    {
        differ("text", expected, &text);
    }
    if let Some(expected) = &expect.selections {
        let actual = marks::show(&text, &editor.view().selection);
        if *expected != actual {
            differ("selections", expected, &actual);
        }
    }
    if let Some(expected) = &expect.message {
        let actual = editor.message().unwrap_or_default();
        if expected != actual {
            differ("message", expected, actual);
        }
    }
    if let Some(expected) = &expect.screen {
        let rows = screen(editor);
        for line in expected {
            if !rows.iter().any(|row| row.contains(line.as_str())) {
                differ("screen", line, &rows.join("\n"));
            }
        }
    }
    if let Some(absent) = &expect.absent {
        let rows = screen(editor);
        for line in absent {
            if rows.iter().any(|row| row.contains(line.as_str())) {
                differ("absent", line, &rows.join("\n"));
            }
        }
    }
    match (result, &expect.result, &expect.error) {
        (Some(Ok(actual)), Some(expected), _) => {
            if !same_json(expected, &actual) {
                differ("result", expected, &actual);
            }
        }
        (Some(Err(actual)), _, Some(expected)) => {
            if !actual.contains(expected.as_str()) {
                differ("error", expected, &actual);
            }
        }
        (Some(Ok(actual)), None, Some(expected)) => {
            differ(
                "error",
                expected,
                &format!("no error; the result was {actual}"),
            );
        }
        (Some(Err(actual)), _, None) => problems.push(format!("command failed: {actual}")),
        (None, Some(_), _) | (None, _, Some(_)) => {
            problems.push("`result` and `error` need a `command` to call".into());
        }
        _ => {}
    }
    match problems.is_empty() {
        true => Ok(()),
        false => Err(problems),
    }
}

/// Compares as JSON if both are, so `{"a": 1}` matches `{"a":1}`.
fn same_json(expected: &str, actual: &str) -> bool {
    match (
        serde_json::from_str::<serde_json::Value>(expected),
        serde_json::from_str::<serde_json::Value>(actual),
    ) {
        (Ok(expected), Ok(actual)) => expected == actual,
        _ => expected == actual,
    }
}

fn screen(editor: &Editor) -> Vec<String> {
    let mut grid = Grid::default();
    editor.render(&mut grid);
    (0..grid.height())
        .map(|y| {
            grid.row(y)
                .iter()
                .filter_map(|cell| match &cell.symbol {
                    Symbol::Char(c) => Some(c.to_string()),
                    Symbol::Str(s) => Some(s.to_string()),
                    Symbol::Continuation => None,
                })
                .collect()
        })
        .collect()
}

/// A directory for the tests' files, removed afterwards.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Self, String> {
        let dir = std::env::temp_dir().join(format!("nib-plugin-test-{}", std::process::id()));
        fs::create_dir_all(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
        Ok(Self(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_paths_drop_the_verbatim_prefix() {
        assert_eq!(
            plain(PathBuf::from(r"\\?\D:\a\b")),
            PathBuf::from(r"D:\a\b")
        );
        // Network paths need it.
        assert_eq!(
            plain(PathBuf::from(r"\\?\UNC\server\share")),
            PathBuf::from(r"\\?\UNC\server\share")
        );
        assert_eq!(plain(PathBuf::from("/tmp/a")), PathBuf::from("/tmp/a"));
    }
}
