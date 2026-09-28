use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use ropey::Rope;

use crate::Error;
use crate::change::Assoc;
use crate::change::{ChangeSet, Edit};
use crate::config::Indent;
use crate::events::{TextChange, text_changes};
use crate::grapheme;
use crate::history::{History, UndoMode};
use crate::plugin::PluginId;
use crate::search;
use crate::selection::{Range, Selection};
use crate::syntax::BufferSyntax;
use crate::ui::{Decoration, Note};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    Crlf,
}

/// Editing settings a plugin set for one buffer, over config.toml's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Overrides {
    pub tab_width: Option<(PluginId, u16)>,
    pub indent: Option<(PluginId, Indent)>,
}

/// What a change did to a buffer.
#[derive(Debug)]
pub struct Change {
    /// The change sets applied to the text, in order.
    pub changes: Vec<ChangeSet>,
    /// The selection of the view that made the change, after the change.
    pub selection: Selection,
}

impl Change {
    /// Where the text changed first, in the text after the change: the
    /// edit that comes first, as it ended up.
    pub fn first_range(&self) -> Option<std::ops::Range<usize>> {
        let mut first: Option<std::ops::Range<usize>> = None;
        for (i, set) in self.changes.iter().enumerate() {
            let mut delta = 0isize;
            for edit in set.edits() {
                let start = edit.start.saturating_add_signed(delta);
                let end = start + edit.text.len();
                delta += edit.text.len() as isize - (edit.end - edit.start) as isize;
                let later = &self.changes[i + 1..];
                let start = later
                    .iter()
                    .fold(start, |pos, set| set.map_pos(pos, Assoc::Before));
                let end = later
                    .iter()
                    .fold(end, |pos, set| set.map_pos(pos, Assoc::After));
                if first.as_ref().is_none_or(|f| start < f.start) {
                    first = Some(start..end);
                }
            }
        }
        first
    }
}

/// A text buffer. Line endings are normalized to LF in memory and restored
/// on save, so offsets never point between `\r` and `\n`.
pub struct Buffer {
    text: Rope,
    version: u64,
    line_ending: LineEnding,
    path: Option<PathBuf>,
    history: History,
    saved_state: u64,
    pub(crate) syntax: Option<BufferSyntax>,
    /// Sorted by start, so drawing can skip to the visible ones.
    pub(crate) decorations: Vec<Decoration>,
    /// Sorted by position.
    pub(crate) notes: Vec<Note>,
    /// Positions plugins keep, such as vim's marks, by owner and namespace.
    marks: Vec<Marks>,
    /// Settings plugins changed for this buffer alone, with who changed them.
    pub(crate) overrides: Overrides,
    /// Changes not yet turned into events: the version after each, and
    /// what it did.
    pub(crate) change_log: Vec<(u64, Vec<TextChange>)>,
    /// Closed with `buffer.close`. Buffers keep their place in the list,
    /// empty, so indices, which plugins hold as handles, never move.
    closed: bool,
}

impl Default for Buffer {
    fn default() -> Self {
        Self::with_text("")
    }
}

impl Buffer {
    /// Creates a buffer from `text`. If the first line ends with CRLF, the
    /// buffer uses CRLF and every CRLF in `text` becomes LF.
    pub fn with_text(text: &str) -> Self {
        let line_ending = match text.find('\n') {
            Some(i) if text[..i].ends_with('\r') => LineEnding::Crlf,
            _ => LineEnding::Lf,
        };
        let text = match line_ending {
            LineEnding::Lf => Rope::from_str(text),
            LineEnding::Crlf => Rope::from_str(&text.replace("\r\n", "\n")),
        };
        Self {
            text,
            version: 0,
            line_ending,
            path: None,
            history: History::default(),
            saved_state: 0,
            syntax: None,
            decorations: Vec::new(),
            notes: Vec::new(),
            marks: Vec::new(),
            overrides: Overrides::default(),
            change_log: Vec::new(),
            closed: false,
        }
    }

