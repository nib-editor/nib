//! Commands between plugins, events, and timers, with the test plugins in
//! `plugins/test`. Build them first with `cargo xtask build-plugins`.

use std::time::Instant;
use std::{env, fs};

use nib_core::{Config, Editor, Grid, KeyCode, KeyEvent};

mod common;
use common::{key, plugin_dir, screen, type_keys};

fn editor_with(plugins: &[&str]) -> Editor {
    let mut editor = Editor::default();
    for name in plugins {
        editor.load_plugin(&plugin_dir(name)).unwrap();
    }
    editor.resize(40, 6);
    editor
}

/// What test-events got since the last look.
fn log(editor: &mut Editor) -> Vec<String> {
    let log = editor.call_command("test-events.log", "").unwrap();
    log.lines().map(String::from).collect()
}

#[test]
fn plugins_call_each_other() {
    let mut editor = editor_with(&["test-events", "test-misbehave"]);
    assert_eq!(
        editor.call_command("test-events.echo", "hi"),
        Ok("hi".into())
    );
    // Through a plugin, to a core command and to another plugin.
    assert_eq!(
        editor.call_command("test-events.call", "buffer.next"),
        Ok("ok:null".into())
    );
    let called_back = |name: &str| {
        Ok(format!(
            "err:{name}: test-events is in a call already and cannot be called back"
        ))
    };
    assert_eq!(
        editor.call_command("test-events.call", "test-events.log"),
        called_back("test-events.log")
    );
    // test-misbehave calls test-events back while test-events waits for it.
    assert_eq!(
        editor.call_command("test-events.call", "test-misbehave.call-back"),
        called_back("test-events.echo")
    );
    assert_eq!(
        editor.call_command("nothing.here", ""),
        Err("no command named nothing.here".into())
    );
}

#[test]
fn a_plugin_failing_in_a_nested_call_is_restarted_after_it() {
    let mut editor = editor_with(&["test-events", "test-misbehave"]);
    let result = editor
        .call_command("test-events.call", "test-misbehave.panic")
        .unwrap();
    assert_eq!(
        result,
        "err:test-misbehave.panic: test-misbehave failed: panicked: asked to panic"
    );
    let misbehave = editor
        .plugins()
        .into_iter()
        .find(|p| p.name == "test-misbehave")
        .unwrap();
    assert!(misbehave.enabled);
    assert!(editor.message().unwrap().contains("restarted"));
    // Restarted, so its commands are registered again.
    assert_eq!(
        editor.call_command("test-events.call", "test-misbehave.call-back"),
        Ok(
            "err:test-events.echo: test-events is in a call already and cannot be called back"
                .into()
        )
    );
}

#[test]
fn events_arrive_in_order() {
    let path = env::temp_dir().join(format!("nib-{}-events.txt", std::process::id()));
    fs::write(&path, "ab\ncd\n").unwrap();
    let mut editor = Editor::default();
    // Opened before the plugin is loaded, as at startup.
    editor.open(&path).unwrap();
    editor.load_plugin(&plugin_dir("test-events")).unwrap();
    editor.catch_up();
    let file = path.file_name().unwrap().to_str().unwrap();
    assert_eq!(log(&mut editor), [format!("opened {file}")]);

    editor.call_command("test-events.edit", "x").unwrap();
    editor.call_command("buffer.save", "").unwrap();
    editor.call_command("test-events.emit", "ping").unwrap();
    assert_eq!(
        log(&mut editor),
        [
            "changed v1 0-0@0:0-0:0=x".to_string(),
            format!("saved {file}"),
            "custom test-events.ping ".to_string(),
        ]
    );
    // Other plugins' events only arrive when listed in the manifest.
    editor.call_command("test-events.emit", "unheard").unwrap();
    assert!(log(&mut editor).is_empty());
    fs::remove_file(&path).unwrap();
}

#[test]
fn endless_events_are_cut_off() {
    let mut editor = editor_with(&["test-events"]);
    editor.call_command("test-events.emit", "loop").unwrap();
    assert!(editor.message().unwrap().starts_with("dropped 1 events"));
    assert_eq!(log(&mut editor).len(), 1000);
}

#[test]
fn timers_fire_once_and_can_be_cancelled() {
    let mut editor = editor_with(&["test-events"]);
    assert_eq!(editor.next_timer(), None);
    let id = editor.call_command("test-events.timer", "0").unwrap();
    let later = editor.call_command("test-events.timer", "60000").unwrap();
    assert!(editor.next_timer().unwrap() <= Instant::now());
    editor.run_timers();
    assert_eq!(log(&mut editor), [format!("timer {id}")]);
    editor.call_command("test-events.cancel", &later).unwrap();
    assert_eq!(editor.next_timer(), None);
}

#[test]
fn the_keymap_tells_others_about_modes() {
    let mut editor = editor_with(&["helix", "test-events"]);
    // Emitted by helix's init, delivered once both are loaded.
    editor.deliver_events();
    assert_eq!(log(&mut editor), ["custom helix.mode_changed \"normal\""]);
    type_keys(&mut editor, "i<esc>");
    assert_eq!(
        log(&mut editor),
        [
            "custom helix.mode_changed \"insert\"",
            "custom helix.mode_changed \"normal\"",
        ]
    );
}

