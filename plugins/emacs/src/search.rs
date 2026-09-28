//! Incremental search (`C-s`, `C-r`), and replacing (`M-%` and the
//! commands that do not ask), with Emacs's patterns read into the core's
//! regex syntax.

use base_kit::doc::Doc;
use base_kit::text::Text;
use base_kit::{error_message, regex_escape};
use nib_plugin::exports::nib::plugin::guest::KeyResult;
use nib_plugin::nib::plugin::prompt::Line;
use nib_plugin::nib::plugin::types::{Edit, KeyCode, KeyEvent, UndoMode};
use nib_plugin::nib::plugin::ui::{self, Decoration};
use nib_plugin::nib::plugin::view::{self, View};

use crate::edit_range;
use crate::motion::is_word;
use crate::{Emacs, ctrl, is_key, is_quit, plain};

/// One step of a search: what was searched for, and where it got. `DEL`
/// goes back a step.
#[derive(Clone)]
struct Step {
    text: String,
    forward: bool,
    /// The match, or where the point stayed when there was none.
    found: Option<(u64, u64)>,
    at: u64,
    wrapped: bool,
    /// The pattern is not a whole regexp yet, as `t\(` while typing
    /// `t\(w\)`: the match before it stays.
    incomplete: bool,
}

pub struct Isearch {
    line: Line,
    regexp: bool,
    origin: u64,
    steps: Vec<Step>,
}

/// `M-%` asking at each match.
pub struct Query {
    from: String,
    to: String,
    regexp: bool,
    /// Search without regard to case, and match the replacement's case.
    fold: bool,
    /// Bytes from the end of the buffer where replacing stops.
    tail: u64,
    current: Option<(u64, u64)>,
    replaced: usize,
    /// `,` replaced the match and stays on it.
    stay: bool,
}

impl Step {
    fn failing(&self) -> bool {
        !self.text.is_empty() && self.found.is_none() && !self.incomplete
    }
}

/// The pattern to search for: `text` as it is, or read as Emacs's regexp,
/// ignoring case unless it has upper case letters.
pub fn pattern(text: &str, regexp: bool) -> Result<String, String> {
    let body = if regexp {
        translate(text)?
    } else {
        regex_escape(text)
    };
    let fold = !text.chars().any(char::is_uppercase);
    Ok(if fold { format!("(?i){body}") } else { body })
}

/// Emacs's regexp `emacs` in the core's syntax: `\(` groups, `\|`
/// alternates, and `(`, `|`, `{` are plain chars.
pub fn translate(emacs: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut chars = emacs.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                let Some(next) = chars.next() else {
                    return Err("Trailing backslash".into());
                };
                match next {
                    '(' => {
                        if chars.peek() == Some(&'?') {
                            chars.next();
                            if chars.next() != Some(':') {
                                return Err("Invalid regexp: numbered groups".into());
                            }
                            out.push_str("(?:");
                        } else {
                            out.push('(');
                        }
                    }
                    ')' | '|' => out.push(next),
                    '{' => {
                        out.push('{');
                        loop {
                            match chars.next() {
                                Some('\\') if chars.peek() == Some(&'}') => {
                                    chars.next();
                                    out.push('}');
                                    break;
                                }
                                Some(d) if d.is_ascii_digit() || d == ',' => out.push(d),
                                _ => return Err("Invalid content of \\{\\}".into()),
                            }
                        }
                    }
                    '<' | '>' | 'b' => out.push_str("\\b"),
                    'B' => out.push_str("\\B"),
                    'w' => out.push_str("[\\w$%]"),
                    'W' => out.push_str("[^\\w$%]"),
                    '`' => out.push_str("\\A"),
                    '\'' => out.push_str("\\z"),
                    's' | 'S' => {
                        let class = chars.next().ok_or("Invalid syntax class")?;
                        let set = match class {
                            '-' | ' ' => "\\s",
                            'w' => "\\w",
                            '_' => "_&*+\\-/<=>|",
                            '.' => "!#,.:;?@^`~'",
                            '(' => "(\\[{",
                            ')' => ")\\]}",
                            '"' => "\"",
                            _ => return Err(format!("Invalid syntax class \\s{class}")),
                        };
                        let negate = if next == 'S' { "^" } else { "" };
                        if set.starts_with('\\') && set.len() == 2 {
                            if negate.is_empty() {
                                out.push_str(set);
                            } else {
                                out.push_str(&set.to_uppercase());
                            }
                        } else {
                            out.push_str(&format!("[{negate}{set}]"));
                        }
                    }
                    '_' => match chars.next() {
                        Some('<') | Some('>') => out.push_str("\\b"),
                        _ => return Err("Invalid \\_ construct".into()),
                    },
                    d if d.is_ascii_digit() => {
                        return Err("Back references are not supported".into());
                    }
                    '\\' => out.push_str("\\\\"),
                    other => out.push_str(&regex_escape(&other.to_string())),
                }
            }
            '(' | ')' | '{' | '}' | '|' => {
                out.push('\\');
                out.push(c);
            }
            '[' => {
                out.push('[');
                if chars.peek() == Some(&'^') {
                    out.push(chars.next().expect("peeked"));
                }
                if chars.peek() == Some(&']') {
                    chars.next();
                    out.push_str("\\]");
                }
                loop {
                    match chars.next() {
                        Some(']') => {
                            out.push(']');
                            break;
                        }
                        Some('[') if chars.peek() == Some(&':') => {
                            out.push_str("[:");
                            chars.next();
                            loop {
                                match chars.next() {
                                    Some(':') if chars.peek() == Some(&']') => {
                                        chars.next();
                                        out.push_str(":]");
                                        break;
                                    }
                                    Some(ch) => out.push(ch),
                                    None => return Err("Unmatched [ or [^".into()),
                                }
                            }
                        }
                        Some(ch @ ('\\' | '[' | '&' | '~')) => {
                            out.push('\\');
                            out.push(ch);
                        }
                        Some(ch) => out.push(ch),
                        None => return Err("Unmatched [ or [^".into()),
                    }
                }
            }
            c => out.push(c),
        }
    }
    Ok(out)
}

