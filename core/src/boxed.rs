//! The box in the middle of the screen that the core menu and plugins'
//! prompts show in (docs/core-menu.md, docs/finder.md): a title, an input
//! line, rows to choose from, and details or a file beside them.

use crate::editor::Editor;
use crate::grid::{Cursor, CursorShape, Grid, Style, display_width, graphemes};
use crate::preview::LoadedFile;
use crate::prompt::Preview;
use crate::render::put_clipped;
use crate::ui::{Span, StyledLine, Theme};
use crate::windows::Rect;

/// What a box shows.
pub(crate) struct Content<'a> {
    pub title: String,
    pub input: Input,
    /// At the right end of the input, such as "3/10".
    pub count: String,
    /// Rows to choose from, and the selected one. Without them, the side
    /// takes the whole width.
    pub rows: Option<(&'a [StyledLine], Option<usize>)>,
    pub side: Side<'a>,
    /// The keys that work, in the bottom border.
    pub keys: &'a str,
}

pub(crate) enum Input {
    /// Text being typed after `label`, with the cursor a byte offset in it.
    Line {
        label: String,
        text: String,
        cursor: usize,
    },
    /// A question to answer with a key.
    Question(String),
}

pub(crate) enum Side<'a> {
    Lines(Vec<StyledLine>),
    /// A file, from the start or around a marked line.
    File(&'a LoadedFile, Option<usize>),
}

/// Where the box goes: its corner and size, and the width of the list
/// inside it, with the side beside the list when there is room.
pub(crate) struct Frame {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    /// Columns of the list, between the left border and the divider.
    pub list: u16,
    pub split: bool,
}

impl Frame {
    pub fn new(width: u16, height: u16) -> Option<Frame> {
        let fit = |size: u16, small: u16, part: u16, most: u16| {
            if size < small {
                size
            } else {
                (size * part / 10).clamp(small, most)
            }
        };
        let w = fit(width, 44, 9, 120);
        let h = fit(height, 12, 8, 30);
        if w < 10 || h < 4 {
            return None;
        }
        let inner = w - 2;
        let split = w >= 72;
        Some(Frame {
            x: (width - w) / 2,
            y: (height - h) / 2,
            width: w,
            height: h,
            list: if split { inner * 45 / 100 } else { inner },
            split,
        })
    }

    /// Rows for the list and the side, below the input line.
    pub fn body_rows(&self) -> u16 {
        self.height - 4
    }

    pub fn right(&self) -> u16 {
        self.x + self.width - 1
    }

    pub fn divider(&self) -> u16 {
        self.x + 1 + self.list
    }
}

impl Editor {
    /// Draws a box over what is on the screen. Returns the cursor, in its
    /// input line.
    pub(crate) fn draw_box(&self, grid: &mut Grid, content: &Content) -> Option<Cursor> {
        let frame = Frame::new(grid.width(), grid.height())?;
        let theme = &self.state().theme;
        let base = theme.style("ui.menu").unwrap_or_default();
        let dim = base.patch(theme.style("ui.window").unwrap_or_default());
        let bold = base.patch(theme.style("ui.popup.title").unwrap_or_default());
        let divider = (content.rows.is_some() && frame.split).then(|| frame.divider());
        draw_frame(grid, &frame, divider, dim, base);
        let right = frame.right();
        let bottom = frame.y + frame.height - 1;
        put_clipped(
            grid,
            frame.x + 2,
            frame.y,
            &format!(" {} ", content.title),
            bold,
            right - 1,
        );
        put_clipped(grid, frame.x + 2, bottom, content.keys, dim, right - 1);

        // The input line.
        let y = frame.y + 1;
        let end = right - 1;
        let cursor = match &content.input {
            Input::Line {
                label,
                text,
                cursor,
            } => {
                let at = put_clipped(grid, frame.x + 2, y, label, dim, end);
                let before = text.get(..*cursor).unwrap_or(text);
                let x = at + width(before).min(u16::MAX as usize) as u16;
                put_clipped(grid, at, y, text, base, end);
                Some(Cursor {
                    x: x.min(end),
                    y,
                    shape: CursorShape::Bar,
                })
            }
            Input::Question(question) => {
                put_clipped(grid, frame.x + 2, y, question, bold, end);
                None
            }
        };
        if !content.count.is_empty() {
            let x = end.saturating_sub(width(&content.count) as u16);
            put_clipped(grid, x, y, &content.count, dim, end);
        }

        let first = frame.y + 3;
        let body = frame.body_rows();
        if let Some((rows, selected)) = content.rows {
            let selected_style = base.patch(theme.style("ui.menu.selected").unwrap_or_default());
            let left = frame.x + 1;
            let list_end = divider.unwrap_or(right);
            let top = selected.map_or(0, |s| s.saturating_sub(usize::from(body).saturating_sub(1)));
            for (i, row) in rows.iter().enumerate().skip(top).take(usize::from(body)) {
                let y = first + (i - top) as u16;
                let style = if Some(i) == selected {
                    selected_style
                } else {
                    base
                };
                let mut at = left;
                while at < list_end {
                    at = grid.put_grapheme(at, y, " ", style);
                }
                put_spans(grid, theme, left + 1, y, row, style, list_end - 1);
            }
            if divider.is_none() {
                return cursor;
            }
        }
        let side_x = divider.map_or(frame.x + 2, |d| d + 2);
        let rect = Rect {
            x: side_x,
            y: first,
            width: end.saturating_sub(side_x),
            height: body,
        };
        match &content.side {
            Side::Lines(lines) => {
                let wrapped = lines
                    .iter()
                    .flat_map(|line| wrap_line(line, usize::from(rect.width)));
                for (i, line) in wrapped.take(usize::from(body)).enumerate() {
                    put_spans(grid, theme, side_x, first + i as u16, &line, base, end);
                }
            }
            Side::File(file, line) => self.draw_file(grid, rect, file, *line, base),
        }
        cursor
    }
}

