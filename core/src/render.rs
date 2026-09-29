use std::borrow::Cow;

use unicode_segmentation::UnicodeSegmentation;

use crate::buffer::Buffer;
use crate::editor::Editor;
use crate::grid::{Cursor, CursorShape, Grid, Style, display_width, graphemes};
use crate::layout;
use crate::prompt::Prompt;
use crate::ui::{PopupAnchor, Side, Span, StyledLine, Theme};
use crate::view::View;
use crate::windows::{Rect, Separator};

impl Editor {
    /// Draws the editor into `grid`, resizing it to the editor size.
    /// Returns the cursor if it is on screen.
    pub fn render(&self, grid: &mut Grid) -> Option<Cursor> {
        let (width, height) = self.size();
        grid.reset(width, height);
        if width == 0 || height == 0 {
            return None;
        }
        let text_rows = self.text_rows();
        let status_row = (height > 1).then(|| height - 1);
        let mut cursor = self.render_views(grid, text_rows);
        self.render_popups(grid, text_rows);
        let panel_end = status_row.unwrap_or(height);
        if let Some(panel_cursor) = self.render_panels(grid, text_rows, panel_end) {
            cursor = Some(panel_cursor);
        }
        if let Some(y) = status_row {
            self.render_status(grid, y);
        }
        // Over the text, and with the cursor in its input line.
        if let Some(box_cursor) = self.render_prompt_box(grid) {
            cursor = Some(box_cursor);
        }
        if let Some(menu_cursor) = self.render_menu(grid) {
            cursor = Some(menu_cursor);
        }
        cursor
    }

    /// Rows left for the views above the panels and the status line.
    pub(crate) fn text_rows(&self) -> u16 {
        self.state().text_area_rows()
    }

    /// Draws panels from row `top` down to `end`, oldest first. Returns the
    /// cursor of the last panel that has one.
    fn render_panels(&self, grid: &mut Grid, top: u16, end: u16) -> Option<Cursor> {
        let mut cursor = None;
        let mut y = top;
        let prompt = self
            .state()
            .prompts
            .last()
            .filter(|p| p.boxed.is_none())
            .map(Prompt::panel);
        for panel in self.state().panels.iter().chain(&prompt) {
            for (i, line) in panel.lines.iter().enumerate() {
                if y >= end {
                    return cursor;
                }
                let x = put_line(grid, &self.state().theme, 0, y, line, Style::default());
                debug_assert!(x <= grid.width());
                if let Some((cursor_line, byte)) = panel.cursor
                    && cursor_line as usize == i
                {
                    let text: String = line.iter().map(|span| span.text.as_str()).collect();
                    let before = text.get(..byte as usize).unwrap_or(&text);
                    let x: u16 = graphemes(before).map(display_width).sum();
                    cursor = (x < grid.width()).then_some(Cursor {
                        x,
                        y,
                        shape: CursorShape::Bar,
                    });
                }
                y += 1;
            }
        }
        cursor
    }

    /// Draws every view in its part of the top `rows` rows, and the lines
    /// between them. Returns the focused view's cursor.
    fn render_views(&self, grid: &mut Grid, rows: u16) -> Option<Cursor> {
        let state = self.state();
        let (rects, separators) = state.view_layout(rows);
        let mut cursor = None;
        for (id, rect) in rects {
            let focused = id == state.focused;
            let drawn = self.render_view(grid, state.view_by_id(id), rect, focused);
            if focused {
                cursor = drawn;
            }
        }
        let style = state.theme.style("ui.window").unwrap_or_default();
        for separator in separators {
            match separator {
                Separator::Column { x, y, height } => {
                    for row in y..y + height {
                        grid.put_grapheme(x, row, "│", style);
                    }
                }
                // The name of the view above.
                Separator::Row { x, y, width, view } => {
                    let buffer = &state.buffers[state.view_by_id(view).buffer];
                    let name = buffer.name();
                    let end = x + width;
                    let mut at = put_clipped(grid, x, y, "── ", style, end);
                    at = put_clipped(grid, at, y, &name, style, end);
                    at = put_clipped(grid, at, y, " ", style, end);
                    while at < end {
                        at = grid.put_grapheme(at, y, "─", style);
                    }
                }
            }
        }
        cursor
    }