/// `to` with Emacs's `\&` as the matched text, `\1` to `\9` as its
/// groups, and `\\` as a backslash. `groups` has the whole match first.
fn expand(to: &str, groups: &[String]) -> Result<String, String> {
    let group = |n: usize| groups.get(n).map_or("", String::as_str);
    let mut out = String::new();
    let mut chars = to.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('&') => out.push_str(group(0)),
            Some('\\') => out.push('\\'),
            Some(d @ '0'..='9') => out.push_str(group(d as usize - '0' as usize)),
            Some(other) => {
                return Err(format!(
                    "Invalid use of `\\' in replacement text: \\{other}"
                ));
            }
            None => return Err("Invalid use of `\\' in replacement text".into()),
        }
    }
    Ok(out)
}

/// The replacement in the case of the match it replaces, as Emacs does
/// when neither was typed with capitals: all capitals stay capitals, and
/// capitalized words stay capitalized.
fn match_case(replacement: &str, matched: &str) -> String {
    let letters: Vec<char> = matched.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.is_empty() {
        return replacement.to_string();
    }
    let all_upper = letters.iter().all(|c| c.is_uppercase());
    if all_upper && letters.len() > 1 {
        return replacement.to_uppercase();
    }
    let mut capitalized = true;
    let mut in_word = false;
    let mut any_word = false;
    for c in matched.chars() {
        if is_word(c) {
            if !in_word && c.is_lowercase() {
                capitalized = false;
            }
            if in_word && c.is_uppercase() {
                capitalized = false;
            }
            in_word = true;
            any_word = true;
        } else {
            in_word = false;
        }
    }
    if capitalized && any_word {
        crate::command::Case::Capital.apply(replacement)
    } else {
        replacement.to_string()
    }
}

impl Emacs {
    pub fn start_isearch(&mut self, view: &View, forward: bool, regexp: bool) {
        let at = self.point(view);
        self.deactivate = true;
        self.isearch = Some(Isearch {
            line: Line::new(&isearch_label(forward, regexp, false, false)),
            regexp,
            origin: at,
            steps: vec![Step {
                text: String::new(),
                forward,
                found: None,
                at,
                wrapped: false,
                incomplete: false,
            }],
        });
    }