impl Editor {
    /// Draws the newest prompt, when it is shown in a box. Returns the
    /// cursor, in its input line.
    pub(crate) fn render_prompt_box(&self, grid: &mut Grid) -> Option<Cursor> {
        let state = self.state();
        let prompt = state.prompts.last()?;
        let boxed = prompt.boxed.as_ref()?;
        let side = match &boxed.preview {
            Preview::None => Side::Lines(Vec::new()),
            Preview::Lines(lines) => Side::Lines(lines.clone()),
            Preview::File { path, line } => match &state.preview {
                Some(file) if &file.path == path => Side::File(file, *line),
                _ => Side::Lines(Vec::new()),
            },
        };
        let content = Content {
            title: boxed.title.clone(),
            input: Input::Line {
                label: prompt.label.clone(),
                text: prompt.text.clone(),
                cursor: prompt.cursor,
            },
            count: prompt.hint.clone(),
            rows: Some((&boxed.rows, boxed.selected)),
            side,
            keys: " ↑↓ move · enter choose · esc close ",
        };
        self.draw_box(grid, &content)
    }
}

fn draw_frame(grid: &mut Grid, frame: &Frame, divider: Option<u16>, dim: Style, base: Style) {
    let Frame { x, y, .. } = *frame;
    let right = frame.right();
    let bottom = y + frame.height - 1;
    for row in y..=bottom {
        let mut at = x;
        while at <= right {
            at = grid.put_grapheme(at, row, " ", base);
        }
        grid.put_grapheme(x, row, "│", dim);
        grid.put_grapheme(right, row, "│", dim);
    }
    for (row, left, middle, end) in [
        (y, "╭", "─", "╮"),
        (y + 2, "├", if divider.is_some() { "┬" } else { "─" }, "┤"),
        (bottom, "╰", if divider.is_some() { "┴" } else { "─" }, "╯"),
    ] {
        grid.put_grapheme(x, row, left, dim);
        for at in x + 1..right {
            let line = if Some(at) == divider { middle } else { "─" };
            grid.put_grapheme(at, row, line, dim);
        }
        grid.put_grapheme(right, row, end, dim);
    }
    if let Some(divider) = divider {
        for row in y + 3..bottom {
            grid.put_grapheme(divider, row, "│", dim);
        }
    }
}

/// `line` broken into lines of at most `columns`, after a space where
/// there is one, each part keeping its style.
fn wrap_line(line: &[Span], columns: usize) -> Vec<StyledLine> {
    let mut lines: Vec<StyledLine> = vec![Vec::new()];
    let mut used = 0;
    for span in line {
        for word in span.text.split_inclusive(' ') {
            let size = width(word.trim_end());
            if used > 0 && used + size > columns {
                lines.push(Vec::new());
                used = 0;
            }
            // A word longer than a line is cut.
            for grapheme in graphemes(word) {
                let size = display_width(grapheme) as usize;
                if used > 0 && used + size > columns {
                    // A space where a line breaks is not shown.
                    if grapheme == " " {
                        continue;
                    }
                    lines.push(Vec::new());
                    used = 0;
                }
                let current = lines.last_mut().expect("never empty");
                match current.last_mut() {
                    Some(last) if last.style == span.style => last.text.push_str(grapheme),
                    _ => current.push(Span {
                        text: grapheme.into(),
                        style: span.style.clone(),
                    }),
                }
                used += size;
            }
        }
    }
    lines
}

/// Columns `text` takes on screen.
pub(crate) fn width(text: &str) -> usize {
    graphemes(text).map(|g| display_width(g) as usize).sum()
}

/// Puts spans from `x`, each styled by the theme over `base`, stopping
/// before column `end`.
pub(crate) fn put_spans(
    grid: &mut Grid,
    theme: &Theme,
    mut x: u16,
    y: u16,
    line: &[Span],
    base: Style,
    end: u16,
) {
    for span in line {
        let style = theme
            .style(&span.style)
            .map_or(base, |style| base.patch(style));
        x = put_clipped(grid, x, y, &span.text, style, end);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn side_lines_wrap_after_spaces_and_keep_their_styles() {
        let line = vec![
            Span {
                text: "ab ".into(),
                style: "x".into(),
            },
            Span {
                text: "cd efghij".into(),
                style: "".into(),
            },
        ];
        let texts: Vec<Vec<(String, String)>> = wrap_line(&line, 5)
            .into_iter()
            .map(|l| l.into_iter().map(|s| (s.text, s.style)).collect())
            .collect();
        let part = |t: &str, s: &str| (t.to_string(), s.to_string());
        assert_eq!(
            texts,
            [
                vec![part("ab ", "x"), part("cd", "")],
                vec![part("efghi", "")],
                vec![part("j", "")],
            ]
        );
    }

    #[test]
    fn the_box_takes_most_of_the_screen_and_leaves_out_the_side_when_narrow() {
        let wide = Frame::new(100, 40).unwrap();
        assert_eq!((wide.width, wide.height, wide.x, wide.y), (90, 30, 5, 5));
        assert!(wide.split);
        let narrow = Frame::new(50, 10).unwrap();
        assert_eq!((narrow.width, narrow.height), (45, 10));
        assert!(!narrow.split);
        assert!(Frame::new(8, 3).is_none());
    }
}
