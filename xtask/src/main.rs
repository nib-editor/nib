//! Build steps that plain cargo cannot express. Run with `cargo xtask <task>`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const USAGE: &str = "usage: cargo xtask build-plugins | package";

fn main() -> ExitCode {
    let result = match std::env::args().nth(1).as_deref() {
        Some("build-plugins") => build_plugins(),
        Some("package") => package(),
        _ => Err(USAGE.into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("xtask: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Readies the crates for crates.io (docs/distribution.md): the built
/// standard plugins go into `tui/plugins/`, as a source build there cannot
/// build them, and the licenses next to each crate's manifest.
fn package() -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives in the repository root");
    for krate in ["core", "tui"] {
        for license in ["LICENSE-MIT", "LICENSE-APACHE"] {
            let to = root.join(krate).join(license);
            fs::copy(root.join(license), &to).map_err(|err| format!("{}: {err}", to.display()))?;
        }
    }
    let list = root.join("tui/standard-plugins.txt");
    let list = fs::read_to_string(&list).map_err(|err| format!("{}: {err}", list.display()))?;
    let out = root.join("tui/plugins");
    let _ = fs::remove_dir_all(&out);
    for name in list
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        let built = root.join("target/plugins").join(name);
        if !built.join("plugin.toml").is_file() {
            return Err(format!(
                "{} is missing; run `cargo xtask build-plugins` first",
                built.display()
            ));
        }
        copy_tree(&built, &out.join(name))?;
    }
    println!("copied the standard plugins to {}", out.display());
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|err| format!("{}: {err}", to.display()))?;
    let entries = fs::read_dir(from).map_err(|err| format!("{}: {err}", from.display()))?;
    for entry in entries {
        let path = entry.map_err(|err| err.to_string())?.path();
        let dest = to.join(path.file_name().expect("an entry has a name"));
        if path.is_dir() {
            copy_tree(&path, &dest)?;
        } else {
            fs::copy(&path, &dest).map_err(|err| format!("{}: {err}", path.display()))?;
        }
    }
    Ok(())
}

/// Builds every plugin under `plugins/` for wasm32-wasip2 and lays them out
/// as `target/plugins/<name>/{plugin.toml,plugin.wasm}`.
fn build_plugins() -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives in the repository root");
    check_wasm_target()?;
    let target = root.join("target");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(cargo)
        .args([
            "build",
            "--release",
            "--workspace",
            "--target",
            "wasm32-wasip2",
        ])
        .arg("--manifest-path")
        .arg(root.join("plugins/Cargo.toml"))
        .arg("--target-dir")
        .arg(&target)
        .status()
        .map_err(|err| format!("failed to run cargo: {err}"))?;
    if !status.success() {
        return Err("building plugins failed".into());
    }

    for dir in plugin_dirs(&root.join("plugins"))? {
        let manifest = read_toml(&dir.join("plugin.toml"))?;
        let name = manifest["name"]
            .as_str()
            .ok_or_else(|| format!("{}: plugin.toml has no name", dir.display()))?;
        let out = target.join("plugins").join(name);
        if out.exists() {
            fs::remove_dir_all(&out).map_err(|err| format!("{}: {err}", out.display()))?;
        }
        fs::create_dir_all(&out).map_err(|err| format!("{}: {err}", out.display()))?;

        // Go plugins build with TinyGo, which is optional: without it they
        // are left out, and so are the tests that use them.
        if dir.join("go.mod").is_file() && !build_go(root, &dir, &out)? {
            fs::remove_dir_all(&out).map_err(|err| format!("{}: {err}", out.display()))?;
            println!("skipped {name}: tinygo is not installed");
            continue;
        }
        // A plugin with a Cargo.toml has code; one without is data only.
        if dir.join("Cargo.toml").is_file() {
            let package = read_toml(&dir.join("Cargo.toml"))?;
            let package = package["package"]["name"]
                .as_str()
                .ok_or_else(|| format!("{}: no package name", dir.display()))?;
            let wasm = target
                .join("wasm32-wasip2/release")
                .join(format!("{}.wasm", package.replace('-', "_")));
            copy(&wasm, &out.join("plugin.wasm"))?;
        }
        copy_data(&dir, &out)?;
        for fetch in manifest
            .get("fetch")
            .and_then(|f| f.as_array())
            .into_iter()
            .flatten()
        {
            let field = |key: &str| {
                fetch
                    .get(key)
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| format!("{}: fetch needs {key}", dir.display()))
            };
            let file = fetch_checked(&target, field("url")?, field("sha256")?)?;
            copy(&file, &out.join(field("file")?))?;
        }
        println!("built {name} -> {}", out.display());
    }
    Ok(())
}

