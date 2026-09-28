//! Syntax trees with the grammars from `plugins/languages`. Build them
//! first with `cargo xtask build-plugins`.

use std::{env, fs};

use nib_core::{Color, Editor, Grid, KeyCode, KeyEvent, Modifiers, Range, Selection};

mod common;
use common::{plugin_dir, screen, type_keys};

fn fg(editor: &Editor, x: u16, y: u16) -> Color {
    let mut grid = Grid::default();
    editor.render(&mut grid);
    grid.cell(x, y).style.fg
}

const KEYWORD: Color = Color::Indexed(5);
const FUNCTION: Color = Color::Indexed(4);
const COMMENT: Color = Color::Indexed(8);

#[test]
fn highlights_and_follows_edits() {
    let path = env::temp_dir().join(format!("nib-{}-syntax.rs", std::process::id()));
    fs::write(&path, "fn main() {}\nfn other() {}\n").unwrap();
    let mut editor = Editor::default();
    editor.open(&path).unwrap();
    editor.load_plugin(&plugin_dir("helix")).unwrap();
    // The language comes after the file is open, as when nib starts.
    editor.load_plugin(&plugin_dir("rust")).unwrap();
    editor.resize(40, 6);

    // The first parse waits until after the first frame.
    assert_eq!(fg(&editor, 0, 0), Color::Reset);
    assert!(editor.catch_up());
    assert_eq!(fg(&editor, 0, 0), KEYWORD);
    // Then syntax-updated reaches the keymap.
    assert!(editor.catch_up());
    assert!(!editor.catch_up());
    assert_eq!(fg(&editor, 3, 0), FUNCTION);
    assert_eq!(fg(&editor, 3, 1), FUNCTION);

    // Reparsed from the edit: the new comment and the lines after it.
    type_keys(&mut editor, "i// x<ret><esc>");
    assert_eq!(fg(&editor, 0, 0), COMMENT);
    assert_eq!(fg(&editor, 0, 1), KEYWORD);
    assert_eq!(fg(&editor, 3, 2), FUNCTION);

    // Undo reaches the tree as one edit around what it changed.
    type_keys(&mut editor, "u");
    assert_eq!(fg(&editor, 0, 0), KEYWORD);

    editor.resize(80, 6);
    editor.handle_key(KeyEvent::ctrl('g'));
    let rows = screen(&editor);
    assert!(
        rows.iter()
            .any(|row| row.contains("rust") && row.contains("languages")),
        "{rows:#?}"
    );
    fs::remove_file(&path).unwrap();
}

/// Prints where startup time goes, without and with the compile cache. Run
/// with `cargo test --release -p nib-editor-core --test syntax -- --ignored --nocapture`,
/// and `NIB_STARTUP_FILE` set to open another Rust file than `src/render.rs`.
#[test]
#[ignore]
fn startup_breakdown() {
    use std::time::Instant;
    let cache = env::temp_dir().join(format!("nib-{}-cache", std::process::id()));
    let file = env::var("NIB_STARTUP_FILE")
        .unwrap_or_else(|_| env!("CARGO_MANIFEST_DIR").to_string() + "/src/render.rs");
    for round in ["cold cache", "warm cache"] {
        let started = Instant::now();
        let mut editor = Editor::default();
        editor.set_plugin_cache_dir(Some(cache.clone()));
        editor.resize(120, 40);
        let mut last = Instant::now();
        let mut step = |name: &str| {
            println!("{round}: {name}: {:?}", last.elapsed());
            last = Instant::now();
        };
        editor.load_plugin(&plugin_dir("helix")).unwrap();
        step("load helix");
        editor.load_plugin(&plugin_dir("rust")).unwrap();
        editor.load_plugin(&plugin_dir("markdown")).unwrap();
        step("load rust and markdown (registered, not compiled)");
        editor.open(&file).unwrap();
        step("open");
        let mut grid = Grid::default();
        editor.render(&mut grid);
        step("first frame, without highlighting");
        editor.catch_up();
        editor.render(&mut grid);
        step("load rust, parse, and draw highlighted");
        editor.catch_up();
        editor.render(&mut grid);
        step("load markdown, parse doc comments on screen, and draw");
        editor.catch_up();
        step("parse doc comments a screen away");
        println!("{round}: total {:?}", started.elapsed());
    }
    let _ = fs::remove_dir_all(&cache);
}

const SOURCE: &str = "\
// one
// two
fn add(a: u8, b: u8) -> u8 {
    let s = \"}\";
    a + b
}

fn other() {}
";

/// An editor on a Rust file with the keymap and the language, parsed.
fn rust_editor(name: &str) -> (Editor, std::path::PathBuf) {
    rust_editor_with(name, SOURCE)
}