    /// Opens the file at `path`. A missing file gives an empty buffer that
    /// will be created on save.
    /// What is left of a buffer after `buffer.close`.
    pub(crate) fn closed() -> Self {
        Self {
            closed: true,
            ..Self::default()
        }
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }

    pub fn open(path: impl Into<PathBuf>) -> Result<Self, Error> {
        let path = path.into();
        let mut buffer = match fs::read_to_string(&path) {
            Ok(text) => Self::with_text(&text),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(err) => return Err(err.into()),
        };
        buffer.path = Some(path);
        Ok(buffer)
    }

    /// Writes a temporary file next to the target and renames it over the
    /// target, so a crash while saving never leaves a truncated file.
    pub fn save(&mut self) -> Result<(), Error> {
        let path = self.path.as_ref().ok_or(Error::NoPath)?;
        // Write to the file a symlink points to, instead of replacing the link.
        let target = fs::canonicalize(path).unwrap_or_else(|_| path.clone());
        let name = target.file_name().unwrap_or_default().to_string_lossy();
        let tmp = target.with_file_name(format!(".{name}.nib-{}~", std::process::id()));
        let result = self.write_file(&tmp).and_then(|()| {
            if let Ok(metadata) = fs::metadata(&target) {
                fs::set_permissions(&tmp, metadata.permissions())?;
            }
            fs::rename(&tmp, &target)
        });
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result?;
        self.saved_state = self.history.state();
        Ok(())
    }

    fn write_file(&self, path: &Path) -> io::Result<()> {
        let mut out = BufWriter::new(File::create(path)?);
        match self.line_ending {
            LineEnding::Lf => self.text.write_to(&mut out)?,
            LineEnding::Crlf => {
                for chunk in self.text.chunks() {
                    out.write_all(chunk.replace('\n', "\r\n").as_bytes())?;
                }
            }
        }
        out.into_inner()
            .map_err(io::IntoInnerError::into_error)?
            .sync_all()
    }

    pub fn save_as(&mut self, path: impl Into<PathBuf>) -> Result<(), Error> {
        self.path = Some(path.into());
        self.save()
    }

    pub fn text(&self) -> &Rope {
        &self.text
    }