    /// Draws `view` in `rect`. Returns its cursor if it is `focused`.
    fn render_view(
        &self,
        grid: &mut Grid,
        view: &View,
        rect: Rect,
        focused: bool,
    ) -> Option<Cursor> {
        let rows = rect.height;
        let width = u32::from(rect.width);
        let tab_width = u32::from(self.state().tab_width(view.buffer));
        let buffer = &self.state().buffers[view.buffer];
        let text = buffer.text();
        let at = |x: u16, row: u16| (rect.x + x, rect.y + row);
        let ranges = view.selection.ranges();
        let selected = |offset: usize| {
            let i = ranges.partition_point(|range| range.to() <= offset);
            ranges.get(i).is_some_and(|range| range.from() <= offset)
        };
        let normal = Style::default();
        let selection_bg = self
            .state()
            .theme
            .style("ui.selection")
            .unwrap_or(normal)
            .bg;
        let visible_start = text.line_to_byte(view.top_line.min(text.len_lines() - 1));
        let visible_end = buffer
            .line_start(view.top_line + rows as usize)
            .unwrap_or(text.len_bytes());
        let syntax = self
            .state()
            .syntax_styles(view.buffer, visible_start..visible_end);
        let decorations = self.decoration_styles(buffer, visible_start..visible_end);
        let style_at = |offset: usize| {
            let at = |styles: &[Option<Style>]| {
                styles
                    .get(offset.checked_sub(visible_start)?)
                    .copied()
                    .flatten()
            };
            let mut style = syntax.as_deref().and_then(at).unwrap_or(normal);
            if let Some(decoration) = at(&decorations) {
                style = style.patch(decoration);
            }
            if selected(offset) {
                Style {
                    bg: selection_bg,
                    ..style
                }
            } else {
                style
            }
        };
        let cursor_pos = view.cursor(text);
        let left = view.left_col;
        let mut cursor = None;

        for row in 0..rows {
            let line_idx = view.top_line + row as usize;
            if line_idx >= text.len_lines() {
                break;
            }
            let line: Cow<str> = text.line(line_idx).into();
            let line = line.strip_suffix('\n').unwrap_or(&line);
            let line_start = text.line_to_byte(line_idx);
            let mut offset = line_start;
            // Display column within the line; the screen shows `left..left + width`.
            let mut column = 0u32;
            for grapheme in graphemes(line) {
                if column >= left + width {
                    break;
                }
                let next = if grapheme == "\t" {
                    column + tab_width - column % tab_width
                } else {
                    column + u32::from(display_width(grapheme))
                };
                if offset == cursor_pos && column >= left {
                    cursor = Some(((column - left) as u16, row));
                }
                let style = style_at(offset);
                if next > left {
                    let x = column.saturating_sub(left) as u16;
                    // Tabs and graphemes cut by the left edge show as blanks.
                    if grapheme == "\t" || column < left {
                        let end = ((next - left).min(width)) as u16;
                        let mut x = x;
                        while x < end {
                            let (gx, gy) = at(x, row);
                            x = grid.put_grapheme(gx, gy, " ", style) - rect.x;
                        }
                    } else {
                        let (gx, gy) = at(x, row);
                        grid.put_grapheme(gx, gy, grapheme, style);
                    }
                }
                column = next;
                offset += grapheme.len();
            }
            // The cursor, and a selected line break, show just past the text.
            if column >= left && column < left + width {
                let x = (column - left) as u16;
                if offset == cursor_pos {
                    cursor = Some((x, row));
                }
                if offset < text.len_bytes() && selected(offset) {
                    let (gx, gy) = at(x, row);
                    grid.put_grapheme(gx, gy, " ", style_at(offset));
                }
            }
            // Notes follow the text, when all of the line is drawn.
            if offset == line_start + line.len() {
                let notes = &buffer.notes;
                let first = notes.partition_point(|note| note.at < line_start);
                let mut x = (column + 2).saturating_sub(left);
                let end = rect.x + rect.width;
                for note in notes[first..].iter().take_while(|note| note.at <= offset) {
                    if x >= width {
                        break;
                    }
                    let style = self.state().theme.style(&note.style).unwrap_or(normal);
                    let text = note.text.lines().next().unwrap_or_default();
                    let (gx, gy) = at(x as u16, row);
                    x = u32::from(put_clipped(grid, gx, gy, text, style, end) - rect.x) + 2;
                }
            }
        }

        cursor.filter(|_| focused).map(|(x, y)| Cursor {
            x: rect.x + x,
            y: rect.y + y,
            shape: view.cursor_shape,
        })
    }