fn rust_editor_with(name: &str, source: &str) -> (Editor, std::path::PathBuf) {
    let path = env::temp_dir().join(format!("nib-{}-{name}.rs", std::process::id()));
    fs::write(&path, source).unwrap();
    let mut editor = Editor::default();
    editor.open(&path).unwrap();
    editor.load_plugin(&plugin_dir("helix")).unwrap();
    editor.load_plugin(&plugin_dir("rust")).unwrap();
    editor.resize(40, 12);
    editor.catch_up();
    (editor, path)
}

/// Puts a block cursor on the first `at` in the text.
fn put_cursor(editor: &mut Editor, at: &str) {
    let pos = SOURCE.find(at).unwrap();
    editor.view_mut().selection =
        Selection::new(vec![Range::new(pos, pos + 1)], 0, editor.buffer().text()).unwrap();
}

fn selected(editor: &Editor) -> String {
    let range = editor.view().selection.primary();
    editor
        .buffer()
        .text()
        .slice(range.from()..range.to())
        .to_string()
}

fn alt(code: KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: Modifiers {
            alt: true,
            ..Modifiers::default()
        },
    }
}

#[test]
fn selects_text_objects() {
    let (mut editor, path) = rust_editor("objects");
    put_cursor(&mut editor, "a + b");
    type_keys(&mut editor, "maf");
    assert!(selected(&editor).starts_with("fn add(") && selected(&editor).ends_with("b\n}"));
    put_cursor(&mut editor, "a + b");
    type_keys(&mut editor, "mif");
    assert!(selected(&editor).starts_with("{\n    let"));

    put_cursor(&mut editor, "a: u8");
    type_keys(&mut editor, "maa");
    assert_eq!(selected(&editor), "a: u8,");
    put_cursor(&mut editor, "b: u8");
    type_keys(&mut editor, "mia");
    assert_eq!(selected(&editor), "b: u8");

    // Consecutive line comments are one comment.
    put_cursor(&mut editor, "one");
    type_keys(&mut editor, "mac");
    assert_eq!(selected(&editor), "// one\n// two");

    // Not in a function: nothing changes.
    put_cursor(&mut editor, "one");
    type_keys(&mut editor, "maf");
    assert_eq!(selected(&editor), "o");
    fs::remove_file(&path).unwrap();
}

#[test]
fn jumps_between_text_objects() {
    let (mut editor, path) = rust_editor("jumps");
    put_cursor(&mut editor, "one");
    type_keys(&mut editor, "]f");
    assert!(selected(&editor).starts_with("fn add("));
    type_keys(&mut editor, "]f");
    assert_eq!(selected(&editor), "fn other() {}");
    type_keys(&mut editor, "[f");
    assert!(selected(&editor).starts_with("fn add("));
    // Backward jumps put the cursor at the start.
    let range = editor.view().selection.primary();
    assert!(range.head < range.anchor);

    put_cursor(&mut editor, "one");
    type_keys(&mut editor, "2]a");
    assert_eq!(selected(&editor), "b: u8");
    fs::remove_file(&path).unwrap();
}

/// The syntax API answers from the language of a code block inside it,
/// so text objects and node moves work there too.
#[test]
fn trees_of_code_blocks_answer_inside_them() {
    let text = "# Notes\n\nSome text.\n\n```rust\nfn add(a: u8) -> u8 {\n    a + 1\n}\n\nfn two() {}\n```\n";
    let (mut editor, path) = editor_with(&["markdown", "rust"], "blocks-tree.md", text);
    let at = |editor: &mut Editor, what: &str| {
        let pos = text.find(what).unwrap();
        editor.view_mut().selection =
            Selection::new(vec![Range::new(pos, pos + 1)], 0, editor.buffer().text()).unwrap();
    };
    at(&mut editor, "a + 1");
    type_keys(&mut editor, "maf");
    assert_eq!(selected(&editor), "fn add(a: u8) -> u8 {\n    a + 1\n}");

    // From the Markdown before the block to the functions in it.
    at(&mut editor, "Some");
    type_keys(&mut editor, "]f");
    assert!(
        selected(&editor).starts_with("fn add("),
        "{:?}",
        selected(&editor)
    );
    type_keys(&mut editor, "]f");
    assert_eq!(selected(&editor), "fn two() {}");

    // Growing the selection goes up Rust's tree, then out into Markdown's.
    at(&mut editor, "a + 1");
    let mut grown = Vec::new();
    for _ in 0..8 {
        editor.handle_key(alt(KeyCode::Char('o')));
        grown.push(selected(&editor));
    }
    assert!(grown.contains(&"a + 1".to_string()), "{grown:#?}");
    assert!(
        grown
            .iter()
            .any(|s| s.starts_with("fn add(") && s.ends_with('}')),
        "{grown:#?}"
    );
    assert!(grown.iter().any(|s| s.starts_with("```rust")), "{grown:#?}");
    fs::remove_file(&path).unwrap();
}

