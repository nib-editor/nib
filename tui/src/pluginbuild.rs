//! `nib plugin build`: builds a plugin with the tools of its language and
//! puts `plugin.wasm` next to its `plugin.toml` (docs/design/api/plugin-dev.md).

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Deserialize;

pub fn run(dir: &Path) -> Result<(), String> {
    if !dir.join("plugin.toml").is_file() {
        return Err(format!("{} has no plugin.toml", dir.display()));
    }
    let wasm = if dir.join("Cargo.toml").is_file() {
        rust(dir)?
    } else if dir.join("go.mod").is_file() {
        go(dir)?
    } else {
        return Err(format!(
            "{} has neither Cargo.toml nor go.mod; nib builds Rust and Go plugins",
            dir.display()
        ));
    };
    let out = dir.join("plugin.wasm");
    if wasm != out {
        fs::copy(&wasm, &out).map_err(|err| format!("{}: {err}", out.display()))?;
    }
    println!("wrote {}", out.display());
    Ok(())
}

/// A line of `cargo build --message-format=json`, as far as we read it.
#[derive(Deserialize)]
struct Message {
    reason: String,
    #[serde(default)]
    target: Option<Target>,
    #[serde(default)]
    filenames: Vec<PathBuf>,
}

#[derive(Deserialize)]
struct Target {
    kind: Vec<String>,
}

/// Builds with cargo, and returns the component it made. Its path comes
/// from cargo, since a workspace or `CARGO_TARGET_DIR` moves it.
fn rust(dir: &Path) -> Result<PathBuf, String> {
    let mut command = Command::new("cargo");
    command
        .args(["build", "--release", "--target", "wasm32-wasip2"])
        .arg("--manifest-path")
        .arg(dir.join("Cargo.toml"))
        .arg("--message-format=json-render-diagnostics")
        .stdout(Stdio::piped());
    announce(&command);
    let mut child = command.spawn().map_err(|err| missing("cargo", err))?;
    let mut wasm = None;
    let stdout = child.stdout.take().expect("piped");
    for line in BufReader::new(stdout).lines() {
        let Ok(message) = serde_json::from_str::<Message>(&line.map_err(|e| e.to_string())?) else {
            continue;
        };
        let cdylib = message
            .target
            .is_some_and(|t| t.kind.iter().any(|k| k == "cdylib"));
        if message.reason == "compiler-artifact" && cdylib {
            wasm = message
                .filenames
                .into_iter()
                .find(|f| f.extension().is_some_and(|e| e == "wasm"));
        }
    }
    let status = child.wait().map_err(|err| err.to_string())?;
    if !status.success() {
        return Err(
            "cargo failed; if the target is missing, add it with `rustup target add wasm32-wasip2`"
                .into(),
        );
    }
    wasm.ok_or_else(|| {
        "cargo built no .wasm; the crate needs `crate-type = [\"cdylib\"]` under [lib]".into()
    })
}

/// Builds with TinyGo, as the Go SDK needs (docs/design/api/plugin-api.md).
fn go(dir: &Path) -> Result<PathBuf, String> {
    if !dir.join("go.sum").is_file() {
        let mut tidy = Command::new("go");
        tidy.args(["mod", "tidy"]).current_dir(dir);
        run_command(tidy, "go")?;
    }
    let mut list = Command::new("go");
    list.args([
        "list",
        "-m",
        "-f",
        "{{.Dir}}",
        "github.com/nib-editor/nib/sdk/go",
    ])
    .current_dir(dir);
    announce(&list);
    let output = list.output().map_err(|err| missing("go", err))?;
    if !output.status.success() {
        return Err(format!(
            "go list failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let sdk = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let mut build = Command::new("tinygo");
    build
        .args(["build", "-target=wasip2", "--wit-package"])
        .arg(Path::new(&sdk).join("wit"))
        .args(["--wit-world", "plugin", "-o", "plugin.wasm", "."])
        .current_dir(dir);
    run_command(build, "tinygo")?;
    Ok(dir.join("plugin.wasm"))
}

fn run_command(mut command: Command, tool: &str) -> Result<(), String> {
    announce(&command);
    let status = command.status().map_err(|err| missing(tool, err))?;
    match status.success() {
        true => Ok(()),
        false => Err(format!("{tool} failed")),
    }
}

/// Says what runs, so a failure can be tried by hand.
fn announce(command: &Command) {
    let words: Vec<_> = std::iter::once(command.get_program())
        .chain(command.get_args())
        .map(|w| w.to_string_lossy())
        .collect();
    eprintln!("running: {}", words.join(" "));
}

fn missing(tool: &str, err: std::io::Error) -> String {
    if err.kind() != std::io::ErrorKind::NotFound {
        return format!("{tool}: {err}");
    }
    let how = match tool {
        "cargo" => "install Rust from https://rustup.rs, then `rustup target add wasm32-wasip2`",
        "go" => "install Go from https://go.dev",
        _ => {
            "install TinyGo 0.42 or later from https://tinygo.org, with binaryen's wasm-opt and wasm-tools"
        }
    };
    format!("{tool} is not installed; {how}")
}