/// Copies a plugin's files besides its code: the manifest, queries, and
/// such.
fn copy_data(from: &Path, to: &Path) -> Result<(), String> {
    let entries = fs::read_dir(from).map_err(|err| format!("{}: {err}", from.display()))?;
    for entry in entries {
        let path = entry.map_err(|err| err.to_string())?.path();
        let name = path.file_name().unwrap_or_default();
        let source = [
            "Cargo.toml",
            "Cargo.lock",
            "src",
            "target",
            "go.mod",
            "go.sum",
        ]
        .iter()
        .any(|skip| name == *skip)
            || path.extension().is_some_and(|e| e == "go");
        if source {
            continue;
        }
        let dest = to.join(name);
        if path.is_dir() {
            fs::create_dir_all(&dest).map_err(|err| format!("{}: {err}", dest.display()))?;
            copy_data(&path, &dest)?;
        } else {
            copy(&path, &dest)?;
        }
    }
    Ok(())
}

/// Builds the Go plugin in `dir` into `out/plugin.wasm` with TinyGo and
/// the Go SDK's copy of the WIT. Returns false when TinyGo is missing.
fn build_go(root: &Path, dir: &Path, out: &Path) -> Result<bool, String> {
    let status = Command::new("tinygo")
        .args(["build", "-target=wasip2", "--wit-package"])
        .arg(root.join("sdk/go/wit"))
        .args(["--wit-world", "plugin", "-o"])
        .arg(out.join("plugin.wasm"))
        .arg(".")
        .current_dir(dir)
        .status();
    match status {
        Ok(status) if status.success() => Ok(true),
        Ok(_) => Err(format!("building {} with tinygo failed", dir.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(format!("running tinygo failed: {err}")),
    }
}

/// Downloads `url` once into `target/downloads`, keyed by its SHA-256, and
/// fails if the content does not match.
fn fetch_checked(target: &Path, url: &str, sha256: &str) -> Result<PathBuf, String> {
    let dir = target.join("downloads");
    let file = dir.join(sha256);
    if file.is_file() && hash(&file)? == sha256 {
        return Ok(file);
    }
    fs::create_dir_all(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    let partial = dir.join(format!("{sha256}.part"));
    let status = Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--output",
        ])
        .arg(&partial)
        .arg(url)
        .status()
        .map_err(|err| format!("failed to run curl: {err}"))?;
    if !status.success() {
        return Err(format!("downloading {url} failed"));
    }
    let actual = hash(&partial)?;
    if actual != sha256 {
        let _ = fs::remove_file(&partial);
        return Err(format!("{url} has SHA-256 {actual}, expected {sha256}"));
    }
    fs::rename(&partial, &file).map_err(|err| format!("{}: {err}", file.display()))?;
    Ok(file)
}

fn hash(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let bytes = fs::read(path).map_err(|err| format!("{}: {err}", path.display()))?;
    Ok(Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// Fails early with a fix, instead of a missing `core` crate deep in the
/// build, when the active toolchain lacks the wasm target. Tool managers
/// such as mise can select a different toolchain than the default one.
fn check_wasm_target() -> Result<(), String> {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let run = |arg: &str| -> Result<String, String> {
        let output = Command::new(&rustc)
            .args(["--print", arg])
            .output()
            .map_err(|err| format!("failed to run {rustc}: {err}"))?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    };
    let sysroot = PathBuf::from(run("sysroot")?);
    if sysroot.join("lib/rustlib/wasm32-wasip2").is_dir() {
        return Ok(());
    }
    Err(format!(
        "the wasm32-wasip2 target is not installed for the toolchain at {}\n\
         install it with: rustup target add wasm32-wasip2",
        sysroot.display()
    ))
}

/// Directories under `dir` that contain a `plugin.toml`.
fn plugin_dirs(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut found = Vec::new();
    let entries = fs::read_dir(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    for entry in entries {
        let path = entry.map_err(|err| err.to_string())?.path();
        if !path.is_dir() || path.ends_with("target") || path.ends_with("src") {
            continue;
        }
        if path.join("plugin.toml").is_file() {
            found.push(path);
        } else {
            found.extend(plugin_dirs(&path)?);
        }
    }
    found.sort();
    Ok(found)
}

fn read_toml(path: &Path) -> Result<toml::Table, String> {
    fs::read_to_string(path)
        .map_err(|err| format!("{}: {err}", path.display()))?
        .parse()
        .map_err(|err| format!("{}: {err}", path.display()))
}

fn copy(from: &Path, to: &Path) -> Result<(), String> {
    fs::copy(from, to)
        .map(|_| ())
        .map_err(|err| format!("copying {} failed: {err}", from.display()))
}