/// In Rust, a code block of a doc comment is Rust of its own, and the code
/// between two doc comments is still the file's, though all of its doc
/// comments make one Markdown document around it.
#[test]
fn doc_comment_code_blocks_have_their_own_tree() {
    let text =
        "/// ```\n/// fn inner() {}\n/// ```\nfn outer() {\n    1\n}\n\n/// Last.\nfn last() {}\n";
    let (mut editor, path) = editor_with(&["rust", "markdown"], "doc-tree.rs", text);
    let at = |editor: &mut Editor, what: &str| {
        let pos = text.find(what).unwrap();
        editor.view_mut().selection =
            Selection::new(vec![Range::new(pos, pos + 1)], 0, editor.buffer().text()).unwrap();
    };
    at(&mut editor, "inner");
    type_keys(&mut editor, "maf");
    assert_eq!(selected(&editor), "fn inner() {}");
    // Growing the selection stays in the file's tree.
    at(&mut editor, "1\n");
    editor.handle_key(alt(KeyCode::Char('o')));
    assert_eq!(selected(&editor), "{\n    1\n}");
    fs::remove_file(&path).unwrap();
}

#[test]
fn jumps_across_long_gaps() {
    // Farther apart than the first window the keymap searches.
    let gap = "// padding\n".repeat(2000);
    let source = format!("fn first() {{}}\n{gap}fn second() {{}}\n{gap}");
    let (mut editor, path) = rust_editor_with("gaps", &source);
    type_keys(&mut editor, "]f");
    assert_eq!(selected(&editor), "fn second() {}");
    type_keys(&mut editor, "]f");
    assert_eq!(selected(&editor), "fn second() {}");
    type_keys(&mut editor, "[f");
    assert_eq!(selected(&editor), "fn first() {}");
    fs::remove_file(&path).unwrap();
}

#[test]
fn expands_shrinks_and_walks_nodes() {
    let (mut editor, path) = rust_editor("nodes");
    // Without an Alt-o to undo, Alt-i goes to the first named child.
    let start = SOURCE.find("(a: u8").unwrap();
    let end = SOURCE.find(" -> u8").unwrap();
    editor.view_mut().selection =
        Selection::new(vec![Range::new(start, end)], 0, editor.buffer().text()).unwrap();
    editor.handle_key(alt(KeyCode::Char('i')));
    assert_eq!(selected(&editor), "a: u8");

    put_cursor(&mut editor, "a + b");
    editor.handle_key(alt(KeyCode::Char('o')));
    assert_eq!(selected(&editor), "a + b");
    editor.handle_key(alt(KeyCode::Up));
    assert!(selected(&editor).starts_with("{\n    let"));
    editor.handle_key(alt(KeyCode::Char('i')));
    assert_eq!(selected(&editor), "a + b");
    editor.handle_key(alt(KeyCode::Down));
    assert_eq!(selected(&editor), "a");

    put_cursor(&mut editor, "(a: u8");
    editor.handle_key(alt(KeyCode::Char('o')));
    assert_eq!(selected(&editor), "(a: u8, b: u8)");

    put_cursor(&mut editor, "a: u8");
    editor.handle_key(alt(KeyCode::Char('o')));
    assert_eq!(selected(&editor), "a: u8");
    editor.handle_key(alt(KeyCode::Char('n')));
    assert_eq!(selected(&editor), "b: u8");
    editor.handle_key(alt(KeyCode::Char('p')));
    assert_eq!(selected(&editor), "a: u8");
    fs::remove_file(&path).unwrap();
}

#[test]
fn matches_pairs_with_the_tree() {
    let (mut editor, path) = rust_editor("pairs");
    // The text alone would stop at the brace in the string.
    put_cursor(&mut editor, "{\n    let");
    type_keys(&mut editor, "mm");
    let close = SOURCE.find("}\n\nfn other").unwrap();
    assert_eq!(editor.view().cursor(editor.buffer().text()), close);
    type_keys(&mut editor, "mm");
    assert_eq!(
        editor.view().cursor(editor.buffer().text()),
        SOURCE.find("{\n    let").unwrap()
    );

    put_cursor(&mut editor, "a + b");
    type_keys(&mut editor, "mi{");
    assert!(selected(&editor).starts_with("\n    let s = \"}\";"));
    put_cursor(&mut editor, "}\";");
    type_keys(&mut editor, "ma\"");
    assert_eq!(selected(&editor), "\"}\"");
    fs::remove_file(&path).unwrap();
}

#[test]
fn highlights_the_matching_bracket() {
    let (mut editor, path) = rust_editor("highlight");
    let underlined = |editor: &Editor, x, y| {
        let mut grid = Grid::default();
        editor.render(&mut grid);
        grid.cell(x, y).style.underline
    };
    put_cursor(&mut editor, "{\n    let");
    // Updated after each key the keymap handles.
    type_keys(&mut editor, ";");
    assert!(underlined(&editor, 0, 5), "the closing brace");
    assert!(!underlined(&editor, 13, 3), "the brace in the string");
    type_keys(&mut editor, "j");
    assert!(!underlined(&editor, 0, 5));
    fs::remove_file(&path).unwrap();
}

