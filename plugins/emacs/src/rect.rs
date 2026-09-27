//! Rectangles: the columns between the mark and the point, on the lines
//! between them.

use base_kit::doc::Doc;
use base_kit::edit::{indent_unit, range};
use nib_plugin::nib::plugin::editor::View;
use nib_plugin::nib::plugin::types::{Edit, SelRange};

use crate::motion::{self, at_column, column};
use crate::{Emacs, Kill};

/// Lines `first..=last`, columns `left..right`.
#[derive(Clone, Copy)]
struct Rect {
    first: u64,
    last: u64,
    left: u32,
    right: u32,
}

/// One line's part of a rectangle: where it starts and ends, and the
/// columns those are at.
struct Part {
    start: u64,
    end: u64,
    start_col: u32,
    end_col: u32,
}

fn rect(doc: &Doc, mark: u64, here: u64) -> Rect {
    let (a, b) = (doc.line_of(mark), doc.line_of(here));
    let (c, d) = (column(doc, mark), column(doc, here));
    Rect {
        first: a.min(b),
        last: a.max(b),
        left: c.min(d),
        right: c.max(d),
    }
}

fn part(doc: &Doc, line: u64, rect: Rect) -> Part {
    let line_start = doc.line_start(line);
    let (start, start_col) = at_column(doc, line_start, rect.left);
    let (end, end_col) = at_column(doc, line_start, rect.right);
    Part {
        start,
        end,
        start_col,
        end_col,
    }
}

/// The rectangle's text, each line padded with spaces to its width.
fn extract(doc: &Doc, rect: Rect) -> Vec<String> {
    (rect.first..=rect.last)
        .map(|line| {
            let p = part(doc, line, rect);
            let mut text = doc.slice(p.start, p.end);
            let width = p.end_col.max(rect.left) - p.start_col.max(rect.left);
            let wanted = rect.right - rect.left;
            if width < wanted {
                text.push_str(&" ".repeat((wanted - width) as usize));
            }
            text
        })
        .collect()
}

/// The selection that draws the rectangle: a range on each line, the
/// point's line first among equals as the primary one.
pub fn ranges(view: &View, mark: u64, here: u64) -> (Vec<SelRange>, u32) {
    let doc = Doc::new(view.buffer());
    let r = rect(&doc, mark, here);
    let rightward = column(&doc, here) >= column(&doc, mark);
    let ranges = (r.first..=r.last)
        .map(|line| {
            let p = part(&doc, line, r);
            if rightward {
                range(p.start, p.end)
            } else {
                range(p.end, p.start)
            }
        })
        .collect();
    let primary = (doc.line_of(here) - r.first) as u32;
    (ranges, primary)
}

impl Emacs {
    fn rect(&self, view: &View) -> Result<(Doc, Rect), String> {
        let doc = Doc::new(view.buffer());
        let mark = self
            .mark(&view.buffer())
            .ok_or("The mark is not set now, so there is no region")?;
        let r = rect(&doc, mark, self.point(view));
        Ok((doc, r))
    }

    /// Applies edits, one or none per line, as one undo step, and puts the
    /// point on line `line` at column `col`.
    fn edit_lines(&mut self, view: &View, edits: Vec<Edit>, line: u64, col: u32) {
        let edits: Vec<Edit> = edits
            .into_iter()
            .filter(|e| e.start != e.end || !e.text.is_empty())
            .collect();
        let doc = Doc::new(view.buffer());
        if edits.is_empty() {
            self.deactivate = true;
            return;
        }
        // Where the point goes, in the text before the edits, then moved
        // by the edits before it.
        let start = doc.line_start(line);
        let shift: i64 = edits
            .iter()
            .filter(|e| e.end <= start)
            .map(|e| e.text.len() as i64 - (e.end - e.start) as i64)
            .sum();
        self.edit(view, edits, 0);
        let doc = Doc::new(view.buffer());
        let start = (start as i64 + shift) as u64;
        let (at, _) = at_column(&doc, start.min(doc.len), col);
        self.goto(view, at);
    }