    /// The decoration style of each byte in `range` of `buffer`.
    fn decoration_styles(
        &self,
        buffer: &Buffer,
        range: std::ops::Range<usize>,
    ) -> Vec<Option<Style>> {
        let theme = &self.state().theme;
        let mut styles: Vec<Option<Style>> = vec![None; range.len()];
        let decorations = &buffer.decorations;
        let first = decorations.partition_point(|d| d.range.start < range.start);
        // Decorations starting before the range may still reach into it;
        // they are few, so scan them all.
        let reaching = decorations[..first]
            .iter()
            .filter(|d| d.range.end > range.start);
        let starting = decorations[first..]
            .iter()
            .take_while(|d| d.range.start < range.end);
        for decoration in reaching.chain(starting) {
            let Some(style) = theme.style(&decoration.style) else {
                continue;
            };
            let start = decoration.range.start.max(range.start) - range.start;
            let end = decoration.range.end.min(range.end) - range.start;
            for slot in &mut styles[start..end] {
                *slot = Some(slot.map_or(style, |below| below.patch(style)));
            }
        }
        styles
    }

    /// Draws popups over the top `rows` rows, oldest first.
    fn render_popups(&self, grid: &mut Grid, rows: u16) {
        let state = self.state();
        let width = grid.width();
        let base = state.theme.style("ui.popup").unwrap_or_default();
        for popup in &state.popups {
            if popup.lines.is_empty() || rows == 0 {
                continue;
            }
            // One blank column on each side.
            let content = popup.lines.iter().map(|l| line_width(l)).max().unwrap_or(0);
            let w = content.saturating_add(2).min(width);
            let h = popup.lines.len().min(rows as usize) as u16;
            let (x, y) = match popup.anchor {
                PopupAnchor::Corner => (width - w, rows - h),
                PopupAnchor::Position { buffer, offset } => {
                    if buffer != state.view.buffer {
                        continue;
                    }
                    let Some((column, row)) = self.screen_position(offset, rows) else {
                        continue;
                    };
                    let y = if row + 1 + h <= rows || row < h {
                        row + 1
                    } else {
                        row - h
                    };
                    (column.min(width - w), y)
                }
            };
            for (i, line) in popup.lines.iter().enumerate() {
                let y = y + i as u16;
                if y >= rows {
                    break;
                }
                let mut fill = x;
                while fill < x + w {
                    fill = grid.put_grapheme(fill, y, " ", base);
                }
                if w > 2 {
                    put_line(grid, &state.theme, x + 1, y, line, base);
                }
            }
        }
    }

    /// Where `offset` of the focused view's buffer is drawn, if it is in
    /// that view, with the views in the top `rows` rows.
    fn screen_position(&self, offset: usize, rows: u16) -> Option<(u16, u16)> {
        let text = self.buffer().text();
        let view = self.view();
        let rect = self.state().focused_rect(rows);
        // On a char boundary, in case the text changed under the offset.
        let offset = text.char_to_byte(text.byte_to_char(offset.min(text.len_bytes())));
        let row = text.byte_to_line(offset).checked_sub(view.top_line)?;
        let column = layout::column_of(text, offset, self.state().tab_width(view.buffer))
            .checked_sub(view.left_col)?;
        let inside = row < rect.height as usize && column < u32::from(rect.width);
        inside.then_some((rect.x + column as u16, rect.y + row as u16))
    }