#[test]
fn the_matching_bracket_waits_for_the_tree_after_an_edit() {
    let (mut editor, path) = rust_editor_with("match-edit", "fn f() {}\n");
    let underlined = |editor: &Editor, x| {
        let mut grid = Grid::default();
        editor.render(&mut grid);
        grid.cell(x, 0).style.underline
    };
    let text = editor.buffer().text().to_string();
    let at = text.find('}').unwrap();
    editor.view_mut().selection =
        Selection::new(vec![Range::new(at, at + 1)], 0, editor.buffer().text()).unwrap();
    type_keys(&mut editor, ";");
    assert!(underlined(&editor, 7), "the opening brace");
    // Deleting the closing brace leaves the opening one without a pair,
    // which shows once the tree is parsed, not while the key is handled.
    type_keys(&mut editor, "d");
    assert_eq!(editor.buffer().text().to_string(), "fn f() {\n");
    assert!(underlined(&editor, 7), "not yet updated");
    while editor.catch_up() {}
    assert!(!underlined(&editor, 7), "updated after the parse");
    fs::remove_file(&path).unwrap();
}

/// Every standard language: its grammar loads, its queries compile, and a
/// sample gets some color.
#[test]
fn every_language_loads_its_queries() {
    let samples = [
        ("rust", "rs", "fn main() {} // c\n"),
        ("python", "py", "def f(a, b):\n    return 'x'  # c\n"),
        ("go", "go", "package main\n\nfunc f(a int) {} // c\n"),
        ("bash", "sh", "f() { echo \"x\"; } # c\n"),
        ("json", "json", "{\"a\": [1, true]}\n"),
        ("toml", "toml", "[a]\nb = \"x\" # c\n"),
        ("yaml", "yaml", "a: [1, \"x\"] # c\n"),
        ("markdown", "md", "# Title\n\ntext\n"),
    ];
    for (name, extension, sample) in samples {
        let path = env::temp_dir().join(format!("nib-{}-sample.{extension}", std::process::id()));
        fs::write(&path, sample).unwrap();
        let mut editor = Editor::default();
        editor.open(&path).unwrap();
        editor.load_plugin(&plugin_dir("helix")).unwrap();
        editor.load_plugin(&plugin_dir(name)).unwrap();
        editor.resize(40, 6);
        editor.catch_up();
        assert_eq!(editor.message(), None, "{name}");
        let mut grid = Grid::default();
        editor.render(&mut grid);
        let colored = (0..grid.width()).any(|x| grid.cell(x, 0).style.fg != Color::Reset);
        assert!(colored, "{name}: {:?}", screen(&editor)[0]);
        // Compiles the text objects query.
        type_keys(&mut editor, "maf");
        assert_eq!(editor.message(), None, "{name}");
        fs::remove_file(&path).unwrap();
    }
}

#[test]
fn text_objects_in_other_languages() {
    let cases = [
        // (language, extension, text, keys, selected)
        (
            "python",
            "py",
            "def f(a, b):\n    return a\n",
            "jmaf",
            "def f(a, b):\n    return a",
        ),
        ("python", "py", "def f(a, b):\n    return a\n", "fbmia", "b"),
        (
            "toml",
            "toml",
            "[a]\nx = 1\n\n[b]\ny = 2\n",
            "]t]t",
            "[b]\ny = 2\n",
        ),
        (
            "markdown",
            "md",
            "# One\n\ntext\n\n# Two\n\nmore\n",
            "]t]t",
            "# Two\n\nmore\n",
        ),
        (
            "go",
            "go",
            "package p\n\nfunc TestX(t int) {}\n",
            "]T",
            "func TestX(t int) {}",
        ),
    ];
    for (name, extension, text, keys, expected) in cases {
        let path = env::temp_dir().join(format!("nib-{}-objects.{extension}", std::process::id()));
        fs::write(&path, text).unwrap();
        let mut editor = Editor::default();
        editor.open(&path).unwrap();
        editor.load_plugin(&plugin_dir("helix")).unwrap();
        editor.load_plugin(&plugin_dir(name)).unwrap();
        editor.resize(40, 12);
        editor.catch_up();
        type_keys(&mut editor, keys);
        assert_eq!(selected(&editor), expected, "{name}: {keys}");
        fs::remove_file(&path).unwrap();
    }
}

fn editor_with(languages: &[&str], name: &str, text: &str) -> (Editor, std::path::PathBuf) {
    let path = env::temp_dir().join(format!("nib-{}-{name}", std::process::id()));
    fs::write(&path, text).unwrap();
    let mut editor = Editor::default();
    editor.open(&path).unwrap();
    editor.load_plugin(&plugin_dir("helix")).unwrap();
    for language in languages {
        editor.load_plugin(&plugin_dir(language)).unwrap();
    }
    editor.resize(40, 12);
    while editor.catch_up() {}
    assert_eq!(editor.message(), None);
    (editor, path)
}

