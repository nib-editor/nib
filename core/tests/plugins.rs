//! Runs the test plugins in `plugins/test`. Build them first with
//! `cargo xtask build-plugins`.

use std::time::{Duration, Instant};
use std::{env, fs, thread};

use nib_core::{Config, Editor, KeyCode, KeyEvent, Menu, PluginOptions, PluginSource, Range};

mod common;
use common::{key, plugin_dir, screen};

fn editor_with(name: &str, options: PluginOptions) -> Editor {
    let mut editor = Editor::default();
    editor.set_plugin_options(options);
    editor.load_plugin(&plugin_dir(name)).unwrap();
    editor
}

#[test]
fn plugin_edits_the_buffer() {
    let mut editor = editor_with("test-insert", PluginOptions::default());
    for c in "hi あ".chars() {
        editor.handle_key(key(c));
    }
    assert_eq!(editor.buffer().text().to_string(), "hi あ");
    assert_eq!(editor.view().selection.primary(), Range::point(6));
    assert_eq!(editor.message(), None);

    // Ctrl-g opens the core menu instead of reaching the plugin, and the key
    // that closes the menu does not reach it either.
    editor.handle_key(KeyEvent::ctrl('g'));
    assert_eq!(editor.menu(), Some(Menu::Main));
    editor.handle_key(KeyEvent::new(KeyCode::Escape));
    assert_eq!(editor.menu(), None);
    editor.handle_key(key('!'));
    assert_eq!(editor.buffer().text().to_string(), "hi あ!");

    // Ctrl-g never reaches the plugin: pressed again, it closes the menu.
    editor.handle_key(KeyEvent::ctrl('g'));
    editor.handle_key(KeyEvent::ctrl('g'));
    assert_eq!(editor.menu(), None);
    assert_eq!(editor.buffer().text().to_string(), "hi あ!");

    // Other Ctrl keys do reach it.
    editor.handle_key(KeyEvent::ctrl('x'));
    assert_eq!(editor.buffer().text().to_string(), "hi あ!^X");

    // Escape pops the plugin's layer.
    assert_eq!(editor.key_hint(), None);
    editor.handle_key(KeyEvent::new(KeyCode::Escape));
    assert!(editor.key_hint().is_some());

    // Unsaved changes: quitting asks first.
    editor.handle_key(KeyEvent::ctrl('g'));
    editor.handle_key(key('q'));
    editor.handle_key(key('y'));
    assert!(editor.should_quit());
}

#[test]
fn failing_plugin_is_restarted_then_disabled() {
    let mut editor = editor_with(
        "test-misbehave",
        PluginOptions {
            call_timeout: Duration::from_millis(100),
            memory_limit: 64 << 20,
            ..PluginOptions::default()
        },
    );

    let started = Instant::now();
    editor.handle_key(key('l'));
    assert!(started.elapsed() < Duration::from_secs(1));
    let message = editor.message().unwrap();
    assert!(message.contains("restarted"), "{message}");
    assert!(message.contains("took too long"), "{message}");

    // Restarted: its layer is back.
    assert_eq!(editor.key_hint(), None);

    editor.handle_key(key('p'));
    let message = editor.message().unwrap();
    assert!(message.contains("panicked: asked to panic"), "{message}");

    // Allocating more than the memory limit is the third failure.
    editor.handle_key(key('m'));
    let message = editor.message().unwrap();
    assert!(message.contains("disabled"), "{message}");
    assert!(!editor.plugins()[0].enabled);

    assert!(editor.key_hint().is_some());

    // The core menu brings it back.
    editor.handle_key(KeyEvent::ctrl('g'));
    editor.handle_key(key('r'));
    assert_eq!(editor.message(), Some("plugins restarted"));
    assert!(editor.plugins()[0].enabled);
    assert_eq!(editor.key_hint(), None);
}

