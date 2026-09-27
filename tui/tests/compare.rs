//! Compares the vim and emacs bases with the editors they follow
//! (docs/base.md): each case's keys go to `nvim --headless` or
//! `emacs --batch` and to nib, and the text and cursor must come out the
//! same. The cases are in `plugins/<base>/compare.toml`; ones marked
//! `differs` must still differ, so the list of differences stays true.
//!
//! Skipped when the editor is not installed, unless NIB_REQUIRE_EDITORS is
//! set, as on CI. Build the plugins first with `cargo xtask build-plugins`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs, thread};

use nib_core::selection::{Range, Selection};
use nib_core::{Buffer, KeyCode, KeyEvent, marks, parse_keys};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cases {
    case: Vec<Case>,
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct Case {
    /// The text, ending with a line break, with the cursor as a point:
    /// `#[|]#`.
    text: String,
    /// Keys as nib's tests write them, such as `d2w` or `<C-x><C-s>`.
    keys: String,
    /// Why nib does otherwise, for a known difference.
    differs: Option<String>,
}

/// What an editor left: the text, the cursor, and where the mark is if
/// the region is active.
#[derive(Debug, PartialEq)]
struct Outcome {
    text: String,
    cursor: usize,
    mark: Option<usize>,
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn scratch(name: &str) -> PathBuf {
    let dir = env::temp_dir().join(format!("nib-compare-{}-{name}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Whether `program` runs; if not, the test is skipped, or fails where the
/// editors are required.
fn available(program: &str, arg: &str) -> bool {
    let found = Command::new(program)
        .arg(arg)
        .output()
        .is_ok_and(|out| out.status.success());
    if !found {
        assert!(
            env::var_os("NIB_REQUIRE_EDITORS").is_none(),
            "{program} is required but was not found"
        );
        eprintln!("skipped: no {program}");
    }
    found
}

const NVIM_SCRIPT: &str = r#"
local case = vim.json.decode(io.open(os.getenv("CASE")):read("a"))
vim.o.shada = ""
vim.o.more = false
local lines = vim.split(case.text, "\n", { plain = true })
vim.api.nvim_buf_set_lines(0, 0, -1, false, lines)
vim.api.nvim_win_set_cursor(0, { case.line + 1, case.col })
local ok, err = pcall(vim.api.nvim_feedkeys,
  vim.api.nvim_replace_termcodes(case.keys, true, false, true), "xt", false)
local out = {
  text = table.concat(vim.api.nvim_buf_get_lines(0, 0, -1, false), "\n"),
  line = vim.api.nvim_win_get_cursor(0)[1] - 1,
  col = vim.api.nvim_win_get_cursor(0)[2],
  mode = vim.api.nvim_get_mode().mode,
  error = (not ok) and tostring(err) or nil,
}
local f = io.open(os.getenv("OUT"), "w")
f:write(vim.json.encode(out))
f:close()
vim.cmd("qa!")
"#;

const EMACS_SCRIPT: &str = r#";;; -*- lexical-binding: t -*-
(require 'json)
(let* ((case (json-read-file (getenv "CASE")))
       (out (getenv "OUT")))
  (with-current-buffer (get-buffer-create "compare")
    (switch-to-buffer (current-buffer))
    (fundamental-mode)
    (transient-mark-mode 1)
    (setq indent-tabs-mode nil)
    (insert (alist-get 'text case))
    (goto-char (1+ (alist-get 'point case)))
    (setq buffer-undo-list nil)
    (let ((err (condition-case e
                   (progn (execute-kbd-macro (kbd (alist-get 'keys case))) nil)
                 (error (error-message-string e)))))
      (with-temp-file out
        (insert (json-encode
                 `((text . ,(with-current-buffer "compare"
                              (buffer-substring-no-properties (point-min) (point-max))))
                   (point . ,(with-current-buffer "compare" (1- (point))))
                   (mark . ,(with-current-buffer "compare"
                              (and (region-active-p) (1- (mark)))))
                   (error . ,err))))))))
"#;

/// The keys in the notation of vim's `nvim_replace_termcodes`.
fn vim_notation(keys: &[KeyEvent]) -> String {
    keys.iter()
        .map(|key| {
            let m = key.modifiers;
            let name = match key.code {
                KeyCode::Char('<') if !m.ctrl && !m.alt => return "<lt>".to_string(),
                KeyCode::Char(c) if !m.ctrl && !m.alt => return c.to_string(),
                KeyCode::Char(' ') => "Space".to_string(),
                KeyCode::Char(c) => c.to_string(),
                KeyCode::Enter => "CR".into(),
                KeyCode::Escape => "Esc".into(),
                KeyCode::Tab => "Tab".into(),
                KeyCode::Backspace => "BS".into(),
                KeyCode::Delete => "Del".into(),
                KeyCode::Up => "Up".into(),
                KeyCode::Down => "Down".into(),
                KeyCode::Left => "Left".into(),
                KeyCode::Right => "Right".into(),
                KeyCode::Home => "Home".into(),
                KeyCode::End => "End".into(),
                KeyCode::PageUp => "PageUp".into(),
                KeyCode::PageDown => "PageDown".into(),
                KeyCode::F(n) => format!("F{n}"),
            };
            let mut prefix = String::new();
            for (on, p) in [(m.ctrl, "C-"), (m.alt, "M-"), (m.shift, "S-")] {
                if on {
                    prefix.push_str(p);
                }
            }
            format!("<{prefix}{name}>")
        })
        .collect()
}

/// The keys as Emacs's `kbd` reads them.
fn emacs_notation(keys: &[KeyEvent]) -> String {
    let words: Vec<String> = keys
        .iter()
        .map(|key| {
            let m = key.modifiers;
            let name = match key.code {
                KeyCode::Char(' ') => "SPC".to_string(),
                KeyCode::Char(c) => c.to_string(),
                KeyCode::Enter => "RET".into(),
                KeyCode::Escape => "ESC".into(),
                KeyCode::Tab => "TAB".into(),
                KeyCode::Backspace => "DEL".into(),
                KeyCode::Delete => "<deletechar>".into(),
                KeyCode::Up => "<up>".into(),
                KeyCode::Down => "<down>".into(),
                KeyCode::Left => "<left>".into(),
                KeyCode::Right => "<right>".into(),
                KeyCode::Home => "<home>".into(),
                KeyCode::End => "<end>".into(),
                KeyCode::PageUp => "<prior>".into(),
                KeyCode::PageDown => "<next>".into(),
                KeyCode::F(n) => format!("<f{n}>"),
            };
            let mut word = String::new();
            for (on, p) in [(m.ctrl, "C-"), (m.alt, "M-"), (m.shift, "S-")] {
                if on {
                    word.push_str(p);
                }
            }
            word + &name
        })
        .collect();
    words.join(" ")
}

/// The case's text and where its cursor is, in bytes.
fn start(case: &Case) -> Result<(String, usize), String> {
    let marked = marks::parse(&case.text)?;
    match marked.ranges[..] {
        [r] if r.anchor == r.head => Ok((marked.text, r.head)),
        _ => Err("the text needs one cursor, written #[|]#".into()),
    }
}

fn run_real(
    program: &str,
    args: &[&str],
    dir: &Path,
    case: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let n = thread_name();
    let input = dir.join(format!("case-{n}.json"));
    let output = dir.join(format!("out-{n}.json"));
    let _ = fs::remove_file(&output);
    fs::write(&input, case.to_string()).map_err(|e| e.to_string())?;
    let status = Command::new(program)
        .args(args)
        .env("CASE", &input)
        .env("OUT", &output)
        .output()
        .map_err(|e| e.to_string())?;
    let text = fs::read_to_string(&output).map_err(|_| {
        format!(
            "{program} left no result: {}",
            String::from_utf8_lossy(&status.stderr)
        )
    })?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

fn thread_name() -> String {
    format!("{:?}", thread::current().id()).replace(|c: char| !c.is_alphanumeric(), "")
}

fn byte_of_char(text: &str, chars: usize) -> usize {
    text.char_indices()
        .nth(chars)
        .map_or(text.len(), |(i, _)| i)
}

fn nvim(dir: &Path, case: &Case) -> Result<Outcome, String> {
    let (text, cursor) = start(case)?;
    let body = text
        .strip_suffix('\n')
        .ok_or("the text needs to end with a line break")?;
    let line = text[..cursor].matches('\n').count();
    let col = cursor - text[..cursor].rfind('\n').map_or(0, |i| i + 1);
    let keys = parse_keys(&case.keys)?;
    let input = serde_json::json!({
        "text": body, "line": line, "col": col, "keys": vim_notation(&keys),
    });
    let script = dir.join("compare.lua");
    let out = run_real(
        "nvim",
        &[
            "--headless",
            "--clean",
            "-n",
            "-l",
            &script.to_string_lossy(),
        ],
        dir,
        &input,
    )?;
    if let Some(err) = out["error"].as_str() {
        return Err(format!("nvim: {err}"));
    }
    let mode = out["mode"].as_str().unwrap_or_default();
    if mode != "n" {
        return Err(format!(
            "nvim ended in mode {mode:?}; end the keys in normal mode"
        ));
    }
    let result = format!("{}\n", out["text"].as_str().unwrap_or_default());
    let line = out["line"].as_u64().unwrap_or(0) as usize;
    let col = out["col"].as_u64().unwrap_or(0) as usize;
    let line_start: usize = result.split_inclusive('\n').take(line).map(str::len).sum();
    Ok(Outcome {
        cursor: line_start + col,
        text: result,
        mark: None,
    })
}

fn emacs(dir: &Path, case: &Case) -> Result<Outcome, String> {
    let (text, cursor) = start(case)?;
    let keys = parse_keys(&case.keys)?;
    let input = serde_json::json!({
        "text": text, "point": text[..cursor].chars().count(), "keys": emacs_notation(&keys),
    });
    let script = dir.join("compare.el");
    let out = run_real(
        "emacs",
        &["--batch", "-Q", "-l", &script.to_string_lossy()],
        dir,
        &input,
    )?;
    if let Some(err) = out["error"].as_str() {
        return Err(format!("emacs: {err}"));
    }
    let result = out["text"].as_str().unwrap_or_default().to_string();
    let point = out["point"].as_u64().unwrap_or(0) as usize;
    let mark = out["mark"]
        .as_u64()
        .map(|m| byte_of_char(&result, m as usize));
    Ok(Outcome {
        cursor: byte_of_char(&result, point),
        mark,
        text: result,
    })
}

/// The outcome as nib's tests write selections.
fn selections(outcome: &Outcome) -> String {
    let buffer = Buffer::with_text(&outcome.text);
    let range = Range::new(outcome.mark.unwrap_or(outcome.cursor), outcome.cursor);
    let selection =
        Selection::new(vec![range], 0, buffer.text()).expect("the cursor is in the text");
    marks::show(&outcome.text, &selection)
}

/// Runs the cases of `base` against `real`, and checks nib's results.
fn compare(base: &str, real: fn(&Path, &Case) -> Result<Outcome, String>, script: (&str, &str)) {
    let dir = scratch(base);
    fs::write(dir.join(script.0), script.1).unwrap();
    let file = root().join("plugins").join(base).join("compare.toml");
    let cases: Cases = toml::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    let only = env::var("NIB_COMPARE_ONLY").ok();
    let cases: Vec<(usize, Case)> = cases
        .case
        .into_iter()
        .enumerate()
        .filter(|(_, c)| only.as_ref().is_none_or(|o| c.keys.contains(o.as_str())))
        .collect();
    // The real editors are slow to start; several run at once.
    let outcomes: Vec<Result<Outcome, String>> = thread::scope(|s| {
        let chunks: Vec<_> = cases
            .chunks(cases.len().div_ceil(8).max(1))
            .map(|chunk| {
                let dir = &dir;
                s.spawn(move || chunk.iter().map(|(_, c)| real(dir, c)).collect::<Vec<_>>())
            })
            .collect();
        chunks.into_iter().flat_map(|h| h.join().unwrap()).collect()
    });
    let mut problems = Vec::new();
    let mut tests = toml::Table::new();
    let mut list = Vec::new();
    for ((n, case), outcome) in cases.iter().zip(&outcomes) {
        match outcome {
            Ok(outcome) => {
                let mut test = toml::Table::new();
                test.insert("name".into(), format!("{n} {}", case.keys).into());
                test.insert("text".into(), case.text.clone().into());
                test.insert("keys".into(), case.keys.clone().into());
                let mut expect = toml::Table::new();
                expect.insert("selections".into(), selections(outcome).into());
                test.insert("expect".into(), expect.into());
                list.push(toml::Value::Table(test));
            }
            Err(err) => problems.push(format!("case {n} ({}): {err}", case.keys)),
        }
    }
    tests.insert("with".into(), toml::Value::Array(Vec::new()));
    tests.insert("test".into(), toml::Value::Array(list));
    let generated = dir.join("generated.toml");
    fs::write(&generated, toml::to_string(&tests).unwrap()).unwrap();
    let plugin = root().join("target/plugins").join(base);
    assert!(
        plugin.join("plugin.toml").is_file(),
        "{} is missing; run `cargo xtask build-plugins` first",
        plugin.display()
    );
    let output = Command::new(env!("CARGO_BIN_EXE_nib"))
        .args(["plugin", "test"])
        .arg(&plugin)
        .arg(&generated)
        .output()
        .unwrap();
    let report = String::from_utf8_lossy(&output.stdout).to_string();
    let (mut same, mut known) = (0, 0);
    for (n, case) in &cases {
        let name = format!("{n} {}", case.keys);
        let Some(at) = report.find(&format!(": {name} ... ")) else {
            continue;
        };
        let rest = &report[at..];
        let passed = rest[..rest.find('\n').unwrap_or(rest.len())].ends_with("ok");
        match (passed, &case.differs) {
            (true, None) => same += 1,
            (false, Some(_)) => known += 1,
            (true, Some(why)) => problems.push(format!(
                "case {n} ({}) is the same now; take out its `differs` ({why})",
                case.keys
            )),
            (false, None) => {
                let detail: String = rest
                    .lines()
                    .skip(1)
                    .take_while(|l| l.starts_with("    "))
                    .collect::<Vec<_>>()
                    .join("\n");
                problems.push(format!("case {n} ({}):\n{detail}", case.keys));
            }
        }
    }
    let _ = fs::remove_dir_all(&dir);
    eprintln!(
        "{base}: {same} of {} cases as the real editor, {known} known differences",
        cases.len()
    );
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn vim_does_as_neovim_does() {
    if available("nvim", "--version") {
        compare("vim", nvim, ("compare.lua", NVIM_SCRIPT));
    }
}

#[test]
fn emacs_does_as_emacs_does() {
    if available("emacs", "--version") && root().join("plugins/emacs/compare.toml").is_file() {
        compare("emacs", emacs, ("compare.el", EMACS_SCRIPT));
    }
}

#[test]
fn keys_are_written_for_each_editor() {
    let keys = parse_keys("d<C-w>x<lt><ret><A-f> <esc>").unwrap();
    assert_eq!(vim_notation(&keys), "d<C-w>x<lt><CR><M-f> <Esc>");
    assert_eq!(emacs_notation(&keys), "d C-w x < RET M-f SPC ESC");
}