const TITLE: Color = Color::Indexed(4);
const LITERAL: Color = Color::Indexed(2);

#[test]
fn colors_code_blocks_in_their_language() {
    let text = "# Notes\n\n```rust\nfn main() {}\n```\n\n```unknown\nfn x\n```\n";
    let (mut editor, path) = editor_with(&["markdown", "rust"], "blocks.md", text);
    assert_eq!(fg(&editor, 0, 3), KEYWORD);
    assert_eq!(fg(&editor, 3, 3), FUNCTION);
    // The fences stay Markdown, and so do blocks of unknown languages.
    assert_eq!(fg(&editor, 0, 2), LITERAL);
    assert_eq!(fg(&editor, 0, 7), LITERAL);

    // Edits inside and before the block.
    type_keys(&mut editor, "jjjA // c<esc>");
    assert_eq!(fg(&editor, 13, 3), COMMENT);
    assert_eq!(fg(&editor, 0, 3), KEYWORD);
    type_keys(&mut editor, "ggO<ret><esc>");
    assert_eq!(fg(&editor, 0, 5), KEYWORD);
    assert_eq!(fg(&editor, 13, 5), COMMENT);

    // Undo reaches the injections too.
    type_keys(&mut editor, "uu");
    assert_eq!(fg(&editor, 0, 3), KEYWORD);
    assert_eq!(fg(&editor, 13, 3), Color::Reset);
    fs::remove_file(&path).unwrap();
}

/// A block with no language is in the language the Markdown is in: Rust
/// in a doc comment, as rustdoc reads it. In a Markdown file it stays
/// Markdown's.
#[test]
fn code_blocks_with_no_language_are_in_their_host_language() {
    let text = "/// ```\n/// let x = 1;\n/// ```\nfn f() {}\n";
    let (editor, path) = editor_with(&["rust", "markdown"], "bare.rs", text);
    assert_eq!(fg(&editor, 4, 1), KEYWORD);
    fs::remove_file(&path).unwrap();

    // So is one with only rustdoc's attributes, but not with other words.
    let text = "/// ```no_run,should_panic\n/// let x = 1;\n/// ```\n/// ```text\n/// let y\n/// ```\nfn f() {}\n";
    let (editor, path) = editor_with(&["rust", "markdown"], "attributes.rs", text);
    assert_eq!(fg(&editor, 4, 1), KEYWORD);
    assert_ne!(fg(&editor, 4, 4), KEYWORD);
    fs::remove_file(&path).unwrap();

    let text =
        "```\nfn main() {}\n```\n\n```rust\nfn main() {}\n```\n\n```ignore\nfn main() {}\n```\n";
    let (editor, path) = editor_with(&["markdown", "rust"], "bare.md", text);
    assert_eq!(fg(&editor, 0, 1), LITERAL);
    // One with a language is only in that language.
    assert_eq!(fg(&editor, 0, 5), KEYWORD);
    // Attributes mean nothing outside Rust.
    assert_eq!(fg(&editor, 0, 9), LITERAL);
    fs::remove_file(&path).unwrap();
}

#[test]
fn doc_comments_are_markdown() {
    let text = "/// # Title\n///\n/// ```rust\n/// let x = 1;\n/// ```\nfn f() {}\n// # plain\n";
    let (mut editor, path) = editor_with(&["rust", "markdown"], "doc.rs", text);
    assert_eq!(fg(&editor, 6, 0), TITLE);
    // The comment markers are not part of the Markdown.
    assert_eq!(fg(&editor, 0, 0), COMMENT);
    assert_eq!(fg(&editor, 0, 3), COMMENT);
    // A code block over several comment lines, in Rust again.
    assert_eq!(fg(&editor, 4, 3), KEYWORD);
    assert_eq!(fg(&editor, 5, 6), COMMENT);

    // A new doc comment line joins the document.
    type_keys(&mut editor, "jjjo/// let y = 2;<esc>");
    assert_eq!(fg(&editor, 4, 4), KEYWORD);

    // So does a plain comment made a doc comment.
    assert_eq!(fg(&editor, 5, 7), COMMENT);
    let at = editor
        .buffer()
        .text()
        .to_string()
        .find("// # plain")
        .unwrap();
    editor.view_mut().selection =
        Selection::new(vec![Range::new(at, at + 1)], 0, editor.buffer().text()).unwrap();
    type_keys(&mut editor, "i/<esc>");
    let row = screen(&editor)
        .iter()
        .position(|r| r.starts_with("/// # plain"));
    assert_eq!(fg(&editor, 6, row.unwrap() as u16), TITLE);

    // And leaves it when made plain again, in the middle too.
    let at = editor.buffer().text().to_string().find("/ let y").unwrap();
    editor.view_mut().selection =
        Selection::new(vec![Range::new(at, at + 1)], 0, editor.buffer().text()).unwrap();
    type_keys(&mut editor, "d");
    let row = screen(&editor)
        .iter()
        .position(|r| r.starts_with("// let y"));
    assert_eq!(fg(&editor, 3, row.unwrap() as u16), COMMENT);
    fs::remove_file(&path).unwrap();
}

