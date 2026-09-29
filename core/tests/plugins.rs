//! Runs the test plugins in `plugins/test`. Build them first with
//! `cargo xtask build-plugins`.

use std::time::{Duration, Instant};
use std::{env, fs, thread};

use nib_core::{
    API_VERSION, Config, Editor, KeyCode, KeyEvent, Menu, PluginOptions, PluginSource, Range,
};

mod common;
use common::{key, menu, plugin_dir, screen};

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
    menu(&mut editor, &["quit"]);
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
    menu(&mut editor, &["restart all"]);
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
/// `cargo test --release -p nib-editor-core --test plugins -- --ignored --nocapture`.
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
    editor.resize(100, 20);

    editor.handle_key(KeyEvent::ctrl('g'));
    let shown = screen(&editor).join("\n");
    assert!(shown.contains("│ test-insert  0.0.0  running"), "{shown}");
    assert!(shown.contains("│ Restart all plugins"), "{shown}");

    // Choose it and disable it: keys no longer reach it.
    editor.handle_key(KeyEvent::new(KeyCode::Enter));
    assert_eq!(editor.menu(), Some(Menu::Plugin(0)));
    let shown = screen(&editor).join("\n");
    for row in ["│ Disable", "│ Reload from disk", "│ Open its settings"] {
        assert!(shown.contains(row), "{row} in {shown}");
    }
    for c in "disable".chars() {
        editor.handle_key(key(c));
    }
    editor.handle_key(KeyEvent::new(KeyCode::Enter));
    assert_eq!(editor.message(), Some("test-insert disabled"));
    editor.handle_key(key('x'));
    assert_eq!(editor.buffer().text().to_string(), "");

    // Enable it again.
    menu(&mut editor, &["test-insert", "enable"]);
    assert_eq!(editor.message(), Some("test-insert enabled"));
    editor.handle_key(key('x'));
    assert_eq!(editor.buffer().text().to_string(), "x");

    // Reload it from disk; its layer comes back with it.
    menu(&mut editor, &["test-insert", "reload"]);
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

    menu(&mut editor, &["test-events", "reload"]);
    assert_eq!(editor.message(), Some("test-events reloaded"));
    assert_eq!(editor.call_command("test-events.load", "").unwrap(), "kept");
    // The plugin holds its directory open, which Windows will not remove.
    drop(editor);
    fs::remove_dir_all(&data).unwrap();

    // Without a place for it, there is no /data.
    let mut editor = Editor::default();
    editor.load_plugin(&plugin_dir("test-events")).unwrap();
    assert!(editor.call_command("test-events.save", "x").is_err());
}

/// Updates as a frontend would give them: the "newer release" rewrites the
/// version in the plugin's manifest, in a copy of the plugin.
struct FakeUpdates {
    dir: std::path::PathBuf,
    /// The version and added capabilities of the next check; `None` when
    /// up to date.
    next: std::sync::Mutex<Option<(String, Vec<String>)>>,
}

struct FakePending {
    dir: std::path::PathBuf,
    version: String,
    added: Vec<String>,
}

impl nib_core::PluginStore for FakeUpdates {
    fn can_update(&self, name: &str) -> bool {
        name == "test-insert"
    }

    fn can_remove(&self, name: &str) -> bool {
        name == "test-insert"
    }

    fn remove(&self, _name: &str) -> Result<(), String> {
        Ok(())
    }

    /// Installs a copy of test-insert, whatever the source.
    fn prepare_install(&self, source: &str) -> Result<Box<dyn nib_core::PendingInstall>, String> {
        if source == "nothing/here" {
            return Err("no such release".into());
        }
        Ok(Box::new(FakeInstall {
            dir: self.dir.join("installed"),
            source: source.to_string(),
        }))
    }

    fn check(&self, _name: &str) -> Result<Option<Box<dyn nib_core::PendingUpdate>>, String> {
        let next = self.next.lock().unwrap().clone();
        Ok(next.map(|(version, added)| {
            Box::new(FakePending {
                dir: self.dir.clone(),
                version,
                added,
            }) as Box<dyn nib_core::PendingUpdate>
        }))
    }
}