    /// Increases on every change, including undo and redo.
    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn len(&self) -> usize {
        self.text.len_bytes()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn line_ending(&self) -> LineEnding {
        self.line_ending
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn is_modified(&self) -> bool {
        self.history.state() != self.saved_state
    }

    pub fn slice(&self, start: usize, end: usize) -> Result<String, Error> {
        grapheme::check_position(&self.text, start)?;
        grapheme::check_position(&self.text, end)?;
        if end < start {
            return Err(Error::InvalidPosition(end));
        }
        Ok(self.text.byte_slice(start..end).to_string())
    }

    /// Text ending with a line break has an empty last line.
    pub fn line_count(&self) -> usize {
        self.text.len_lines()
    }

    pub fn line_start(&self, line: usize) -> Option<usize> {
        (line < self.line_count()).then(|| self.text.line_to_byte(line))
    }

    pub fn line_of(&self, pos: usize) -> Result<usize, Error> {
        grapheme::check_position(&self.text, pos)?;
        Ok(self.text.byte_to_line(pos))
    }

    pub fn next_grapheme(&self, pos: usize) -> Result<usize, Error> {
        grapheme::check_position(&self.text, pos)?;
        Ok(grapheme::next_boundary(&self.text, pos))
    }

    pub fn prev_grapheme(&self, pos: usize) -> Result<usize, Error> {
        grapheme::check_position(&self.text, pos)?;
        Ok(grapheme::prev_boundary(&self.text, pos))
    }

    /// See [`search::find`].
    pub fn find(
        &self,
        pattern: &str,
        start: usize,
        backward: bool,
    ) -> Result<Option<(usize, usize)>, Error> {
        search::find(&self.text, pattern, start, backward)
    }

    /// See [`search::find_all`].
    pub fn find_all(
        &self,
        pattern: &str,
        start: usize,
        end: usize,
    ) -> Result<Vec<(usize, usize)>, Error> {
        search::find_all(&self.text, pattern, start, end)
    }

    /// See [`search::find_groups`].
    pub fn find_groups(
        &self,
        pattern: &str,
        start: usize,
        backward: bool,
    ) -> Result<Option<search::Groups>, Error> {
        search::find_groups(&self.text, pattern, start, backward)
    }

    /// Applies `edits` atomically: on error, nothing changes.
    ///
    /// `selection` is the current selection of the view making the change.
    /// `after` is the new selection as ranges in the changed text and the
    /// primary index; without it, `selection` is mapped through the change.
    pub fn apply(
        &mut self,
        base_version: u64,
        edits: Vec<Edit>,
        selection: &Selection,
        after: Option<(Vec<Range>, usize)>,
        mode: UndoMode,
    ) -> Result<Change, Error> {
        if base_version != self.version {
            return Err(Error::StaleVersion {
                base: base_version,
                current: self.version,
            });
        }
        let changes = ChangeSet::new(edits, &self.text)?;
        let mut text = self.text.clone();
        let inverse = changes.apply(&mut text);
        let after = match after {
            Some((ranges, primary)) => Selection::new(ranges, primary, &text)?,
            None => selection.map(&changes, &text),
        };
        if changes.is_empty() {
            return Ok(Change {
                changes: Vec::new(),
                selection: after,
            });
        }
        if let (Some(syntax), Some(first), Some(last)) = (
            &mut self.syntax,
            changes.edits().first(),
            changes.edits().last(),
        ) {
            // One edit around all changes: tree-sitter reparses a bit more,
            // but its positions never go through intermediate texts.
            let old_end = last.end;
            let new_end = (old_end + text.len_bytes()).saturating_sub(self.text.len_bytes());
            syntax.edit(&self.text, &text, first.start, old_end, new_end);
        }
        let logged = text_changes(&self.text, std::slice::from_ref(&changes));
        self.text = text;
        self.version += 1;
        self.change_log.push((self.version, logged));
        self.map_decorations(std::slice::from_ref(&changes));
        self.history.record(
            changes.clone(),
            inverse,
            selection.clone(),
            after.clone(),
            mode,
        );
        Ok(Change {
            changes: vec![changes],
            selection: after,
        })
    }

    pub fn undo(&mut self) -> Option<Change> {
        let before = self.text.clone();
        let (changes, selection) = self.history.undo(&mut self.text)?;
        self.after_history(&before, &changes);
        Some(Change { changes, selection })
    }

    pub fn redo(&mut self) -> Option<Change> {
        let before = self.text.clone();
        let (changes, selection) = self.history.redo(&mut self.text)?;
        self.after_history(&before, &changes);
        Some(Change { changes, selection })
    }

    /// Records an undo or redo that turned `before` into the text now.
    fn after_history(&mut self, before: &Rope, changes: &[ChangeSet]) {
        self.version += 1;
        self.change_log
            .push((self.version, text_changes(before, changes)));
        self.map_decorations(changes);
        if let Some(syntax) = &mut self.syntax {
            // The steps went through texts of their own; one edit around
            // what differs keeps the tree, so colors stay while it is
            // parsed again.
            let (start, old_end, new_end) = differing(before, &self.text);
            syntax.edit(before, &self.text, start, old_end, new_end);
        }
    }
}

/// Where `after` differs from `before`: the start, and the ends in each,
/// between the parts at their starts and ends that are the same.
fn differing(before: &Rope, after: &Rope) -> (usize, usize, usize) {
    let prefix = before
        .bytes()
        .zip(after.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    let most = before.len_bytes().min(after.len_bytes()) - prefix;
    let suffix = before
        .bytes_at(before.len_bytes())
        .reversed()
        .zip(after.bytes_at(after.len_bytes()).reversed())
        .take(most)
        .take_while(|(a, b)| a == b)
        .count();
    (
        prefix,
        before.len_bytes() - suffix,
        after.len_bytes() - suffix,
    )
}

impl Buffer {
    /// Replaces the decorations `owner` has in `namespace`. Ranges are cut
    /// to the text, and empty ones dropped.
    pub(crate) fn set_decorations(
        &mut self,
        owner: PluginId,
        namespace: &str,
        decorations: impl IntoIterator<Item = (std::ops::Range<usize>, String)>,
    ) {
        self.decorations
            .retain(|d| !(d.owner == owner && d.namespace == namespace));
        let len = self.len();
        self.decorations.extend(
            decorations
                .into_iter()
                .map(|(range, style)| Decoration {
                    owner,
                    namespace: namespace.to_string(),
                    range: range.start.min(len)..range.end.min(len),
                    style,
                })
                .filter(|d| d.range.start < d.range.end),
        );
        self.decorations.sort_by_key(|d| d.range.start);
    }

    pub(crate) fn remove_decorations(&mut self, owner: PluginId) {
        self.decorations.retain(|d| d.owner != owner);
        self.notes.retain(|n| n.owner != owner);
        self.marks.retain(|m| m.owner != owner);
        let overrides = &mut self.overrides;
        overrides.tab_width = overrides.tab_width.filter(|(o, _)| *o != owner);
        overrides.indent = overrides.indent.filter(|(o, _)| *o != owner);
    }

    /// Replaces the notes `owner` has in `namespace`.
    pub(crate) fn set_notes(
        &mut self,
        owner: PluginId,
        namespace: &str,
        notes: impl IntoIterator<Item = (usize, String, String)>,
    ) {
        self.notes
            .retain(|n| !(n.owner == owner && n.namespace == namespace));
        let len = self.len();
        self.notes
            .extend(notes.into_iter().map(|(at, text, style)| Note {
                owner,
                namespace: namespace.to_string(),
                at: at.min(len),
                text,
                style,
            }));
        self.notes.sort_by_key(|n| n.at);
    }

    /// Replaces the positions `owner` keeps in `namespace`, in their order.
    /// Positions past the end go to the end; none removes the namespace.
    pub(crate) fn set_marks(&mut self, owner: PluginId, namespace: &str, positions: Vec<usize>) {
        self.marks
            .retain(|m| !(m.owner == owner && m.namespace == namespace));
        if positions.is_empty() {
            return;
        }
        let len = self.len();
        self.marks.push(Marks {
            owner,
            namespace: namespace.to_string(),
            positions: positions.into_iter().map(|p| p.min(len)).collect(),
        });
    }

    /// The positions `owner` keeps in `namespace`, where edits moved them.
    pub(crate) fn marks(&self, owner: PluginId, namespace: &str) -> &[usize] {
        self.marks
            .iter()
            .find(|m| m.owner == owner && m.namespace == namespace)
            .map_or(&[], |m| &m.positions)
    }

    /// Moves decorations, notes, and marks with the text. Text inserted at
    /// either end of a decoration stays outside, and decorations whose text
    /// is gone are dropped. Notes and marks stay before text inserted where
    /// they are, as Emacs's markers do, and go where their text went.
    fn map_decorations(&mut self, changes: &[ChangeSet]) {
        let positions = self.notes.iter_mut().map(|note| &mut note.at).chain(
            self.marks
                .iter_mut()
                .flat_map(|marks| marks.positions.iter_mut()),
        );
        for pos in positions {
            for change in changes {
                *pos = change.map_pos(*pos, Assoc::Before);
            }
        }
        if self.decorations.is_empty() {
            return;
        }
        for decoration in &mut self.decorations {
            for change in changes {
                let range = &mut decoration.range;
                *range = change.map_pos(range.start, Assoc::After)
                    ..change.map_pos(range.end, Assoc::Before);
                if range.end <= range.start {
                    break;
                }
            }
        }
        self.decorations.retain(|d| d.range.start < d.range.end);
        self.decorations.sort_by_key(|d| d.range.start);
    }
}

/// Positions a plugin keeps in one namespace.
struct Marks {
    owner: PluginId,
    namespace: String,
    positions: Vec<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_follow_edits_and_undo() {
        let mut buffer = Buffer::with_text("one two three");
        buffer.set_marks(1, "a", vec![4, 8, 99]);
        buffer.set_marks(2, "a", vec![0]);
        assert_eq!(buffer.marks(1, "a"), [4, 8, 13]);
        let edit = |buffer: &mut Buffer, edits| {
            let version = buffer.version();
            buffer
                .apply(
                    version,
                    edits,
                    &Selection::point(0),
                    None,
                    UndoMode::NewStep,
                )
                .unwrap();
        };
        edit(&mut buffer, vec![Edit::insert(0, "zero ")]);
        assert_eq!(buffer.marks(1, "a"), [9, 13, 18]);
        // Text typed where a mark is goes after it; deleted text takes its
        // marks to where it was.
        edit(
            &mut buffer,
            vec![Edit::insert(9, "2"), Edit::delete(12, 18)],
        );
        assert_eq!(buffer.text().to_string(), "zero one 2two");
        assert_eq!(buffer.marks(1, "a"), [9, 13, 13]);
        // Undo puts the text back after the marks, as it is inserted
        // where they are.
        buffer.undo();
        assert_eq!(buffer.marks(1, "a"), [9, 12, 12]);
        assert_eq!(buffer.marks(2, "a"), [0]);
        buffer.remove_decorations(2);
        assert!(buffer.marks(2, "a").is_empty());
        buffer.set_marks(1, "a", Vec::new());
        assert!(buffer.marks(1, "a").is_empty());
    }

    #[test]
    fn finds_what_differs() {
        let rope = Rope::from_str;
        assert_eq!(differing(&rope("abcdef"), &rope("abXYef")), (2, 4, 4));
        assert_eq!(differing(&rope("abc"), &rope("abXc")), (2, 2, 3));
        assert_eq!(differing(&rope("aaa"), &rope("aa")), (2, 3, 2));
        assert_eq!(differing(&rope("same"), &rope("same")), (4, 4, 4));
    }

    struct TempFile(PathBuf);

    impl TempFile {
        fn new(name: &str, contents: &[u8]) -> Self {
            let path = std::env::temp_dir().join(format!("nib-{}-{name}", std::process::id()));
            fs::write(&path, contents).unwrap();
            Self(path)
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    fn insert(buffer: &mut Buffer, pos: usize, text: &str, mode: UndoMode) -> Change {
        let sel = Selection::point(pos);
        buffer
            .apply(
                buffer.version(),
                vec![Edit::insert(pos, text)],
                &sel,
                None,
                mode,
            )
            .unwrap()
    }

    #[test]
    fn crlf_is_normalized_and_restored() {
        let file = TempFile::new("crlf.txt", b"one\r\ntwo\r\n");
        let mut buffer = Buffer::open(&file.0).unwrap();
        assert_eq!(buffer.line_ending(), LineEnding::Crlf);
        assert_eq!(buffer.text().to_string(), "one\ntwo\n");
        assert_eq!(buffer.line_count(), 3);
        insert(&mut buffer, 8, "three\n", UndoMode::NewStep);
        buffer.save().unwrap();
        assert_eq!(fs::read(&file.0).unwrap(), b"one\r\ntwo\r\nthree\r\n");
    }

    #[test]
    fn lone_cr_is_not_a_line_break() {
        let buffer = Buffer::with_text("a\rb\nc");
        assert_eq!(buffer.line_ending(), LineEnding::Lf);
        assert_eq!(buffer.line_count(), 2);
        assert_eq!(buffer.line_start(1), Some(4));
        assert_eq!(buffer.line_start(2), None);
    }

    #[test]
    fn missing_file_opens_empty() {
        let path = std::env::temp_dir().join(format!("nib-{}-missing.txt", std::process::id()));
        let buffer = Buffer::open(&path).unwrap();
        assert!(buffer.is_empty());
        assert_eq!(buffer.path(), Some(path.as_path()));
        assert!(!buffer.is_modified());
    }

    #[test]
    fn rejects_stale_version() {
        let mut buffer = Buffer::with_text("abc");
        insert(&mut buffer, 0, "x", UndoMode::NewStep);
        let result = buffer.apply(
            0,
            vec![Edit::insert(0, "y")],
            &Selection::point(0),
            None,
            UndoMode::NewStep,
        );
        assert!(matches!(
            result,
            Err(Error::StaleVersion {
                base: 0,
                current: 1
            })
        ));
        assert_eq!(buffer.text().to_string(), "xabc");
    }

    #[test]
    fn invalid_selection_leaves_buffer_unchanged() {
        let mut buffer = Buffer::with_text("abc");
        let result = buffer.apply(
            0,
            vec![Edit::insert(0, "あ")],
            &Selection::point(0),
            Some((vec![Range::point(1)], 0)),
            UndoMode::NewStep,
        );
        assert!(matches!(result, Err(Error::InvalidPosition(1))));
        assert_eq!(buffer.text().to_string(), "abc");
        assert_eq!(buffer.version(), 0);
        assert!(buffer.undo().is_none());
    }

    #[test]
    fn maps_selection_without_explicit_after() {
        let mut buffer = Buffer::with_text("abc");
        let change = insert(&mut buffer, 1, "xy", UndoMode::NewStep);
        assert_eq!(change.selection.primary(), Range::point(3));
    }

    #[test]
    fn merged_edits_undo_together() {
        let mut buffer = Buffer::with_text("");
        insert(&mut buffer, 0, "a", UndoMode::NewStep);
        insert(&mut buffer, 1, "b", UndoMode::Merge);
        insert(&mut buffer, 2, "c", UndoMode::Merge);
        insert(&mut buffer, 3, " d", UndoMode::NewStep);
        assert_eq!(buffer.text().to_string(), "abc d");

        let change = buffer.undo().unwrap();
        assert_eq!(buffer.text().to_string(), "abc");
        assert_eq!(change.selection.primary(), Range::point(3));
        let change = buffer.undo().unwrap();
        assert_eq!(buffer.text().to_string(), "");
        assert_eq!(change.changes.len(), 3);
        assert!(buffer.undo().is_none());

        buffer.redo().unwrap();
        assert_eq!(buffer.text().to_string(), "abc");
        assert_eq!(buffer.version(), 7);
    }

    #[test]
    fn merge_after_undo_starts_a_new_step() {
        let mut buffer = Buffer::with_text("");
        insert(&mut buffer, 0, "a", UndoMode::NewStep);
        insert(&mut buffer, 1, "b", UndoMode::NewStep);
        buffer.undo().unwrap();
        insert(&mut buffer, 1, "c", UndoMode::Merge);
        assert_eq!(buffer.text().to_string(), "ac");
        assert!(buffer.redo().is_none());
        buffer.undo().unwrap();
        assert_eq!(buffer.text().to_string(), "a");
    }

    #[test]
    fn tracks_modification_across_save_and_undo() {
        let file = TempFile::new("modified.txt", b"");
        let mut buffer = Buffer::open(&file.0).unwrap();
        insert(&mut buffer, 0, "a", UndoMode::NewStep);
        assert!(buffer.is_modified());
        buffer.save().unwrap();
        assert!(!buffer.is_modified());
        buffer.undo().unwrap();
        assert!(buffer.is_modified());
        buffer.redo().unwrap();
        assert!(!buffer.is_modified());
    }

    #[cfg(unix)]
    #[test]
    fn save_keeps_permissions_and_leaves_no_temp_file() {
        use std::os::unix::fs::PermissionsExt;

        let file = TempFile::new("perm.sh", b"echo hi\n");
        fs::set_permissions(&file.0, fs::Permissions::from_mode(0o750)).unwrap();
        let mut buffer = Buffer::open(&file.0).unwrap();
        insert(&mut buffer, 0, "#!/bin/sh\n", UndoMode::NewStep);
        buffer.save().unwrap();

        let metadata = fs::metadata(&file.0).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o750);
        assert_eq!(fs::read(&file.0).unwrap(), b"#!/bin/sh\necho hi\n");
        let name = file.0.file_name().unwrap().to_string_lossy().into_owned();
        let leftovers = fs::read_dir(file.0.parent().unwrap())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                let entry = entry.file_name().to_string_lossy().into_owned();
                entry.contains(&name) && entry.ends_with('~')
            })
            .count();
        assert_eq!(leftovers, 0);
    }

    #[cfg(unix)]
    #[test]
    fn save_writes_through_symlinks() {
        let target = TempFile::new("target.txt", b"a");
        let link =
            TempFile(std::env::temp_dir().join(format!("nib-{}-link.txt", std::process::id())));
        std::os::unix::fs::symlink(&target.0, &link.0).unwrap();

        let mut buffer = Buffer::open(&link.0).unwrap();
        insert(&mut buffer, 1, "b", UndoMode::NewStep);
        buffer.save().unwrap();

        assert!(fs::symlink_metadata(&link.0).unwrap().is_symlink());
        assert_eq!(fs::read(&target.0).unwrap(), b"ab");
    }

    #[test]
    fn empty_edits_do_not_create_undo_steps() {
        let mut buffer = Buffer::with_text("abc");
        let change = buffer
            .apply(
                0,
                vec![],
                &Selection::point(0),
                Some((vec![Range::new(0, 2)], 0)),
                UndoMode::NewStep,
            )
            .unwrap();
        assert!(change.changes.is_empty());
        assert_eq!(change.selection.primary(), Range::new(0, 2));
        assert_eq!(buffer.version(), 0);
        assert!(buffer.undo().is_none());
    }

    fn decorated(buffer: &Buffer) -> Vec<(usize, usize)> {
        buffer
            .decorations
            .iter()
            .map(|d| (d.range.start, d.range.end))
            .collect()
    }

    #[test]
    fn decorations_follow_edits() {
        let mut buffer = Buffer::with_text("one two three");
        buffer.set_decorations(1, "words", [(4..7, "a".into()), (8..13, "b".into())]);
        // Text inserted at either end stays outside.
        insert(&mut buffer, 4, "xx", UndoMode::NewStep);
        insert(&mut buffer, 9, "yy", UndoMode::NewStep);
        assert_eq!(decorated(&buffer), [(6, 9), (12, 17)]);
        assert_eq!(buffer.slice(6, 9).unwrap(), "two");
        buffer.undo();
        buffer.undo();
        assert_eq!(decorated(&buffer), [(4, 7), (8, 13)]);

        // Deleting the text drops the decoration.
        let sel = Selection::point(0);
        let edits = vec![Edit::delete(3, 8)];
        buffer
            .apply(buffer.version(), edits, &sel, None, UndoMode::NewStep)
            .unwrap();
        assert_eq!(decorated(&buffer), [(3, 8)]);
        assert_eq!(buffer.slice(3, 8).unwrap(), "three");
    }

    #[test]
    fn decorations_are_replaced_per_owner_and_namespace() {
        let mut buffer = Buffer::with_text("abcdef");
        buffer.set_decorations(1, "x", [(0..1, "a".into())]);
        buffer.set_decorations(1, "y", [(1..2, "a".into())]);
        buffer.set_decorations(2, "x", [(2..3, "a".into())]);
        buffer.set_decorations(
            1,
            "x",
            [(3..4, "a".into()), (5..99, "a".into()), (4..4, "a".into())],
        );
        assert_eq!(decorated(&buffer), [(1, 2), (2, 3), (3, 4), (5, 6)]);
        buffer.remove_decorations(1);
        assert_eq!(decorated(&buffer), [(2, 3)]);
    }

    #[test]
    fn notes_move_with_edits_and_stay() {
        let mut buffer = Buffer::with_text("one two");
        buffer.set_notes(1, "n", [(4, "x".into(), String::new())]);
        insert(&mut buffer, 0, "ab", UndoMode::NewStep);
        assert_eq!(buffer.notes[0].at, 6);
        let sel = Selection::point(0);
        let edits = vec![Edit::delete(2, 9)];
        buffer
            .apply(buffer.version(), edits, &sel, None, UndoMode::NewStep)
            .unwrap();
        assert_eq!(buffer.notes[0].at, 2);
    }
}
