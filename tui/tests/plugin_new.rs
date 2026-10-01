//! `nib plugin new` makes plugins that `nib plugin build` builds and whose
//! tests `nib plugin test` passes, as docs/design/api/plugin-dev.md promises. Build the
//! standard plugins first with `cargo xtask build-plugins`, which also
//! fetches the crates the Rust template needs; the build here is offline.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .unwrap()
}

fn nib(args: &[&str], dir: &Path, envs: &[(&str, &str)]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_nib"))
        .args(args)
        .arg(dir)
        .envs(envs.iter().copied())
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout).to_string()
        + &String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "nib {args:?} failed:\n{text}");
    text
}

fn fresh(name: &str) -> PathBuf {
    let dir = env::temp_dir().join(format!("nib-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
}

#[test]
fn the_rust_template_builds_and_passes_its_tests() {
    let dir = fresh("new-rust");
    nib(&["plugin", "new", "demo"], &dir, &[]);
    // The SDK from this checkout, not from its tag.
    let manifest = dir.join("Cargo.toml");
    let text = fs::read_to_string(&manifest).unwrap();
    let from_git = text
        .lines()
        .find(|line| line.starts_with("nib-plugin = "))
        .unwrap()
        .to_string();
    let local = format!(
        "nib-plugin = {{ path = {:?} }}",
        root().join("sdk/rust").display().to_string()
    );
    fs::write(&manifest, text.replace(&from_git, &local)).unwrap();

    let target = root().join("target/plugin-new-test");
    let target = target.to_str().unwrap();
    let envs = [("CARGO_NET_OFFLINE", "true"), ("CARGO_TARGET_DIR", target)];
    nib(&["plugin", "build"], &dir, &envs);
    let out = nib(&["plugin", "test"], &dir, &[]);
    assert!(out.contains("3 passed, 0 failed"), "{out}");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_go_template_builds_and_passes_its_tests() {
    if Command::new("tinygo").arg("version").output().is_err() {
        eprintln!("skipped: no TinyGo");
        return;
    }
    let dir = fresh("new-go");
    nib(&["plugin", "new", "demo", "--go"], &dir, &[]);
    // The SDK from this checkout, not from its tag.
    let go_mod = dir.join("go.mod");
    let text = fs::read_to_string(&go_mod).unwrap();
    let local = root().join("sdk/go").display().to_string();
    let replace = format!("\nreplace github.com/nib-editor/nib/sdk/go => {local}\n");
    fs::write(&go_mod, text + &replace).unwrap();
    nib(&["plugin", "build"], &dir, &[]);
    let out = nib(&["plugin", "test"], &dir, &[]);
    assert!(out.contains("3 passed, 0 failed"), "{out}");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_existing_directory_is_left_alone() {
    let dir = fresh("new-existing");
    fs::create_dir_all(&dir).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_nib"))
        .args(["plugin", "new", "demo"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("already exists"));
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
    fs::remove_dir_all(&dir).unwrap();
}