struct FakeInstall {
    dir: std::path::PathBuf,
    source: String,
}

impl nib_core::PendingInstall for FakeInstall {
    fn name(&self) -> &str {
        "test-insert"
    }

    fn version(&self) -> &str {
        "0.0.0"
    }

    fn source(&self) -> &str {
        &self.source
    }

    fn capabilities(&self) -> &[String] {
        &[]
    }

    fn apply(self: Box<Self>) -> Result<std::path::PathBuf, String> {
        fs::create_dir_all(&self.dir).unwrap();
        for file in ["plugin.toml", "plugin.wasm"] {
            fs::copy(plugin_dir("test-insert").join(file), self.dir.join(file)).unwrap();
        }
        Ok(self.dir)
    }
}

impl nib_core::PendingUpdate for FakePending {
    fn version(&self) -> &str {
        &self.version
    }

    fn added_capabilities(&self) -> &[String] {
        &self.added
    }

    fn apply(self: Box<Self>) -> Result<(), String> {
        let manifest = self.dir.join("plugin.toml");
        let text = fs::read_to_string(&manifest).unwrap();
        let text = text.replace(
            "version = \"0.0.0\"",
            &format!("version = \"{}\"", self.version),
        );
        fs::write(manifest, text).map_err(|err| err.to_string())
    }
}