    fn render_status(&self, grid: &mut Grid, y: u16) {
        let style = Style {
            reverse: true,
            ..Style::default()
        };
        grid.fill_row(0, y, style);

        let state = self.state();
        let items = |side| {
            let mut items: Vec<_> = state.status.iter().filter(|i| i.side == side).collect();
            items.sort_by_key(|item| item.priority);
            items
        };

        let buffer = self.buffer();
        let text = buffer.text();
        let cursor = self.view().cursor(text);
        let line = text.byte_to_line(cursor);
        let before_cursor: Cow<str> = text.byte_slice(text.line_to_byte(line)..cursor).into();
        let column = graphemes(&before_cursor).count();
        let mut right: StyledLine = Vec::new();
        for item in items(Side::Right) {
            right.extend(item.content.iter().cloned());
            right.push(plain(" "));
        }
        if let Some(hint) = self.key_hint() {
            right.push(plain(&format!("{hint}  ")));
        }
        right.push(plain(&format!("{}:{} ", line + 1, column + 1)));
        let right_width: u16 = right
            .iter()
            .flat_map(|span| graphemes(&span.text))
            .map(display_width)
            .sum();
        let right_start = grid.width().saturating_sub(right_width);

        let mut x = 0;
        for item in items(Side::Left) {
            x = put_line(grid, &state.theme, x, y, &item.content, style);
        }
        let name = buffer.name();
        let modified = if buffer.is_modified() { " [+]" } else { "" };
        // On one line, whatever it holds.
        let message = self.message().map(|message| {
            let lines: Vec<&str> = message
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .collect();
            format!("  {}", lines.join(" "))
        });
        // Shorten a long path from the left so the modified mark stays
        // visible, and further for a message, which matters more than the
        // path's start.
        let room_before = |end: u16| end.saturating_sub(x + 2 + modified.len() as u16);
        let mut room = room_before(right_start);
        if let Some(message) = &message {
            let width = graphemes(message).map(display_width).sum();
            room = room.min(room_before(grid.width().saturating_sub(width)).max(MIN_PATH_WIDTH));
        }
        x = grid.put_str(
            x,
            y,
            &format!(" {}{modified}", truncate_left(&name, room)),
            style,
        );
        if let Some(message) = &message {
            x = grid.put_str(x, y, message, style);
        }
        if message.is_none() || right_start >= x {
            put_line(grid, &state.theme, right_start, y, &right, style);
        }
    }
}

/// The columns a path keeps beside a message, for its file name.
const MIN_PATH_WIDTH: u16 = 20;

/// `text` cut to `width` columns, keeping its end: "…/src/lib.rs".
fn truncate_left(text: &str, width: u16) -> Cow<'_, str> {
    let total: u16 = graphemes(text).map(display_width).sum();
    if total <= width {
        return Cow::Borrowed(text);
    }
    let mut kept = 1; // for the ellipsis
    let mut start = text.len();
    for (i, grapheme) in text.grapheme_indices(true).rev() {
        let w = display_width(grapheme);
        if kept + w > width {
            break;
        }
        kept += w;
        start = i;
    }
    Cow::Owned(format!("…{}", &text[start..]))
}

fn plain(text: &str) -> Span {
    Span {
        text: text.into(),
        style: String::new(),
    }
}

/// Puts spans from `x`, each styled by the theme over `base`. Returns the
/// column after the last grapheme put.
fn put_line(grid: &mut Grid, theme: &Theme, mut x: u16, y: u16, line: &[Span], base: Style) -> u16 {
    for span in line {
        let style = theme
            .style(&span.style)
            .map_or(base, |style| base.patch(style));
        x = grid.put_str(x, y, &span.text, style);
    }
    x
}

/// Puts `text` from `x`, stopping before column `end`. Returns the column
/// after the last grapheme put.
pub(crate) fn put_clipped(
    grid: &mut Grid,
    mut x: u16,
    y: u16,
    text: &str,
    style: Style,
    end: u16,
) -> u16 {
    for grapheme in graphemes(text) {
        if x + display_width(grapheme) > end {
            break;
        }
        x = grid.put_grapheme(x, y, grapheme, style);
    }
    x
}