#[test]
fn injections_follow_languages_loaded_later() {
    let (mut editor, path) = editor_with(&["rust"], "later.rs", "/// # Title\nfn f() {}\n");
    assert_eq!(fg(&editor, 6, 0), COMMENT);
    editor.load_plugin(&plugin_dir("markdown")).unwrap();
    while editor.catch_up() {}
    assert_eq!(fg(&editor, 6, 0), TITLE);
    fs::remove_file(&path).unwrap();
}

#[test]
fn front_matter_is_yaml() {
    let text = "---\nn: 1\n---\n\n# Notes\n";
    let (editor, path) = editor_with(&["markdown", "yaml"], "front.md", text);
    assert_eq!(fg(&editor, 3, 1), Color::Indexed(6), "the number");
    assert_eq!(fg(&editor, 2, 4), TITLE);
    fs::remove_file(&path).unwrap();
}

#[test]
fn injections_come_after_the_first_colors() {
    let path = env::temp_dir().join(format!("nib-{}-first.rs", std::process::id()));
    fs::write(&path, "/// # Title\nfn f() {}\n").unwrap();
    let mut editor = Editor::default();
    editor.open(&path).unwrap();
    editor.load_plugin(&plugin_dir("rust")).unwrap();
    editor.load_plugin(&plugin_dir("markdown")).unwrap();
    editor.resize(40, 6);
    assert!(editor.catch_up());
    assert_eq!(fg(&editor, 0, 1), KEYWORD);
    assert_eq!(fg(&editor, 6, 0), COMMENT);
    assert!(editor.catch_up());
    assert_eq!(fg(&editor, 6, 0), TITLE);
    assert!(!editor.catch_up());
    fs::remove_file(&path).unwrap();
}

fn cell_style(editor: &Editor, x: u16, y: u16) -> nib_core::Style {
    let mut grid = Grid::default();
    editor.render(&mut grid);
    grid.cell(x, y).style
}

#[test]
fn inline_elements_have_their_own_grammar() {
    let text = "# A `b`\n\nx `code` *em* **strong**\n\n| `c` |\n|---|\n";
    let (mut editor, path) = editor_with(&["markdown"], "inline.md", text);
    assert_eq!(fg(&editor, 2, 0), TITLE);
    assert_eq!(fg(&editor, 5, 0), LITERAL, "code in a heading");
    assert_eq!(fg(&editor, 0, 2), Color::Reset);
    assert_eq!(fg(&editor, 4, 2), LITERAL);
    assert!(cell_style(&editor, 11, 2).italic);
    assert!(cell_style(&editor, 18, 2).bold);
    assert_eq!(fg(&editor, 3, 4), LITERAL, "code in a table cell");

    // Typing into a paragraph parses its inline elements again.
    type_keys(&mut editor, "jjA `y`<esc>");
    assert_eq!(fg(&editor, 27, 2), LITERAL);
    fs::remove_file(&path).unwrap();
}

#[test]
fn doc_comments_have_inline_elements() {
    let text = "/// Uses `code`.\nfn f() {}\n";
    let (editor, path) = editor_with(&["rust", "markdown"], "inline.rs", text);
    assert_eq!(fg(&editor, 0, 0), COMMENT);
    assert_eq!(fg(&editor, 10, 0), LITERAL);
    fs::remove_file(&path).unwrap();
}

#[test]
fn layers_far_from_the_screen_wait_until_it_comes() {
    let text = format!("{}```rust\nfn far() {{}}\n```\n", "text\n".repeat(100));
    let (mut editor, path) = editor_with(&["markdown", "rust"], "far.md", &text);
    type_keys(&mut editor, "ge");
    let row = screen(&editor).iter().position(|r| r.starts_with("fn far"));
    assert_eq!(fg(&editor, 0, row.unwrap() as u16), KEYWORD);
    // And edits far from the screen still reach them.
    type_keys(&mut editor, "ggO<ret><esc>ge");
    let row = screen(&editor).iter().position(|r| r.starts_with("fn far"));
    assert_eq!(fg(&editor, 0, row.unwrap() as u16), KEYWORD);
    fs::remove_file(&path).unwrap();
}

fn settle_in_background(editor: &mut Editor) {
    loop {
        editor.wait_for_syntax();
        // Taking trees in can send more parses, as of layers near the
        // screen.
        if !editor.catch_up() && !editor.is_parsing() {
            break;
        }
    }
}