#[test]
fn plugins_update_from_the_core_menu() {
    let dir = env::temp_dir().join(format!("nib-{}-updating", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    for file in ["plugin.toml", "plugin.wasm"] {
        fs::copy(plugin_dir("test-insert").join(file), dir.join(file)).unwrap();
    }
    let updates = std::sync::Arc::new(FakeUpdates {
        dir: dir.clone(),
        next: std::sync::Mutex::new(None),
    });
    let mut editor = Editor::default();
    editor.resize(100, 20);
    editor.set_plugin_store(Some(updates.clone()));
    editor.load_plugin(&dir).unwrap();
    let update = |editor: &mut Editor| {
        menu(editor, &["test-insert"]);
        let shown = screen(editor).join("\n");
        assert!(shown.contains("│ Update"), "{shown}");
        for c in "update".chars() {
            editor.handle_key(key(c));
        }
        editor.handle_key(KeyEvent::new(KeyCode::Enter));
        // Checked on a thread; the answer comes back through the inbox.
        let deadline = Instant::now() + Duration::from_secs(10);
        while editor.message().is_some_and(|m| m.starts_with("checking")) {
            assert_eq!(editor.menu(), None, "the menu closes while it checks");
            assert!(Instant::now() < deadline, "no answer from the check");
            editor.run_background();
            thread::sleep(Duration::from_millis(5));
        }
    };

    update(&mut editor);
    assert_eq!(editor.message(), Some("test-insert is up to date"));

    *updates.next.lock().unwrap() = Some(("0.1.0".into(), Vec::new()));
    update(&mut editor);
    assert_eq!(editor.message(), Some("test-insert updated to 0.1.0"));
    assert_eq!(editor.plugins()[0].version, "0.1.0");

    // More capabilities are asked for first; no keeps the one there.
    fs::write(
        dir.join("plugin.toml"),
        fs::read_to_string(dir.join("plugin.toml"))
            .unwrap()
            .replace("0.1.0", "0.0.0"),
    )
    .unwrap();
    *updates.next.lock().unwrap() = Some(("0.2.0".into(), vec!["process".into()]));
    update(&mut editor);
    assert_eq!(editor.menu(), Some(Menu::ConfirmUpdate(0)));
    let shown = screen(&editor).join("\n");
    assert!(shown.contains("0.2.0 also wants: process"), "{shown}");
    editor.handle_key(key('n'));
    assert_eq!(editor.message(), Some("test-insert left as it was"));
    update(&mut editor);
    editor.handle_key(key('y'));
    assert_eq!(editor.message(), Some("test-insert updated to 0.2.0"));
    assert_eq!(editor.plugins()[0].version, "0.2.0");

    // Without the frontend's updates, there is no Update.
    editor.set_plugin_store(None);
    menu(&mut editor, &["test-insert"]);
    assert!(!screen(&editor).join("\n").contains("│ Update"));
    fs::remove_dir_all(&dir).unwrap();
}

/// Arrows, Tab, and Ctrl-n and Ctrl-p move in the core menu, and Esc goes
/// back to where it was.
#[test]
fn keys_move_in_the_core_menu() {
    let mut editor = Editor::default();
    editor.load_plugin(&plugin_dir("test-insert")).unwrap();
    editor.load_plugin(&plugin_dir("test-events")).unwrap();
    editor.resize(100, 20);
    let press = |editor: &mut Editor, codes: &[KeyCode]| {
        for &code in codes {
            editor.handle_key(KeyEvent::new(code));
        }
    };
    editor.handle_key(KeyEvent::ctrl('g'));
    press(&mut editor, &[KeyCode::Down, KeyCode::Enter]);
    assert_eq!(editor.menu(), Some(Menu::Plugin(1)));
    press(&mut editor, &[KeyCode::Escape, KeyCode::Enter]);
    assert_eq!(editor.menu(), Some(Menu::Plugin(1)), "back where it was");
    press(&mut editor, &[KeyCode::Escape, KeyCode::Escape]);
    assert_eq!(editor.menu(), None);

    // Above the first one, it stays; opened again, it starts at the top.
    editor.handle_key(KeyEvent::ctrl('g'));
    press(&mut editor, &[KeyCode::Up, KeyCode::Enter]);
    assert_eq!(editor.menu(), Some(Menu::Plugin(0)));
    press(&mut editor, &[KeyCode::Escape, KeyCode::Tab]);
    editor.handle_key(KeyEvent::ctrl('n'));
    editor.handle_key(KeyEvent::ctrl('p'));
    editor.handle_key(KeyEvent {
        code: KeyCode::Tab,
        modifiers: nib_core::Modifiers {
            shift: true,
            ..Default::default()
        },
    });
    press(&mut editor, &[KeyCode::Enter]);
    assert_eq!(editor.menu(), Some(Menu::Plugin(0)));
}

/// Waits for what the core menu started on a thread.
fn wait_for_menu(editor: &mut Editor, busy: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while editor.message().is_some_and(|m| m.starts_with(busy)) {
        assert!(Instant::now() < deadline, "nothing came back");
        editor.run_background();
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn plugins_install_and_remove_from_the_core_menu() {
    let dir = env::temp_dir().join(format!("nib-{}-installing", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let store = std::sync::Arc::new(FakeUpdates {
        dir: dir.clone(),
        next: std::sync::Mutex::new(None),
    });
    let mut editor = Editor::default();
    editor.resize(120, 20);
    editor.set_plugin_store(Some(store));
    let type_in = |editor: &mut Editor, text: &str| {
        for c in text.chars() {
            editor.handle_key(key(c));
        }
    };

    // A source that cannot be fetched says why.
    menu(&mut editor, &["add"]);
    assert_eq!(editor.menu(), Some(Menu::AddPlugin));
    type_in(&mut editor, "nothing/her");
    editor.handle_key(KeyEvent::new(KeyCode::Char('x')));
    editor.handle_key(KeyEvent::new(KeyCode::Backspace));
    type_in(&mut editor, "e");
    let shown = screen(&editor).join("\n");
    assert!(shown.contains("add: nothing/here "), "{shown}");
    editor.handle_key(KeyEvent::new(KeyCode::Enter));
    wait_for_menu(&mut editor, "fetching");
    assert_eq!(editor.message(), Some("nothing/here: no such release"));

    // One that can is asked about, then loaded at once.
    menu(&mut editor, &["add"]);
    type_in(&mut editor, "someone/insert");
    editor.handle_key(KeyEvent::new(KeyCode::Enter));
    wait_for_menu(&mut editor, "fetching");
    assert_eq!(editor.menu(), Some(Menu::ConfirmInstall));
    let shown = screen(&editor).join("\n");
    assert!(
        shown.contains("install test-insert 0.0.0 from someone/insert? no capabilities"),
        "{shown}"
    );
    editor.handle_key(key('y'));
    assert_eq!(editor.message(), Some("test-insert 0.0.0 installed"));
    editor.handle_key(key('z'));
    assert_eq!(editor.buffer().text().to_string(), "z");

    // Removed, it stops taking keys.
    menu(&mut editor, &["test-insert", "remove"]);
    assert_eq!(editor.menu(), Some(Menu::ConfirmRemove(0)));
    assert!(screen(&editor).join("\n").contains("Remove test-insert?"));
    editor.handle_key(key('y'));
    assert_eq!(
        editor.message(),
        Some("test-insert removed; its settings and data are kept")
    );
    editor.handle_key(key('w'));
    assert_eq!(editor.buffer().text().to_string(), "z");
    drop(editor);
    fs::remove_dir_all(&dir).unwrap();
}

/// Settings open from inside nib, and saving them reads them again.
#[test]
fn settings_open_and_reload_when_saved() {
    let dir = env::temp_dir().join(format!("nib-{}-settings", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let mut editor = Editor::default();
    editor.set_config_dir(Some(dir.clone()));
    editor.load_plugin(&plugin_dir("helix")).unwrap();
    editor.load_plugin(&plugin_dir("indent")).unwrap();
    // Replaces the text with the helix keymap, then saves.
    let write = |editor: &mut Editor, text: &str| {
        let keys = text.replace('\n', "<ret>");
        common::type_keys(editor, &format!("%di{keys}<esc>"));
        editor.call_command("buffer.save", "{}").unwrap();
    };

    // Missing, config.toml opens with the defaults commented out, unsaved.
    editor.call_command("config.open", "{}").unwrap();
    assert_eq!(
        editor.buffer().path(),
        Some(dir.join("config.toml").as_path())
    );
    assert_eq!(
        editor.buffer().text().to_string(),
        nib_core::CONFIG_TEMPLATE
    );
    assert!(editor.buffer().is_modified());
    assert!(!dir.join("config.toml").exists());

    // Saved, it takes effect at once.
    write(&mut editor, "[core]\ntab-width = 3\n");
    assert_eq!(editor.message(), Some("settings reloaded"));
    assert_eq!(editor.settings().tab_width, 3);

    // A plugin's [settings] restart it; the rest waits for the next start.
    editor
        .call_command("config.open", r#"{"plugin": "indent"}"#)
        .unwrap();
    assert!(
        editor
            .buffer()
            .text()
            .to_string()
            .contains("How nib runs the indent plugin")
    );
    write(
        &mut editor,
        "timeout-ms = 2000\n[settings.languages.yaml]\nindent = 3\n",
    );
    assert_eq!(
        editor.message(),
        Some(
            "settings reloaded; indent restarted; the rest of indent's settings take effect when nib starts again"
        )
    );

    // Broken, the settings in use stay.
    editor.call_command("config.open", "{}").unwrap();
    write(&mut editor, "[core]\ntab-width = \"wide\"\n");
    assert!(
        editor
            .message()
            .unwrap()
            .ends_with("the settings in use are kept"),
        "{:?}",
        editor.message()
    );
    assert_eq!(editor.settings().tab_width, 3);
    drop(editor);
    fs::remove_dir_all(&dir).unwrap();
}

/// helix, copied as another base named `other` whose menu key is F10.
fn other_base(dir: &std::path::Path) -> std::path::PathBuf {
    let other = dir.join("other");
    fs::create_dir_all(&other).unwrap();
    fs::copy(
        plugin_dir("helix").join("plugin.wasm"),
        other.join("plugin.wasm"),
    )
    .unwrap();
    let manifest = format!(
        "name = \"other\"\nversion = \"0.1.0\"\napi = \"{API_VERSION}\"\nbase = true\n\
         menu-key = \"F10\"\nevents = [\"syntax-updated\"]\n"
    );
    fs::write(other.join("plugin.toml"), manifest).unwrap();
    other
}

fn running(editor: &Editor) -> Vec<String> {
    editor
        .plugins()
        .into_iter()
        .filter(|p| p.enabled)
        .map(|p| p.name)
        .collect()
}

#[test]
fn only_the_chosen_base_runs() {
    let dir = env::temp_dir().join(format!("nib-{}-bases", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let other = other_base(&dir);
    let helix = plugin_dir("helix");
    let load = |base: &str, menu_key: Option<KeyEvent>| {
        let mut editor = Editor::default();
        let mut config = Config::default();
        config.core.base = base.into();
        config.core.menu_key = menu_key;
        editor.apply_config(config);
        let results = editor.load_plugins(&[PluginSource::Dir(&helix), PluginSource::Dir(&other)]);
        let errors: Vec<String> = results
            .into_iter()
            .filter_map(Result::err)
            .map(|err| err.to_string())
            .collect();
        (editor, errors)
    };

    // The other base waits, stopped.
    let (editor, errors) = load("helix", None);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(running(&editor), ["helix"]);
    assert_eq!(editor.base_in_use(), Some("helix"));
    assert_eq!(editor.menu_key(), KeyEvent::ctrl('g'));

    // The chosen one brings its menu key, unless the user set one.
    let (editor, _) = load("other", None);
    assert_eq!(running(&editor), ["other"]);
    assert_eq!(editor.menu_key(), KeyEvent::new(KeyCode::F(10)));
    let (editor, _) = load("other", Some(KeyEvent::ctrl(']')));
    assert_eq!(editor.menu_key(), KeyEvent::ctrl(']'));

    // Without the chosen one, helix, as no key would edit otherwise.
    let (mut editor, errors) = load("vim", None);
    assert_eq!(errors, ["vim is not available as the base; using helix"]);
    assert_eq!(running(&editor), ["helix"]);

    // The menu switches, and restarting everything keeps one base.
    menu(&mut editor, &["other", "use as"]);
    assert_eq!(running(&editor), ["other"]);
    assert!(
        editor
            .message()
            .unwrap()
            .starts_with("other is the base now")
    );
    assert_eq!(editor.menu_key(), KeyEvent::new(KeyCode::F(10)));
    menu(&mut editor, &["restart all"]);
    assert_eq!(running(&editor), ["other"]);

    // So does the core.menu command, and the settings when read again.
    editor.call_command("core.menu", "{}").unwrap();
    assert_eq!(editor.menu(), Some(Menu::Main));
    editor.handle_key(KeyEvent::new(KeyCode::Escape));
    editor.set_config_dir(Some(dir.clone()));
    fs::write(dir.join("config.toml"), "[core]\nbase = \"helix\"\n").unwrap();
    editor.call_command("config.reload", "{}").unwrap();
    assert_eq!(
        editor.message(),
        Some("settings reloaded; helix is the base now")
    );
    assert_eq!(running(&editor), ["helix"]);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_first_start_asks_for_a_base_and_keeps_the_answer() {
    let dir = env::temp_dir().join(format!("nib-{}-first-start", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let start = |keys: &[KeyEvent]| {
        let mut editor = Editor::default();
        editor.set_config_dir(Some(dir.clone()));
        let (helix, nano) = (plugin_dir("helix"), plugin_dir("nano"));
        for loaded in editor.load_plugins(&[PluginSource::Dir(&helix), PluginSource::Dir(&nano)]) {
            loaded.unwrap();
        }
        editor.ask_for_base();
        assert_eq!(editor.menu(), Some(Menu::ChooseBase));
        for &key in keys {
            editor.handle_key(key);
        }
        let config = Config::load(&dir).unwrap();
        (
            running(&editor),
            config.core.base,
            editor.message().map(String::from),
        )
    };

    let enter = KeyEvent::new(KeyCode::Enter);
    let (running, base, message) = start(&[key('n'), enter]);
    assert_eq!((running, base.as_str()), (vec!["nano".to_string()], "nano"));
    assert_eq!(
        message.as_deref(),
        Some("nano is the base; base in config.toml keeps it")
    );

    // Esc keeps the one in use, and says so in config.toml too.
    fs::remove_file(dir.join("config.toml")).unwrap();
    let (running, base, _) = start(&[KeyEvent::new(KeyCode::Escape)]);
    assert_eq!(
        (running, base.as_str()),
        (vec!["helix".to_string()], "helix")
    );
    let _ = fs::remove_dir_all(&dir);
}