/// An editor where `lazy` starts only when needed.
fn with_lazy(lazy: &str, others: &[&str]) -> Editor {
    let mut config = Config::default();
    config.plugins.insert(
        lazy.into(),
        Config::parse_plugin(lazy, "load = \"lazy\"").unwrap(),
    );
    let mut editor = Editor::default();
    editor.apply_config(config);
    for name in [lazy].iter().chain(others) {
        editor.load_plugin(&plugin_dir(name)).unwrap();
    }
    editor
}

#[test]
fn lazy_plugins_start_when_their_commands_are_called() {
    let mut editor = with_lazy("test-events", &[]);
    assert!(editor.plugins()[0].waiting);
    assert!(
        !editor
            .commands()
            .iter()
            .any(|(name, _)| name == "test-events.echo")
    );
    assert_eq!(editor.call_command("test-events.echo", "x"), Ok("x".into()));
    assert!(!editor.plugins()[0].waiting);
}

#[test]
fn lazy_plugins_start_for_calls_from_other_plugins() {
    let mut editor = with_lazy("test-events", &["test-misbehave"]);
    assert_eq!(
        editor.call_command("test-misbehave.call-back", ""),
        Ok("back".into())
    );
}

#[test]
fn lazy_plugins_start_for_their_events() {
    let mut editor = with_lazy("test-events", &[]);
    let path = env::temp_dir().join(format!("nib-{}-lazy.txt", std::process::id()));
    fs::write(&path, "x").unwrap();
    editor.open(&path).unwrap();
    editor.deliver_events();
    assert!(!editor.plugins()[0].waiting);
    let file = path.file_name().unwrap().to_str().unwrap();
    assert_eq!(log(&mut editor), [format!("opened {file}")]);
    fs::remove_file(&path).unwrap();
}

#[test]
fn plugins_list_commands_and_buffers() {
    let mut editor = editor_with(&["test-events"]);
    assert_eq!(
        editor.call_command("test-events.buffers", ""),
        Ok("1".into())
    );
    let names: Vec<String> = editor
        .commands()
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert!(names.contains(&"buffer.save".to_string()));
    assert!(names.contains(&"test-events.echo".to_string()));
}

/// A closed buffer leaves plugins' lists.
#[test]
fn closed_buffers_are_gone_for_plugins() {
    let dir = env::temp_dir();
    let a = dir.join(format!("nib-{}-closed-a.txt", std::process::id()));
    let b = dir.join(format!("nib-{}-closed-b.txt", std::process::id()));
    fs::write(&a, "a").unwrap();
    fs::write(&b, "b").unwrap();
    let mut editor = Editor::default();
    editor.load_plugin(&plugin_dir("test-events")).unwrap();
    editor.open(&a).unwrap();
    editor.open(&b).unwrap();
    assert_eq!(editor.call_command("test-events.buffers", "").unwrap(), "2");
    editor.call_command("buffer.close", "").unwrap();
    assert_eq!(editor.call_command("test-events.buffers", "").unwrap(), "1");
    assert!(editor.plugins()[0].last_error.is_none());
    fs::remove_file(&a).unwrap();
    fs::remove_file(&b).unwrap();
}

#[test]
fn prompts_take_keys_the_core_way_without_a_base() {
    let mut editor = Editor::default();
    editor.resize(40, 6);
    editor.load_plugin(&plugin_dir("test-events")).unwrap();
    let id = editor.call_command("test-events.prompt", "ask> ").unwrap();
    for c in "hé".chars() {
        editor.handle_key(key(c));
    }
    editor.handle_key(KeyEvent::new(KeyCode::Left));
    editor.handle_key(KeyEvent::new(KeyCode::Down));
    editor.handle_key(KeyEvent::new(KeyCode::Enter));
    assert_eq!(
        log(&mut editor),
        [
            format!("prompt {id} \"h\" 1"),
            format!("prompt {id} \"hé\" 3"),
            format!("prompt {id} \"hé\" 1"),
            format!("prompt {id} Action::Next"),
            format!("prompt {id} Action::Accept"),
        ]
    );
    // Drawn above the status line, with the cursor in it.
    assert_eq!(screen(&editor)[4].trim_end(), "ask> hé");
    let cursor = editor.render(&mut Grid::default()).unwrap();
    assert_eq!((cursor.x, cursor.y), (6, 4));

    // Closed, keys go down the input stack again, and the text has room.
    editor.call_command("test-events.close-prompt", "").unwrap();
    editor.handle_key(key('x'));
    assert!(log(&mut editor).is_empty());
    assert_eq!(screen(&editor)[4].trim_end(), "");
}

#[test]
fn the_base_edits_prompts_its_own_way() {
    let mut editor = Editor::default();
    editor.load_plugin(&plugin_dir("helix")).unwrap();
    editor.load_plugin(&plugin_dir("test-events")).unwrap();
    let id = editor.call_command("test-events.prompt", "ask> ").unwrap();
    type_keys(&mut editor, "one two");
    editor.handle_key(KeyEvent::ctrl('w'));
    editor.handle_key(KeyEvent::ctrl('p'));
    editor.handle_key(KeyEvent::new(KeyCode::Escape));
    let log = log(&mut editor);
    assert_eq!(
        log[log.len() - 3..],
        [
            format!("prompt {id} \"one \" 4"),
            format!("prompt {id} Action::Previous"),
            format!("prompt {id} Action::Cancel"),
        ]
    );
    // The keys never reached helix's own: nothing was typed into the text.
    assert_eq!(editor.buffer().text().to_string(), "");
}
