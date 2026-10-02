//! `nib plugin test`, run as people run it. Build the plugins first with
//! `cargo xtask build-plugins`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn built(name: &str) -> PathBuf {
    let dir = root().join("target/plugins").join(name);
    assert!(
        dir.join("plugin.toml").is_file(),
        "{} is missing; run `cargo xtask build-plugins` first",
        dir.display()
    );
    dir
}

fn nib_plugin_test(dir: &Path, file: &Path) -> (bool, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_nib"))
        .args(["plugin", "test"])
        .arg(dir)
        .arg(file)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout).to_string()
        + &String::from_utf8_lossy(&output.stderr);
    (output.status.success(), text)
}

/// Runs a standard plugin's tests in `plugins/<dir>/tests/`.
fn passes_its_tests(name: &str, dir: &str, count: usize) {
    let tests = root().join("plugins").join(dir).join("tests");
    let mut files: Vec<_> = fs::read_dir(&tests)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "toml"))
        .collect();
    files.sort();
    let mut out = String::new();
    let mut ok = true;
    for file in files {
        let (passed, text) = nib_plugin_test(&built(name), &file);
        ok &= passed;
        out += &text;
    }
    assert!(ok, "{out}");
    let passed: usize = out
        .lines()
        .filter_map(|line| line.split_once(" passed, 0 failed"))
        .map(|(n, _)| n.parse::<usize>().unwrap())
        .sum();
    assert_eq!(passed, count, "{out}");
}

#[test]
fn the_helix_plugin_passes_its_tests() {
    passes_its_tests("helix", "helix", 11);
}

#[test]
fn the_indent_plugin_passes_its_tests() {
    passes_its_tests("indent", "indent", 5);
}

#[test]
fn the_nano_plugin_passes_its_tests() {
    passes_its_tests("nano", "nano", 14);
}

#[test]
fn the_vim_plugin_passes_its_tests() {
    passes_its_tests("vim", "vim", 13);
}

#[test]
fn the_emacs_plugin_passes_its_tests() {
    passes_its_tests("emacs", "emacs", 31);
}

#[test]
fn the_picker_plugin_passes_its_tests() {
    passes_its_tests("picker", "picker", 12);
}

/// Its fake language server is a Python script.
#[test]
fn the_lsp_plugin_passes_its_tests() {
    // Windows may have a python3 that only points to the store.
    let python = Command::new("python3").arg("--version").output();
    let found = python.is_ok_and(|out| {
        out.status.success() && String::from_utf8_lossy(&out.stdout).starts_with("Python 3")
    });
    if !found {
        // CI has Python everywhere; a skip there would hide a broken test.
        assert!(env::var_os("CI").is_none(), "no python3 on CI");
        eprintln!("skipped: no python3");
        return;
    }
    passes_its_tests("lsp", "lsp", 3);
}

#[test]
fn failures_show_what_was_expected() {
    let file = env::temp_dir().join(format!("nib-{}-failing.toml", std::process::id()));
    fs::write(
        &file,
        "with = []\n\
         [[test]]\nname = \"wrong keys\"\ntext = \"#[x|]#\\n\"\nkeys = \"a<nope>\"\n\
         [[test]]\nname = \"wrong text\"\ntext = \"x\\n\"\n\
         [test.expect]\ntext = \"y\\n\"\n",
    )
    .unwrap();
    let (ok, out) = nib_plugin_test(&built("test-insert"), &file);
    fs::remove_file(&file).unwrap();
    assert!(!ok, "{out}");
    assert!(out.contains("wrong keys ... FAILED"), "{out}");
    assert!(out.contains("keys: unknown key \"nope\""), "{out}");
    assert!(out.contains("expected: \"y\\n\""), "{out}");
    assert!(out.contains("actual:   \"x\\n\""), "{out}");
    assert!(out.contains("0 passed, 2 failed"), "{out}");
}

#[test]
fn waits_for_timers() {
    let file = env::temp_dir().join(format!("nib-{}-timer.toml", std::process::id()));
    fs::write(
        &file,
        r#"with = []

[[test]]
name = "a timer fires after waiting"

[[test.step]]
command = "test-events.timer"
args = "100"

[[test.step]]
command = "test-events.log"
expect.result = "opened test.txt"

[[test.step]]
wait = 300

[[test.step]]
command = "test-events.log"
expect.result = "timer 1"
"#,
    )
    .unwrap();
    let (ok, out) = nib_plugin_test(&built("test-events"), &file);
    fs::remove_file(&file).unwrap();
    assert!(ok, "{out}");
}

#[test]
fn each_test_gets_an_empty_data_directory() {
    let file = env::temp_dir().join(format!("nib-{}-data.toml", std::process::id()));
    fs::write(
        &file,
        r#"with = []

[[test]]
name = "writes to /data"

[[test.step]]
command = "test-events.save"
args = "kept"

[[test.step]]
command = "test-events.load"
expect.result = "kept"

[[test]]
name = "starts without what the last test wrote"
command = "test-events.load"
expect.error = "No such file"
"#,
    )
    .unwrap();
    let (ok, out) = nib_plugin_test(&built("test-events"), &file);
    fs::remove_file(&file).unwrap();
    assert!(ok, "{out}");
}
