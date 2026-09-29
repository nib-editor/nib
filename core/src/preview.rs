//! The file a box shows beside its rows (docs/finder.md): read once per
//! path, parsed for its colors, and drawn from its start or around a line.

use std::borrow::Cow;
use std::io::Read;
use std::path::{Path, PathBuf};

use ropey::Rope;

use crate::editor::{Editor, State};
use crate::grid::{Grid, Style, display_width, graphemes};
use crate::render::put_clipped;
use crate::syntax::BufferSyntax;
use crate::windows::Rect;

/// Read at most, so a large file does not hold up choosing.
const MOST: u64 = 128 * 1024;

pub(crate) struct LoadedFile {
    pub path: PathBuf,
    pub text: Rope,
    pub syntax: Option<BufferSyntax>,
    /// Said instead of the text: a directory, a binary file, or why it
    /// could not be read.
    pub note: Option<String>,
}

impl State {
    /// Reads `path` for a preview, unless it is the one read last.
    pub(crate) fn load_preview(&mut self, path: &Path) {
        if self.preview.as_ref().is_some_and(|p| p.path == path) {
            return;
        }
        let mut loaded = LoadedFile {
            path: path.to_path_buf(),
            text: Rope::new(),
            syntax: None,
            note: None,
        };
        let mut bytes = Vec::new();
        let read =
            std::fs::File::open(path).and_then(|file| file.take(MOST).read_to_end(&mut bytes));
        match read {
            _ if path.is_dir() => loaded.note = Some("a directory".into()),
            Err(err) => loaded.note = Some(err.to_string()),
            Ok(_) if bytes.contains(&0) => loaded.note = Some("binary file".into()),
            Ok(_) => {
                let mut text = String::from_utf8_lossy(&bytes).into_owned();
                // Cut short, the last line is whole or not there.
                if bytes.len() as u64 == MOST
                    && let Some(end) = text.rfind('\n')
                {
                    text.truncate(end + 1);
                }
                loaded.text = Rope::from_str(&text);
                if let Some(language) = self.languages.for_path(path) {
                    let mut syntax = BufferSyntax::new(language);
                    if self.languages.parse(&mut syntax, &loaded.text).is_ok() {
                        loaded.syntax = Some(syntax);
                    }
                }
            }
        }
        self.preview = Some(loaded);
    }
}

impl Editor {
    /// Draws `file` in `rect`: from its start, or with `line` a third of
    /// the way down, marked.
    pub(crate) fn draw_file(
        &self,
        grid: &mut Grid,
        rect: Rect,
        file: &LoadedFile,
        line: Option<usize>,
        base: Style,
    ) {
        let state = self.state();
        let theme = &state.theme;
        let end = rect.x + rect.width;
        if let Some(note) = &file.note {
            let dim = base.patch(theme.style("ui.window").unwrap_or_default());
            put_clipped(grid, rect.x, rect.y, note, dim, end);
            return;
        }
        let text = &file.text;
        let lines = text.len_lines();
        let rows = usize::from(rect.height);
        let top = line
            .map_or(0, |l| l.saturating_sub(rows / 3))
            .min(lines.saturating_sub(1));
        let start = text.line_to_byte(top);
        let stop = text.line_to_byte((top + rows).min(lines));
        let styles = file
            .syntax
            .as_ref()
            .filter(|s| s.tree.is_some())
            .map(|syntax| state.languages.highlight(theme, syntax, text, start..stop));
        let marked = base.patch(theme.style("ui.selection").unwrap_or_default());
        let tab_width = u32::from(state.settings.tab_width);
        for row in 0..rows {
            let index = top + row;
            if index >= lines {
                break;
            }
            let y = rect.y + row as u16;
            let line_style = if Some(index) == line { marked } else { base };
            if Some(index) == line {
                let mut x = rect.x;
                while x < end {
                    x = grid.put_grapheme(x, y, " ", line_style);
                }
            }
            let content: Cow<str> = text.line(index).into();
            let content = content.trim_end_matches(['\n', '\r']);
            let mut offset = text.line_to_byte(index);
            let mut column = 0u32;
            for grapheme in graphemes(content) {
                let style = styles
                    .as_ref()
                    .and_then(|styles| styles.get(offset - start).copied().flatten())
                    .map_or(line_style, |style| line_style.patch(style));
                let size = match grapheme {
                    "\t" => tab_width - column % tab_width,
                    _ => u32::from(display_width(grapheme)),
                };
                if column + size > u32::from(rect.width) {
                    break;
                }
                match grapheme {
                    // A tab as the blanks up to its stop.
                    "\t" => {
                        for blank in column..column + size {
                            grid.put_grapheme(rect.x + blank as u16, y, " ", style);
                        }
                    }
                    _ => {
                        grid.put_grapheme(rect.x + column as u16, y, grapheme, style);
                    }
                }
                column += size;
                offset += grapheme.len();
            }
        }
    }
}
