//! The file picker in `plugins/picker`, opened from the Helix keymap. Build
//! them first with `cargo xtask build-plugins`. It lists the files of this
//! crate with git, as tests run in the crate's directory.

use std::thread;
use std::time::{Duration, Instant};

use nib_core::{Editor, KeyCode, KeyEvent};

mod common;
use common::{plugin_dir, screen, type_keys};

fn editor() -> Editor {
    let mut editor = Editor::default();
    editor.load_plugin(&plugin_dir("helix")).unwrap();
    editor.load_plugin(&plugin_dir("picker")).unwrap();
    editor.resize(80, 20);
    editor
}

/// Opens the picker and waits for its list: the count after the query
/// ends with "…" while files are still coming.
fn open_picker(editor: &mut Editor) {
    type_keys(editor, " f");
    let deadline = Instant::now() + Duration::from_secs(20);
    let listing = |editor: &Editor| {
        screen(editor).iter().any(|row| {
            row.contains("files> ")
                && row
                    .trim_end()
                    .trim_end_matches('│')
                    .trim_end()
                    .ends_with('…')
        })
    };
    while listing(editor) {
        assert!(Instant::now() < deadline, "no file list within 20 seconds");
        editor.run_background();
        thread::sleep(Duration::from_millis(10));
    }
}

fn shows(editor: &Editor, text: &str) -> bool {
    screen(editor).iter().any(|row| row.contains(text))
}

#[test]
fn picks_a_file_by_fuzzy_name() {
    let mut editor = editor();
    open_picker(&mut editor);
    assert!(shows(&editor, "files> "));
    type_keys(&mut editor, "cargotoml");
    // Under the query, past the line between, the best match first.
    let rows = screen(&editor);
    let prompt = rows.iter().position(|row| row.contains("files> ")).unwrap();
    assert!(
        rows[prompt + 2].trim_start().starts_with("│ Cargo.toml "),
        "{rows:#?}"
    );
    type_keys(&mut editor, "<ret>");
    let path = editor.buffer().path().unwrap().to_path_buf();
    assert!(path.ends_with("Cargo.toml"), "{path:?}");
    assert!(!shows(&editor, "files> "));
}

#[test]
fn escape_closes_it_and_gives_the_keys_back() {
    let mut editor = editor();
    open_picker(&mut editor);
    editor.handle_key(KeyEvent::new(KeyCode::Down));
    type_keys(&mut editor, "<esc>");
    assert!(!shows(&editor, "files> "));
    // The keymap has the keys again.
    type_keys(&mut editor, "ihello<esc>");
    assert_eq!(editor.buffer().text().to_string(), "hello");
}