#[test]
fn parses_in_the_background() {
    let path = env::temp_dir().join(format!("nib-{}-background.rs", std::process::id()));
    fs::write(&path, "fn main() {}\nfn other() {}\n").unwrap();
    let mut editor = Editor::default();
    editor.set_background_parsing(true);
    editor.open(&path).unwrap();
    editor.load_plugin(&plugin_dir("helix")).unwrap();
    editor.load_plugin(&plugin_dir("rust")).unwrap();
    editor.resize(40, 6);
    settle_in_background(&mut editor);
    assert_eq!(fg(&editor, 0, 0), KEYWORD);

    type_keys(&mut editor, "i// x<ret><esc>");
    settle_in_background(&mut editor);
    assert_eq!(fg(&editor, 0, 0), COMMENT);
    assert_eq!(fg(&editor, 0, 1), KEYWORD);
    assert_eq!(fg(&editor, 3, 2), FUNCTION);

    // Undo is an edit too.
    type_keys(&mut editor, "u");
    settle_in_background(&mut editor);
    assert_eq!(fg(&editor, 0, 0), KEYWORD);
    fs::remove_file(&path).unwrap();
}

/// Keys typed while the syntax thread parses change the text under it;
/// the tree it returns has them applied, then is parsed again. In the end
/// the colors are those of a tree parsed from the final text.
#[test]
fn edits_during_a_parse_are_applied_to_its_tree() {
    let source: String = (0..3000)
        .map(|i| format!("fn f{i}(a: u8) -> u8 {{ a + {i} }} // n\n"))
        .collect();
    let path = env::temp_dir().join(format!("nib-{}-replay.rs", std::process::id()));
    fs::write(&path, &source).unwrap();
    let open = |background: bool| {
        let mut editor = Editor::default();
        editor.set_background_parsing(background);
        editor.open(&path).unwrap();
        editor.load_plugin(&plugin_dir("helix")).unwrap();
        editor.load_plugin(&plugin_dir("rust")).unwrap();
        editor.resize(60, 20);
        editor
    };
    let mut background = open(true);
    settle_in_background(&mut background);
    // Each key starts or extends a parse of 3,000 lines, so later keys
    // land while one runs.
    type_keys(&mut background, "jjwwi/* <esc>jji\"<esc>kkA x */<esc>");
    let text = background.buffer().text().to_string();
    settle_in_background(&mut background);

    let mut foreground = open(false);
    let edited = env::temp_dir().join(format!("nib-{}-replay-final.rs", std::process::id()));
    fs::write(&edited, &text).unwrap();
    foreground.open(&edited).unwrap();
    foreground.view_mut().selection = background.view().selection.clone();
    while foreground.catch_up() {}

    let styles = |editor: &Editor| {
        let mut grid = Grid::default();
        editor.render(&mut grid);
        (0..grid.height() - 2)
            .flat_map(|y| (0..grid.width()).map(move |x| (x, y)))
            .map(|(x, y)| grid.cell(x, y).style.fg)
            .collect::<Vec<_>>()
    };
    assert_eq!(styles(&background), styles(&foreground));
    fs::remove_file(&path).unwrap();
    fs::remove_file(&edited).unwrap();
}