#[test]
fn init_error_is_reported() {
    let mut editor = Editor::default();
    editor.apply_config(with_plugin("test-misbehave", "[settings]\nfail = true"));
    let err = editor
        .load_plugin(&plugin_dir("test-misbehave"))
        .unwrap_err();
    assert!(err.to_string().contains("asked to fail"), "{err}");
    assert!(editor.plugins().is_empty());
    // Whatever the plugin did in `init` was undone.
    assert!(editor.key_hint().is_some());
}

#[test]
fn api_version_mismatch_is_rejected() {
    let dir = env::temp_dir().join(format!("nib-{}-old-plugin", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    fs::copy(
        plugin_dir("test-insert").join("plugin.wasm"),
        dir.join("plugin.wasm"),
    )
    .unwrap();
    fs::write(
        dir.join("plugin.toml"),
        "name = \"old\"\nversion = \"0.0.0\"\napi = \"0.0\"\n",
    )
    .unwrap();

    let err = Editor::default().load_plugin(&dir).unwrap_err();
    fs::remove_dir_all(&dir).unwrap();
    assert!(err.to_string().contains("needs API 0.0"), "{err}");
}

#[test]
fn plugins_load_together_in_order() {
    let missing = env::temp_dir().join(format!("nib-{}-no-plugin", std::process::id()));
    let (insert, events, rust) = (
        plugin_dir("test-insert"),
        plugin_dir("test-events"),
        plugin_dir("rust"),
    );
    let mut editor = Editor::default();
    let loaded = editor.load_plugins(&[
        PluginSource::Dir(&insert),
        PluginSource::Dir(&missing),
        PluginSource::Dir(&rust),
        PluginSource::Dir(&events),
    ]);
    assert!(loaded[0].is_ok() && loaded[2].is_ok() && loaded[3].is_ok());
    // One that fails leaves the others loaded.
    assert!(loaded[1].is_err());
    let names: Vec<_> = editor.plugins().into_iter().map(|p| p.name).collect();
    assert_eq!(names, ["test-insert", "rust", "test-events"]);
    editor.handle_key(key('x'));
    assert_eq!(editor.buffer().text().to_string(), "x");
}

/// Prints the cost of sending one key through a plugin. Run with
/// `cargo test --release -p nib-core --test plugins -- --ignored --nocapture`.
#[test]
#[ignore]
fn key_latency() {
    let mut editor = editor_with("test-insert", PluginOptions::default());
    let keys = 10_000;
    let started = Instant::now();
    for _ in 0..keys {
        editor.handle_key(key('a'));
    }
    let per_key = started.elapsed() / keys;
    println!("{per_key:?} per key (insert through test-insert)");
    assert_eq!(editor.plugins()[0].slow_calls, 0);
}

#[test]
fn core_menu_manages_each_plugin() {
    let mut editor = editor_with("test-insert", PluginOptions::default());
    editor.resize(100, 6);

    editor.handle_key(KeyEvent::ctrl('g'));
    let rows = screen(&editor);
    assert!(rows[4].starts_with(" 1  test-insert"), "{rows:#?}");
    assert!(rows[4].contains("running"), "{rows:#?}");
    assert!(rows[5].contains("[1-9] choose a plugin"), "{rows:#?}");

    // Choose it and disable it: keys no longer reach it.
    editor.handle_key(key('1'));
    assert_eq!(editor.menu(), Some(Menu::Plugin(0)));
    assert!(screen(&editor)[5].contains("[d] disable  [l] reload from disk"));
    editor.handle_key(key('d'));
    assert_eq!(editor.message(), Some("test-insert disabled"));
    editor.handle_key(key('x'));
    assert_eq!(editor.buffer().text().to_string(), "");

    // Enable it again.
    editor.handle_key(KeyEvent::ctrl('g'));
    editor.handle_key(key('1'));
    editor.handle_key(key('d'));
    assert_eq!(editor.message(), Some("test-insert enabled"));
    editor.handle_key(key('x'));
    assert_eq!(editor.buffer().text().to_string(), "x");

    // Reload it from disk; its layer comes back with it.
    editor.handle_key(KeyEvent::ctrl('g'));
    editor.handle_key(key('1'));
    editor.handle_key(key('l'));
    assert_eq!(editor.message(), Some("test-insert reloaded"));
    editor.handle_key(key('y'));
    assert_eq!(editor.buffer().text().to_string(), "xy");
}

/// A config with one `plugins/<name>.toml`.
fn with_plugin(name: &str, text: &str) -> Config {
    let mut config = Config::default();
    config
        .plugins
        .insert(name.into(), Config::parse_plugin(name, text).unwrap());
    config
}

#[test]
fn a_plugins_own_limit_wins_over_the_default() {
    let mut config = with_plugin("test-misbehave", "timeout-ms = 50");
    config.core = Config::parse("[core]\nplugin-timeout-ms = 5000")
        .unwrap()
        .core;
    let mut editor = Editor::default();
    editor.apply_config(config);
    editor.load_plugin(&plugin_dir("test-misbehave")).unwrap();
    assert_eq!(editor.plugins()[0].timeout, Some(Duration::from_millis(50)));
    let started = Instant::now();
    editor.handle_key(key('l'));
    assert!(started.elapsed() < Duration::from_millis(500));
}

#[test]
fn ctrl_g_stops_a_call_without_a_time_limit() {
    let mut editor = Editor::default();
    editor.apply_config(with_plugin("test-misbehave", "timeout-ms = \"none\""));
    editor.load_plugin(&plugin_dir("test-misbehave")).unwrap();
    assert_eq!(editor.plugins()[0].timeout, None);
    // Pressed while the plugin loops, as the terminal's input thread does.
    let interrupter = editor.interrupter();
    let pressing = thread::spawn(move || {
        thread::sleep(Duration::from_millis(300));
        interrupter.interrupt();
    });
    let started = Instant::now();
    editor.handle_key(key('l'));
    pressing.join().unwrap();
    assert!(started.elapsed() >= Duration::from_millis(300));
    let message = editor.message().unwrap();
    assert!(message.contains("stopped with Ctrl-g"), "{message}");
    assert!(editor.plugins()[0].enabled, "restarted");
}

#[test]
fn interrupts_only_stop_calls_already_running() {
    let mut editor = editor_with("test-insert", PluginOptions::default());
    editor.interrupter().interrupt();
    editor.handle_key(key('a'));
    assert_eq!(editor.buffer().text().to_string(), "a");
    assert_eq!(editor.message(), None);
}

#[test]
fn plugin_limits_come_from_config() {
    let mut editor = Editor::default();
    editor.apply_config(Config::parse("[core]\nplugin-timeout-ms = 50").unwrap());
    editor.load_plugin(&plugin_dir("test-misbehave")).unwrap();
    let started = Instant::now();
    editor.handle_key(key('l'));
    assert!(started.elapsed() < Duration::from_millis(500));
    assert!(editor.message().unwrap().contains("took too long"));
}

/// A plugin's data directory is its `/data`, and what it writes there is
/// still there after a restart.
#[test]
fn data_outlives_restarts() {
    let data = env::temp_dir().join(format!("nib-{}-data", std::process::id()));
    let mut editor = Editor::default();
    editor.set_plugin_data_dir(Some(data.clone()));
    editor.load_plugin(&plugin_dir("test-events")).unwrap();
    editor.call_command("test-events.save", "kept").unwrap();
    assert_eq!(
        fs::read_to_string(data.join("test-events/note")).unwrap(),
        "kept"
    );

    editor.handle_key(KeyEvent::ctrl('g'));
    editor.handle_key(key('1'));
    editor.handle_key(key('l'));
    assert_eq!(editor.message(), Some("test-events reloaded"));
    assert_eq!(editor.call_command("test-events.load", "").unwrap(), "kept");
    fs::remove_dir_all(&data).unwrap();

    // Without a place for it, there is no /data.
    let mut editor = Editor::default();
    editor.load_plugin(&plugin_dir("test-events")).unwrap();
    assert!(editor.call_command("test-events.save", "x").is_err());
}