    pub fn rectangle_command(&mut self, view: &View, name: &str) -> Result<(), String> {
        let (doc, r) = self.rect(view)?;
        let here_line = doc.line_of(self.point(view));
        let parts: Vec<Part> = (r.first..=r.last).map(|l| part(&doc, l, r)).collect();
        let deletions = || {
            parts
                .iter()
                .map(|p| Edit {
                    start: p.start,
                    end: p.end,
                    text: String::new(),
                })
                .collect::<Vec<_>>()
        };
        match name {
            "kill-rectangle" | "delete-rectangle" | "kill-rectangle-to-ring" => {
                let lines = extract(&doc, r);
                match name {
                    "kill-rectangle" => self.killed_rectangle = lines,
                    "kill-rectangle-to-ring" => self.kill_new(Kill {
                        text: lines.join("\n"),
                        rect: Some(lines),
                    }),
                    _ => {}
                }
                self.edit_lines(view, deletions(), here_line, r.left);
            }
            "copy-rectangle-as-kill" => {
                self.killed_rectangle = extract(&doc, r);
                self.deactivate = true;
            }
            "copy-rectangle-to-ring" => {
                let lines = extract(&doc, r);
                self.kill_new(Kill {
                    text: lines.join("\n"),
                    rect: Some(lines),
                });
                self.deactivate = true;
            }
            "yank-rectangle" => {
                let lines = self.killed_rectangle.clone();
                if lines.is_empty() {
                    return Err("No rectangle has been killed".into());
                }
                let here = self.point(view);
                self.push_mark(view, here);
                self.insert_rectangle(view, here, &lines);
            }
            "open-rectangle" => {
                let tabs = indent_unit() == "\t";
                let tab = motion::tab_width();
                let edits = parts
                    .iter()
                    .filter(|p| p.end_col > r.left || doc.line_end(p.start) > p.start)
                    .map(|p| Edit {
                        start: p.start,
                        end: p.start,
                        text: motion::spaces(r.left, r.right, tabs, tab),
                    })
                    .collect();
                self.edit_lines(view, edits, r.first, r.left);
            }
            "clear-rectangle" => {
                let edits = parts
                    .iter()
                    .map(|p| {
                        let more = doc.line_end(p.end) > p.end;
                        let width = if more {
                            r.right.saturating_sub(p.start_col)
                        } else {
                            0
                        };
                        Edit {
                            start: p.start,
                            end: p.end,
                            text: " ".repeat(width as usize),
                        }
                    })
                    .collect();
                self.edit_lines(view, edits, here_line, r.left);
            }
            "rectangle-number-lines" => {
                let count = r.last - r.first + 1;
                let width = count.to_string().len();
                let edits = parts
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        let pad = " ".repeat(r.left.saturating_sub(p.start_col) as usize);
                        Edit {
                            start: p.start,
                            end: p.start,
                            text: format!("{pad}{:>width$} ", i + 1),
                        }
                    })
                    .collect();
                self.edit_lines(view, edits, here_line, r.left);
            }
            _ => return self.run_file_command(view, name, crate::Arg::None),
        }
        Ok(())
    }

    /// The rectangle between `mark` and the point, as lines.
    pub fn rectangle_text(&self, view: &View, mark: u64) -> Vec<String> {
        let doc = Doc::new(view.buffer());
        extract(&doc, rect(&doc, mark, self.point(view)))
    }

    /// `C-x r t`: each line's part becomes `text`.
    pub fn string_rectangle(&mut self, view: &View, text: &str) -> Result<(), String> {
        let (doc, r) = self.rect(view)?;
        let here_line = doc.line_of(self.point(view));
        let edits = (r.first..=r.last)
            .map(|line| {
                let p = part(&doc, line, r);
                let pad = " ".repeat(r.left.saturating_sub(p.start_col) as usize);
                Edit {
                    start: p.start,
                    end: p.end,
                    text: format!("{pad}{text}"),
                }
            })
            .collect();
        let width = text.chars().count() as u32;
        self.edit_lines(view, edits, here_line, r.left + width);
        Ok(())
    }

    /// Inserts `lines` as a rectangle with its corner at `at`, adding lines
    /// at the end of the buffer as needed, and returns the end of the last
    /// line inserted.
    pub fn insert_rectangle(&mut self, view: &View, at: u64, lines: &[String]) -> u64 {
        let doc = Doc::new(view.buffer());
        let col = column(&doc, at);
        let first = doc.line_of(at);
        let count = doc.line_count();
        let mut edits = Vec::new();
        let mut appended = String::new();
        for (i, text) in lines.iter().enumerate() {
            let line = first + i as u64;
            if line < count {
                let start = doc.line_start(line);
                let (pos, reached) = at_column(&doc, start, col);
                let pad = " ".repeat(col.saturating_sub(reached) as usize);
                edits.push(Edit {
                    start: pos,
                    end: pos,
                    text: format!("{pad}{text}"),
                });
            } else {
                appended.push('\n');
                appended.push_str(&" ".repeat(col as usize));
                appended.push_str(text);
            }
        }
        if !appended.is_empty() {
            edits.push(Edit {
                start: doc.len,
                end: doc.len,
                text: appended,
            });
        }
        let inserted: u64 = edits.iter().map(|e| e.text.len() as u64).sum();
        let last = edits.last().map_or(at, |e| e.start);
        let end = last + inserted;
        self.edit(view, edits, end);
        end
    }
}