/// Colors kept from frame to frame and moved with edits come out as a
/// full paint of the same text would, including Markdown in doc comments.
#[test]
fn kept_colors_match_a_full_paint() {
    let block =
        "/// # Title\n/// Some `code`.\nfn f(a: u8) -> u8 {\n    let s = \"x\"; // c\n    a\n}\n\n";
    let source = block.repeat(30);
    let path = env::temp_dir().join(format!("nib-{}-kept.rs", std::process::id()));
    fs::write(&path, &source).unwrap();
    let cache = env::temp_dir().join(format!("nib-{}-kept-cache", std::process::id()));
    let open = |path: &std::path::Path, background: bool| {
        let mut editor = Editor::default();
        editor.set_background_parsing(background);
        editor.set_plugin_cache_dir(Some(cache.clone()));
        editor.open(path).unwrap();
        for plugin in ["helix", "rust", "markdown"] {
            editor.load_plugin(&plugin_dir(plugin)).unwrap();
        }
        editor.resize(60, 20);
        settle_in_background(&mut editor);
        editor
    };
    let colors = |editor: &Editor| {
        let mut grid = Grid::default();
        editor.render(&mut grid);
        (0..grid.height() - 2)
            .map(|y| {
                (0..grid.width())
                    .map(|x| grid.cell(x, y).style.fg)
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
    };
    // Parsing in the background, so the frame right after a key is drawn
    // with the old tree, moved.
    let mut editor = open(&path, true);
    // Each step ends with a key that edits, so the frame after it is drawn
    // with the old tree, moved, before the thread's parse is taken in.
    let steps = [
        "jjjA z",
        "<esc>kkkA more",
        "<esc>ggO/// ## New",
        "<esc>jjjjwwi\"",
        "<esc>xd",
        "u",
        "U",
        "i// out",
        "<esc><C-d>o}",
        "<esc><C-u>jjA `x`",
    ];
    let fresh_path = env::temp_dir().join(format!("nib-{}-kept-fresh.rs", std::process::id()));
    let full = |editor: &mut Editor| {
        editor.repaint_all();
        colors(editor)
    };
    for step in steps {
        colors(&editor);
        type_keys(&mut editor, step);
        let kept = colors(&editor);
        assert_eq!(kept, full(&mut editor), "right after {step:?}");
        settle_in_background(&mut editor);
        let kept = colors(&editor);
        assert_eq!(kept, full(&mut editor), "after parsing {step:?}");

        fs::write(&fresh_path, editor.buffer().text().to_string()).unwrap();
        let mut fresh = open(&fresh_path, false);
        fresh.view_mut().top_line = editor.view().top_line;
        fresh.view_mut().selection = editor.view().selection.clone();
        while fresh.catch_up() {}
        assert_eq!(
            colors(&editor),
            colors(&fresh),
            "opened fresh after {step:?}"
        );
    }
    fs::remove_file(&path).unwrap();
    fs::remove_file(&fresh_path).unwrap();
    let _ = fs::remove_dir_all(&cache);
}

/// Keys typed in a doc comment while the syntax thread parses the
/// Markdown of all doc comments land in its tree too.
#[test]
fn edits_during_a_layer_parse_are_applied_to_its_tree() {
    let source: String = (0..1500)
        .map(|i| format!("/// Item `{i}` and *more*\n/// # H{i}\nfn f{i}() {{}}\n"))
        .collect();
    let path = env::temp_dir().join(format!("nib-{}-layer-replay.rs", std::process::id()));
    fs::write(&path, &source).unwrap();
    let open = |path: &std::path::Path, background: bool| {
        let mut editor = Editor::default();
        editor.set_background_parsing(background);
        editor.open(path).unwrap();
        for plugin in ["helix", "rust", "markdown"] {
            editor.load_plugin(&plugin_dir(plugin)).unwrap();
        }
        editor.resize(60, 20);
        settle_in_background(&mut editor);
        editor
    };
    let mut background = open(&path, true);
    type_keys(&mut background, "wwwi`x` and **y** <esc>jA z<esc>");
    settle_in_background(&mut background);

    let edited = env::temp_dir().join(format!("nib-{}-layer-final.rs", std::process::id()));
    fs::write(&edited, background.buffer().text().to_string()).unwrap();
    let mut foreground = open(&edited, false);
    foreground.view_mut().top_line = background.view().top_line;
    foreground.view_mut().selection = background.view().selection.clone();
    while foreground.catch_up() {}
    let colors = |editor: &Editor| {
        let mut grid = Grid::default();
        editor.render(&mut grid);
        (0..grid.height() - 2)
            .flat_map(|y| (0..grid.width()).map(move |x| (x, y)))
            .map(|(x, y)| (grid.cell(x, y).style.fg, grid.cell(x, y).style.bold))
            .collect::<Vec<_>>()
    };
    assert_eq!(colors(&background), colors(&foreground));
    fs::remove_file(&path).unwrap();
    fs::remove_file(&edited).unwrap();
}

/// Frames drawn while the syntax thread parses, with keys coming faster
/// than parses, so parses come back after more edits: the colors kept from
/// the frames must end up as a full paint's. CI once found them apart,
/// with a late parse's tree shown without marking what it changed; this
/// did not catch that here, but goes that way on every run.
#[test]
fn kept_colors_survive_parses_that_come_back_late() {
    let block =
        "/// # Title\n/// Some `code`.\nfn f(a: u8) -> u8 {\n    let s = \"x\"; // c\n    a\n}\n\n";
    let source = block.repeat(1500);
    let path = env::temp_dir().join(format!("nib-{}-late.rs", std::process::id()));
    fs::write(&path, &source).unwrap();
    let mut editor = Editor::default();
    editor.set_background_parsing(true);
    editor.open(&path).unwrap();
    for plugin in ["helix", "rust", "markdown"] {
        editor.load_plugin(&plugin_dir(plugin)).unwrap();
    }
    editor.resize(60, 20);
    settle_in_background(&mut editor);
    let colors = |editor: &Editor| {
        let mut grid = Grid::default();
        editor.render(&mut grid);
        (0..grid.height() - 2)
            .map(|y| {
                (0..grid.width())
                    .map(|x| grid.cell(x, y).style.fg)
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
    };
    for round in 0..20 {
        // Keys, each drawn, taking in whatever the thread has finished.
        for key in ["gg", "O", "/", "/", "/", " ", "#", "x", "<esc>", "j", "d"] {
            type_keys(&mut editor, key);
            editor.catch_up();
            colors(&editor);
        }
        settle_in_background(&mut editor);
        let kept = colors(&editor);
        editor.repaint_all();
        assert_eq!(kept, colors(&editor), "round {round}");
    }
    fs::remove_file(&path).unwrap();
}