    pub fn isearch_key(&mut self, ev: KeyEvent) -> KeyResult {
        let Some(search) = self.isearch.as_mut() else {
            return KeyResult::Pass;
        };
        let step = search.steps.last().expect("a search has a step").clone();
        let view = view::active();
        match (ctrl(&ev), ev.code) {
            (Some(c @ ('s' | 'r')), _) => {
                let forward = c == 's';
                let next = self.isearch_next(&view, &step, forward);
                self.push_step(next);
            }
            (Some('g'), _) => {
                if step.failing() {
                    // Back to the last step that found something.
                    let search = self.isearch.as_mut().expect("searching");
                    while search.steps.len() > 1 && search.steps.last().is_some_and(Step::failing) {
                        search.steps.pop();
                    }
                    self.show_step();
                } else {
                    let origin = search.origin;
                    self.end_isearch(&view, origin, false);
                    ui::show_message("Quit");
                    self.failed = true;
                }
            }
            (Some('w'), _) => {
                let doc = Doc::new(view.buffer());
                let mut t = Text::new(&doc);
                let from = step.found.map_or(step.at, |(_, e)| e);
                let from = if step.forward { step.at } else { from };
                let to = crate::motion::forward_word(&mut t, from).unwrap_or(from);
                let more = doc.slice(from, to);
                self.add_text(&view, &more);
            }
            (Some('y'), _) => {
                let more = self
                    .kills
                    .first()
                    .map(|k| k.text.clone())
                    .unwrap_or_default();
                self.add_text(&view, &more);
            }
            (Some('j'), _) => self.add_text(&view, "\n"),
            _ if is_key(&ev, KeyCode::Backspace) => {
                let search = self.isearch.as_mut().expect("searching");
                if search.steps.len() > 1 {
                    search.steps.pop();
                }
                self.show_step();
            }
            _ if is_key(&ev, KeyCode::Enter) => {
                let at = step.at;
                self.end_isearch(&view, at, true);
            }
            _ if plain(&ev).is_some()
                && !ev
                    .modifiers
                    .contains(nib_plugin::nib::plugin::types::Modifiers::SUPER) =>
            {
                let c = plain(&ev).expect("checked");
                self.add_text(&view, &c.to_string());
            }
            _ => {
                // Any other key ends the search and does what it does.
                let at = step.at;
                self.end_isearch(&view, at, true);
                self.command_key(ev);
            }
        }
        KeyResult::Handled
    }

    /// Adds `more` to what is searched for, and looks again from the start
    /// of the match.
    pub fn add_text(&mut self, view: &View, more: &str) {
        let Some(search) = self.isearch.as_ref() else {
            return;
        };
        let step = search.steps.last().expect("a search has a step").clone();
        let text = format!("{}{more}", step.text);
        let mut next = Step {
            text: text.clone(),
            ..step.clone()
        };
        if !step.failing() {
            let start = match (step.found, step.forward) {
                (Some((s, _)), true) => s,
                (Some((s, _)), false) => s + text.len() as u64,
                (None, true) => step.at,
                (None, false) => step.at + text.len() as u64,
            };
            let start = start.min(view.buffer().len());
            match self.try_find(view, &text, search.regexp, start, step.forward) {
                Ok(found) => {
                    next.found = found;
                    next.incomplete = false;
                    if let Some((s, e)) = found {
                        next.at = if step.forward { e } else { s };
                    }
                }
                Err(_) => next.incomplete = true,
            }
        }
        self.push_step(next);
    }

    /// `C-s` and `C-r` in a search: the next match, or the last search
    /// again, or the other end of the match when turning around, or from
    /// the other end of the buffer after failing.
    fn isearch_next(&mut self, view: &View, step: &Step, forward: bool) -> Step {
        let regexp = self.isearch.as_ref().is_some_and(|s| s.regexp);
        let mut next = step.clone();
        next.forward = forward;
        if step.text.is_empty() {
            let Some((last, _)) = self.last_search.clone() else {
                return next;
            };
            next.text = last;
            next.found = self.find(view, &next.text, regexp, step.at, forward);
        } else if forward != step.forward {
            if let Some((s, e)) = step.found {
                next.at = if forward { e } else { s };
                return next;
            }
            let start = step.at;
            next.found = self.find(view, &next.text, regexp, start, forward);
        } else if step.failing() {
            let start = if forward { 0 } else { view.buffer().len() };
            next.found = self.find(view, &next.text, regexp, start, forward);
            next.wrapped = true;
        } else {
            let (s, e) = step.found.unwrap_or((step.at, step.at));
            let start = if forward {
                if s == e {
                    Doc::new(view.buffer()).next_grapheme(e)
                } else {
                    e
                }
            } else if s == e {
                Doc::new(view.buffer()).prev_grapheme(s)
            } else {
                s
            };
            next.found = if (forward && start >= view.buffer().len() && s == e)
                || (!forward && start == 0 && s == e)
            {
                None
            } else {
                self.find(view, &next.text, regexp, start, forward)
            };
        }
        if let Some((s, e)) = next.found {
            next.at = if forward { e } else { s };
        }
        next
    }