fn line_width(line: &[Span]) -> u16 {
    line.iter()
        .flat_map(|span| graphemes(&span.text))
        .map(display_width)
        .fold(0u16, u16::saturating_add)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::CursorShape;
    use crate::selection::Selection;

    fn render(editor: &Editor) -> (Vec<String>, Option<Cursor>) {
        let mut grid = Grid::default();
        let cursor = editor.render(&mut grid);
        let rows = (0..grid.height()).map(|y| grid.row_text(y)).collect();
        (rows, cursor)
    }

    #[test]
    fn draws_lines_and_status() {
        let mut editor = Editor::with_text("fn main() {\n\tnib();\n}\n");
        editor.resize(30, 5);
        let (rows, cursor) = render(&editor);
        assert_eq!(rows[0], format!("{:30}", "fn main() {"));
        assert_eq!(rows[1], format!("{:30}", "    nib();"));
        assert_eq!(rows[2], format!("{:30}", "}"));
        assert_eq!(rows[3], " ".repeat(30));
        assert_eq!(rows[4], " [scratch]  Ctrl-g: menu  1:1 ");
        assert_eq!(
            cursor,
            Some(Cursor {
                x: 0,
                y: 0,
                shape: CursorShape::Block
            })
        );
    }

    #[test]
    fn tabs_align_to_tab_stops() {
        let mut editor = Editor::with_text("ab\tc\n\t\td");
        editor.resize(12, 3);
        let (rows, _) = render(&editor);
        assert_eq!(rows[0], "ab  c       ");
        assert_eq!(rows[1], "        d   ");
    }

    #[test]
    fn tab_width_comes_from_config() {
        let mut editor = Editor::with_text("\tx");
        editor.apply_config(crate::Config::parse("[core]\ntab-width = 8").unwrap());
        editor.resize(12, 2);
        let (rows, _) = render(&editor);
        assert_eq!(rows[0], "        x   ");
    }

    #[test]
    fn long_lines_are_cut() {
        let mut editor = Editor::with_text("0123456789");
        editor.resize(6, 2);
        let (rows, _) = render(&editor);
        assert_eq!(rows[0], "012345");
    }

    #[test]
    fn cursor_follows_graphemes_and_tabs() {
        let mut editor = Editor::with_text("\tあx\n");
        editor.resize(20, 3);
        let text = editor.buffer().text().clone();
        // A forward range over "あ": the cursor sits on "あ".
        editor.view_mut().selection =
            Selection::new(vec![crate::Range::new(1, 4)], 0, &text).unwrap();
        let (rows, cursor) = render(&editor);
        let cursor = cursor.unwrap();
        assert_eq!((cursor.x, cursor.y), (4, 0));
        assert!(rows[2].ends_with("1:2 "));

        // At the end of the line.
        editor.view_mut().selection = Selection::point(5);
        let (_, cursor) = render(&editor);
        assert_eq!(cursor.map(|c| (c.x, c.y)), Some((7, 0)));
    }

    #[test]
    fn a_message_of_many_lines_shows_on_one() {
        let mut editor = Editor::with_text("a");
        editor.resize(60, 3);
        editor.show_message("broken:\n  |\n5 | x\n\nfix it");
        let (rows, _) = render(&editor);
        assert!(rows[2].contains("broken: | 5 | x fix it"), "{rows:#?}");
    }

    #[test]
    fn the_menu_is_a_box_over_the_middle_with_the_cursor_in_its_input() {
        let mut editor = Editor::with_text("a");
        editor.resize(80, 20);
        editor.handle_key(crate::KeyEvent::ctrl('g'));
        editor.handle_key(crate::KeyEvent::new(crate::KeyCode::Char('r')));
        let (rows, cursor) = render(&editor);
        // 72 by 16, from (4, 2).
        assert!(rows[2].starts_with("    ╭─ nib ─"), "{rows:#?}");
        assert!(rows[3].starts_with("    │ > r "), "{rows:#?}");
        assert!(rows[3].contains("3/6 │"), "{rows:#?}");
        assert!(rows[4].contains("┬"), "{rows:#?}");
        // Starting with it first, the rest in list order.
        assert!(rows[5].contains("│ Reload the settings "), "{rows:#?}");
        assert!(rows[6].contains("│ Restart all plugins "), "{rows:#?}");
        assert!(
            rows[7].contains("│ Open the settings directory "),
            "{rows:#?}"
        );
        // The details of the one under the cursor.
        assert!(rows[5].contains("Read config.toml and the"), "{rows:#?}");
        assert!(
            rows[17].contains("╰─ ↑↓ move · enter choose · esc back ─"),
            "{rows:#?}"
        );
        // The status line is as it always is.
        assert!(rows[19].contains("[scratch]"), "{rows:#?}");
        let cursor = cursor.unwrap();
        assert_eq!((cursor.x, cursor.y, cursor.shape), (9, 3, CursorShape::Bar));
    }

    fn span(text: &str, style: &str) -> Span {
        Span {
            text: text.into(),
            style: style.into(),
        }
    }

    #[test]
    fn panels_sit_above_the_status_line_and_take_the_cursor() {
        let mut editor = Editor::with_text("a\nb\nc\nd");
        editor.resize(20, 5);
        editor.state_mut().panels.push(crate::ui::Panel {
            id: 1,
            owner: 0,
            lines: vec![vec![span(":", ""), span("wq", "")]],
            cursor: Some((0, 3)),
        });
        let (rows, cursor) = render(&editor);
        assert_eq!(editor.text_rows(), 3);
        assert_eq!(rows[2].trim_end(), "c");
        assert_eq!(rows[3].trim_end(), ":wq");
        assert_eq!(
            cursor,
            Some(Cursor {
                x: 3,
                y: 3,
                shape: CursorShape::Bar
            })
        );
    }

    #[test]
    fn status_items_are_ordered_by_priority() {
        let mut editor = Editor::with_text("a");
        editor.resize(40, 2);
        let item = |id: &str, side, priority, text: &str| crate::ui::StatusItem {
            owner: 0,
            id: id.into(),
            side,
            priority,
            content: vec![span(text, "ui.mode.normal")],
        };
        let status = &mut editor.state_mut().status;
        status.push(item("b", Side::Left, 1, "B"));
        status.push(item("a", Side::Left, 0, "A"));
        status.push(item("r", Side::Right, 0, "R"));
        let (rows, _) = render(&editor);
        assert_eq!(rows[1], "AB [scratch]        R Ctrl-g: menu  1:1 ");
    }

    #[test]
    fn scrolls_sideways_to_the_cursor() {
        let mut editor = Editor::with_text("0123456789abcdef\nxy");
        editor.resize(8, 3);
        editor.view_mut().selection = Selection::point(12);
        editor.handle_key(crate::KeyEvent::new(crate::KeyCode::Escape));
        let (rows, cursor) = render(&editor);
        assert_eq!(editor.view().left_col, 5);
        assert_eq!(rows[0], "56789abc");
        assert_eq!(rows[1], "        ");
        assert_eq!(cursor.map(|c| (c.x, c.y)), Some((7, 0)));
    }

    #[test]
    fn selections_are_highlighted_including_line_breaks() {
        let mut editor = Editor::with_text("ab\ncd");
        editor.resize(6, 3);
        let text = editor.buffer().text().clone();
        editor.view_mut().selection =
            Selection::new(vec![crate::Range::new(1, 4)], 0, &text).unwrap();
        let mut grid = Grid::default();
        editor.render(&mut grid);
        let selected =
            |x, y| grid.cell(x, y).style == Theme::default().style("ui.selection").unwrap();
        assert!(!selected(0, 0));
        assert!(selected(1, 0));
        assert!(selected(2, 0), "the line break");
        assert!(selected(0, 1));
        assert!(!selected(1, 1));
    }

    #[test]
    fn long_paths_keep_the_modified_mark() {
        assert_eq!(truncate_left("a/b/c.rs", 20), "a/b/c.rs");
        assert_eq!(truncate_left("very/long/path/c.rs", 8), "…th/c.rs");
        let path =
            std::env::temp_dir().join(format!("nib-{}-{}.txt", std::process::id(), "x".repeat(60)));
        let mut editor = Editor::default();
        editor.open(&path).unwrap();
        let buffer = &mut editor.state_mut().buffers[0];
        let version = buffer.version();
        buffer
            .apply(
                version,
                vec![crate::Edit::insert(0, "a")],
                &Selection::point(0),
                None,
                crate::UndoMode::NewStep,
            )
            .unwrap();
        editor.resize(40, 2);
        let (rows, _) = render(&editor);
        assert!(rows[1].contains("x.txt [+]"), "{}", rows[1]);
        assert!(rows[1].starts_with(" …"), "{}", rows[1]);

        editor.resize(60, 2);
        editor.show_message("settings reloaded");
        let (rows, _) = render(&editor);
        assert!(
            rows[1].contains("x.txt [+]  settings reloaded"),
            "{}",
            rows[1]
        );
    }

    #[test]
    fn tiny_sizes() {
        let mut editor = Editor::with_text("abc");
        editor.resize(0, 0);
        assert_eq!(render(&editor).1, None);
        editor.resize(2, 1);
        assert_eq!(render(&editor).0, vec!["ab".to_string()]);
    }

    #[test]
    fn decorations_go_over_syntax_and_under_the_selection() {
        let mut editor = Editor::with_text("abc");
        editor.resize(6, 2);
        let state = editor.state_mut();
        let buffer = state.view.buffer;
        state.buffers[buffer].set_decorations(1, "x", [(0..2, "ui.cursor.match".into())]);
        let text = editor.buffer().text().clone();
        editor.view_mut().selection =
            Selection::new(vec![crate::Range::new(0, 1)], 0, &text).unwrap();
        let mut grid = Grid::default();
        editor.render(&mut grid);
        let matched = Theme::default().style("ui.cursor.match").unwrap();
        assert_eq!(
            grid.cell(0, 0).style,
            Theme::default()
                .style("ui.selection")
                .unwrap()
                .patch(matched)
        );
        assert_eq!(grid.cell(1, 0).style, matched);
        assert_eq!(grid.cell(2, 0).style, Style::default());
    }

    fn popup(anchor: PopupAnchor, lines: &[&str]) -> crate::ui::Popup {
        crate::ui::Popup {
            id: 1,
            owner: 0,
            anchor,
            lines: lines.iter().map(|l| vec![span(l, "")]).collect(),
        }
    }

    #[test]
    fn popups_sit_at_their_anchor() {
        let mut editor = Editor::with_text("a\nbcd\nc\nd\ne");
        editor.resize(12, 6);
        let at = |offset| PopupAnchor::Position { buffer: 0, offset };
        // Below the line of the offset, from its column.
        editor.state_mut().popups = vec![popup(at(3), &["hi"])];
        let (rows, cursor) = render(&editor);
        assert_eq!(rows[2], "c hi        ");
        assert_eq!(
            cursor.map(|c| (c.x, c.y)),
            Some((0, 0)),
            "popups do not take the cursor"
        );
        // Above it when there is no room below.
        editor.state_mut().popups = vec![popup(at(10), &["one", "two"])];
        let (rows, _) = render(&editor);
        assert_eq!(
            &rows[2..5],
            [" one        ", " two        ", "e           "]
        );
        // Moved left to fit, and in the corner.
        editor.state_mut().popups = vec![
            popup(at(2), &["wide popup"]),
            popup(PopupAnchor::Corner, &["k"]),
        ];
        let (rows, _) = render(&editor);
        assert_eq!(rows[2], " wide popup ");
        assert_eq!(rows[4], "e         k ");
        // Hidden while the offset is off screen.
        editor.state_mut().popups = vec![popup(at(9), &["x"])];
        editor.view_mut().top_line = 4;
        let (rows, _) = render(&editor);
        assert!(!rows.concat().contains('x'));
    }

    #[test]
    fn notes_follow_their_line() {
        let mut editor = Editor::with_text("ab\ncd\n");
        editor.resize(12, 4);
        let state = editor.state_mut();
        let buffer = state.view.buffer;
        state.buffers[buffer].set_notes(
            1,
            "x",
            [
                (4, "two".to_string(), String::new()),
                (0, "one".to_string(), String::new()),
                (1, "more text".to_string(), String::new()),
            ],
        );
        let (rows, _) = render(&editor);
        assert_eq!(rows[0], "ab  one  mor");
        assert_eq!(rows[1], "cd  two     ");
    }
}