    fn find(
        &mut self,
        view: &View,
        text: &str,
        regexp: bool,
        start: u64,
        forward: bool,
    ) -> Option<(u64, u64)> {
        self.try_find(view, text, regexp, start, forward)
            .unwrap_or_else(|err| {
                ui::show_message(&err);
                None
            })
    }

    /// The match, or why the pattern is not one.
    fn try_find(
        &mut self,
        view: &View,
        text: &str,
        regexp: bool,
        start: u64,
        forward: bool,
    ) -> Result<Option<(u64, u64)>, String> {
        let pattern = pattern(text, regexp)?;
        view.buffer()
            .find(&pattern, start, !forward)
            .map(|found| found.map(|r| (r.start, r.end)))
            .map_err(error_message)
    }

    fn push_step(&mut self, step: Step) {
        if let Some(search) = self.isearch.as_mut() {
            search.steps.push(step);
        }
        self.show_step();
    }

    /// Shows the search: its line, the point at the match, and the match.
    fn show_step(&mut self) {
        let Some(search) = self.isearch.as_mut() else {
            return;
        };
        let step = search.steps.last().expect("a search has a step").clone();
        let regexp = search.regexp;
        let mut label = isearch_label(step.forward, regexp, step.failing(), step.wrapped);
        if step.incomplete {
            label = label.replace(": ", " (incomplete input): ");
        }
        search.line.set_label(&label);
        search.line.set(&step.text, step.text.len() as u32);
        let view = view::active();
        match step.found {
            Some((s, e)) if !step.text.is_empty() => {
                let (anchor, head) = if step.forward { (s, e) } else { (e, s) };
                edit_range(&view, anchor, head);
            }
            _ => self.goto(&view, step.at),
        }
        let buffer = view.buffer();
        let decorations: Vec<Decoration> = match (step.found, step.text.is_empty()) {
            (Some(_), false) => {
                let shown = view.visible_range();
                let (top, bottom) = (shown.start, shown.end);
                pattern(&step.text, regexp)
                    .ok()
                    .and_then(|p| buffer.find_all(&p, top, bottom).ok())
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|r| r.start < r.end)
                    .map(|r| Decoration {
                        start: r.start,
                        end: r.end,
                        style: "ui.selection".into(),
                    })
                    .collect()
            }
            _ => Vec::new(),
        };
        ui::set_decorations(&buffer, "isearch", &decorations);
    }

    /// Ends the search with the point at `at`; `done` keeps what was
    /// searched for and leaves the mark where the search started.
    fn end_isearch(&mut self, view: &View, at: u64, done: bool) {
        let Some(search) = self.isearch.take() else {
            return;
        };
        ui::set_decorations(&view.buffer(), "isearch", &[]);
        let text = search
            .steps
            .iter()
            .rev()
            .find(|s| !s.text.is_empty())
            .map(|s| s.text.clone());
        if done {
            if let Some(text) = text {
                self.last_search = Some((text.clone(), search.regexp));
                self.remember("search", &text);
            }
            if at != search.origin {
                self.push_mark(view, search.origin);
                ui::show_message("Mark saved where search started");
            }
        }
        self.active = false;
        self.goto(view, at);
    }

    /// `M-%` and the others: ask what to replace, then with what.
    pub fn ask_replace(&mut self, name: &str) {
        let regexp = name.contains("regexp");
        let query = name.starts_with("query");
        let what = match (query, regexp) {
            (true, false) => "Query replace",
            (true, true) => "Query replace regexp",
            (false, false) => "Replace string",
            (false, true) => "Replace regexp",
        };
        let region = if self.active { " in region" } else { "" };
        let default = self.last_replace.clone();
        let label = match &default {
            Some((from, to)) => format!("{what}{region} (default {from} → {to}): "),
            None => format!("{what}{region}: "),
        };
        self.ask(
            crate::minibuffer::Ask::ReplaceFrom {
                regexp,
                query,
                region: self.active,
            },
            &label,
            "",
        );
    }

    /// Starts replacing `from` with `to` after the point, or in the region.
    pub fn start_replace(
        &mut self,
        view: &View,
        from: String,
        to: String,
        regexp: bool,
        query: bool,
        region: bool,
    ) -> Result<(), String> {
        self.last_replace = Some((from.clone(), to.clone()));
        let len = view.buffer().len();
        let (start, end) = if region {
            self.region(view)?
        } else {
            (self.point(view), len)
        };
        let here = self.point(view);
        if !self.active || !region {
            self.push_mark(view, here);
        }
        self.active = false;
        let fold = !from.chars().any(char::is_uppercase) && !to.chars().any(char::is_uppercase);
        let mut state = Query {
            from,
            to,
            regexp,
            fold,
            tail: len - end,
            current: None,
            replaced: 0,
            stay: false,
        };
        state.current = self.next_match(view, &state, start)?;
        if query {
            if state.current.is_none() {
                ui::show_message("Replaced 0 occurrences");
                return Ok(());
            }
            self.query = Some(state);
            self.show_query();
            return Ok(());
        }
        self.query = Some(state);
        self.replace_rest(view);
        self.finish_query();
        Ok(())
    }

    fn next_match(
        &self,
        view: &View,
        state: &Query,
        start: u64,
    ) -> Result<Option<(u64, u64)>, String> {
        let buffer = view.buffer();
        let end = buffer.len().saturating_sub(state.tail);
        if start > end {
            return Ok(None);
        }
        let pattern = query_pattern(state)?;
        match buffer.find(&pattern, start, false) {
            Ok(Some(r)) if r.end <= end => Ok(Some((r.start, r.end))),
            Ok(_) => Ok(None),
            Err(err) => Err(error_message(err)),
        }
    }

    /// Replaces the current match, and returns where the search goes on.
    fn replace_current(&mut self, view: &View) -> Result<u64, String> {
        let state = self.query.as_mut().expect("replacing");
        let (s, e) = state.current.expect("a match");
        let matched = view.buffer().slice(s, e).unwrap_or_default();
        let groups = if base_kit::refers_to_groups(&state.to) {
            base_kit::match_groups(&view.buffer(), &query_pattern(state)?, s)?
        } else {
            vec![matched.clone()]
        };
        let mut text = expand(&state.to, &groups)?;
        if state.fold {
            text = match_case(&text, &matched);
        }
        let end = s + text.len() as u64;
        let undo = if state.replaced == 0 {
            UndoMode::NewStep
        } else {
            UndoMode::Merge
        };
        state.replaced += 1;
        let version = view.buffer().version();
        let edit = Edit {
            start: s,
            end: e,
            text,
        };
        let _ = view.apply(version, &[edit], None, undo);
        self.goto(view, end);
        // An empty match moves on by a char, so it is not found again.
        Ok(if s == e {
            Doc::new(view.buffer()).next_grapheme(end)
        } else {
            end
        })
    }

    fn replace_rest(&mut self, view: &View) {
        loop {
            let Some(state) = self.query.as_ref() else {
                return;
            };
            if state.current.is_none() {
                return;
            }
            let from = match self.replace_current(view) {
                Ok(from) => from,
                Err(err) => {
                    ui::show_message(&err);
                    return;
                }
            };
            let state = self.query.as_ref().expect("replacing");
            let next = self.next_match(view, state, from).ok().flatten();
            self.query.as_mut().expect("replacing").current = next;
        }
    }

    fn show_query(&mut self) {
        let Some(state) = self.query.as_ref() else {
            return;
        };
        let view = view::active();
        if let Some((s, e)) = state.current {
            edit_range(&view, s, e);
        }
        let what = if state.regexp {
            "Query replacing regexp"
        } else {
            "Query replacing"
        };
        ui::show_message(&format!(
            "{what} {} with {}: (? for help)",
            state.from, state.to
        ));
    }

    fn finish_query(&mut self) {
        let Some(state) = self.query.take() else {
            return;
        };
        let n = state.replaced;
        let plural = if n == 1 { "" } else { "s" };
        ui::show_message(&format!("Replaced {n} occurrence{plural}"));
        self.render();
    }

    pub fn query_key(&mut self, ev: KeyEvent) -> KeyResult {
        let view = view::active();
        let Some(state) = self.query.as_ref() else {
            return KeyResult::Pass;
        };
        let (s, e) = state.current.expect("a query has a match");
        let stay = state.stay;
        let key = plain(&ev).or(if is_key(&ev, KeyCode::Backspace) {
            Some('\u{7f}')
        } else if is_key(&ev, KeyCode::Enter) {
            Some('\r')
        } else {
            None
        });
        let result = match key {
            Some('y' | ' ') if stay => self.query_move(&view, e),
            Some('n' | '\u{7f}') if stay => self.query_move(&view, e),
            Some('y' | ' ') => self
                .replace_current(&view)
                .and_then(|from| self.query_move(&view, from)),
            Some('n' | '\u{7f}') => {
                let from = if s == e {
                    Doc::new(view.buffer()).next_grapheme(e)
                } else {
                    e
                };
                self.query_move(&view, from)
            }
            Some('!') => {
                if stay {
                    let _ = self.query_move(&view, e);
                }
                self.replace_rest(&view);
                self.finish_query();
                Ok(())
            }
            Some('.') => {
                let replaced = if stay {
                    Ok(e)
                } else {
                    self.replace_current(&view)
                };
                replaced.map(|_| self.finish_query())
            }
            Some(',') if stay => Ok(()),
            Some(',') => self.replace_current(&view).map(|_| {
                self.query.as_mut().expect("replacing").stay = true;
            }),
            Some('q' | '\r') => {
                self.goto(&view, e);
                self.finish_query();
                Ok(())
            }
            Some('?') => {
                ui::show_message(
                    "y/SPC replace, n/DEL skip, ! all, . replace and stop, , replace and stay, q/RET stop",
                );
                return KeyResult::Handled;
            }
            _ if is_quit(&ev) => {
                self.goto(&view, e);
                self.finish_query();
                self.failed = true;
                ui::show_message("Quit");
                return KeyResult::Handled;
            }
            _ => {
                // Any other key ends replacing and does what it does.
                self.goto(&view, e);
                self.finish_query();
                self.command_key(ev);
                return KeyResult::Handled;
            }
        };
        if let Err(err) = result {
            ui::show_message(&err);
        }
        if self.query.is_some() {
            self.show_query();
        }
        KeyResult::Handled
    }

    fn query_move(&mut self, view: &View, from: u64) -> Result<(), String> {
        let state = self.query.as_ref().expect("replacing");
        let next = self.next_match(view, state, from)?;
        let state = self.query.as_mut().expect("replacing");
        state.stay = false;
        match next {
            Some(found) => state.current = Some(found),
            None => self.finish_query(),
        }
        Ok(())
    }
}

/// What `M-%` searches for, in the core's syntax.
fn query_pattern(state: &Query) -> Result<String, String> {
    let body = if state.regexp {
        translate(&state.from)?
    } else {
        regex_escape(&state.from)
    };
    Ok(if state.fold {
        format!("(?i){body}")
    } else {
        body
    })
}

fn isearch_label(forward: bool, regexp: bool, failing: bool, wrapped: bool) -> String {
    let mut label = String::new();
    if failing {
        label.push_str("Failing ");
    }
    if wrapped {
        label.push_str("Wrapped ");
    }
    if regexp {
        label.push_str("Regexp ");
    }
    label.push_str(if forward {
        "I-search: "
    } else {
        "I-search backward: "
    });
    label
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emacs_patterns_read_as_the_cores() {
        assert_eq!(translate(r"\(foo\|bar\)+").unwrap(), "(foo|bar)+");
        assert_eq!(translate("a(b)|{c}").unwrap(), r"a\(b\)\|\{c\}");
        assert_eq!(translate(r"x\{2,3\}").unwrap(), "x{2,3}");
        assert_eq!(translate(r"\<w\>\s-").unwrap(), r"\bw\b\s");
        assert_eq!(translate("[[:space:]a-z]").unwrap(), "[[:space:]a-z]");
        assert_eq!(translate(r"\(?:a\)").unwrap(), "(?:a)");
        assert!(translate(r"\1").is_err());
    }

    #[test]
    fn replacements_follow_the_case_of_the_match() {
        assert_eq!(match_case("xyz", "One"), "Xyz");
        assert_eq!(match_case("xyz", "ONE"), "XYZ");
        assert_eq!(match_case("xyz", "one"), "xyz");
        assert_eq!(match_case("xyz", "oNe"), "xyz");
        let groups = ["ab".to_string(), "b".to_string()];
        assert_eq!(expand(r"<\&>\\", &groups).unwrap(), r"<ab>\");
        assert_eq!(expand(r"\1-\2", &groups).unwrap(), "b-");
    }

    #[test]
    fn search_labels_say_how_it_goes() {
        assert_eq!(isearch_label(true, false, false, false), "I-search: ");
        assert_eq!(
            isearch_label(false, true, true, false),
            "Failing Regexp I-search backward: "
        );
    }
}
