//! Regex search over a rope without copying it into one string.

use regex_cursor::Input;
use regex_cursor::engines::meta::Regex;
use regex_cursor::regex_automata::util::syntax;
use ropey::Rope;

use crate::Error;
use crate::grapheme::check_position;

/// Finds the first match starting at or after `start`, or with `backward`,
/// the last match ending at or before `start`. Does not wrap around.
pub fn find(
    text: &Rope,
    pattern: &str,
    start: usize,
    backward: bool,
) -> Result<Option<(usize, usize)>, Error> {
    check_position(text, start)?;
    let regex = compile(pattern)?;
    let found = if backward {
        regex.find_iter(Input::new(text).range(..start)).last()
    } else {
        regex.find(Input::new(text).range(start..))
    };
    Ok(found.map(|m| (m.start(), m.end())))
}

/// As [`find`], with the span of each group: the whole match first, then
/// each group, `None` for a group that took no part.
pub type Groups = Vec<Option<(usize, usize)>>;

pub fn find_groups(
    text: &Rope,
    pattern: &str,
    start: usize,
    backward: bool,
) -> Result<Option<Groups>, Error> {
    check_position(text, start)?;
    let regex = compile(pattern)?;
    let caps = if backward {
        regex.captures_iter(Input::new(text).range(..start)).last()
    } else {
        let mut caps = regex.create_captures();
        regex.captures(Input::new(text).range(start..), &mut caps);
        caps.is_match().then_some(caps)
    };
    Ok(caps.map(|caps| {
        (0..caps.group_len())
            .map(|i| caps.get_group(i).map(|span| (span.start, span.end)))
            .collect()
    }))
}

/// Finds all non-overlapping matches within `start..end`.
pub fn find_all(
    text: &Rope,
    pattern: &str,
    start: usize,
    end: usize,
) -> Result<Vec<(usize, usize)>, Error> {
    check_position(text, start)?;
    check_position(text, end)?;
    if end < start {
        return Err(Error::InvalidPosition(end));
    }
    let regex = compile(pattern)?;
    Ok(regex
        .find_iter(Input::new(text).range(start..end))
        .map(|m| (m.start(), m.end()))
        .collect())
}

fn compile(pattern: &str) -> Result<Regex, Error> {
    // `^` and `$` match at every line, as users expect in an editor.
    Regex::builder()
        .syntax(syntax::Config::new().multi_line(true))
        .build(pattern)
        .map_err(|err| Error::InvalidPattern(describe(&err)))
}

/// The one-line reason a pattern is invalid, e.g. "unclosed group". The
/// error itself only says which pattern failed; the parser's error, its
/// source, spans several lines with the pattern and a caret.
fn describe(err: &dyn std::error::Error) -> String {
    err.source()
        .and_then(|source| {
            source
                .to_string()
                .lines()
                .find_map(|line| line.strip_prefix("error: ").map(str::to_string))
        })
        .unwrap_or_else(|| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_come_with_the_match() {
        let text = Rope::from_str("ab ac ad");
        let groups = find_groups(&text, "a(b|c)|(d)", 1, false).unwrap().unwrap();
        assert_eq!(groups, vec![Some((3, 5)), Some((4, 5)), None]);
        let groups = find_groups(&text, "a(.)", 8, true).unwrap().unwrap();
        assert_eq!(groups, vec![Some((6, 8)), Some((7, 8))]);
        assert_eq!(find_groups(&text, "z", 0, false).unwrap(), None);
    }

    #[test]
    fn forward_and_backward() {
        let text = Rope::from_str("foo bar foo baz foo");
        assert_eq!(find(&text, "foo", 0, false).unwrap(), Some((0, 3)));
        assert_eq!(find(&text, "foo", 1, false).unwrap(), Some((8, 11)));
        assert_eq!(find(&text, "foo", 17, false).unwrap(), None);
        assert_eq!(find(&text, "foo", 16, true).unwrap(), Some((8, 11)));
        assert_eq!(find(&text, "foo", 2, true).unwrap(), None);
    }

    #[test]
    fn line_anchors_match_every_line() {
        let text = Rope::from_str("fn a\n  fn b\nfn c\n");
        assert_eq!(
            find_all(&text, "^fn", 0, text.len_bytes()).unwrap(),
            vec![(0, 2), (12, 14)]
        );
        assert_eq!(find(&text, r"b$", 0, false).unwrap(), Some((10, 11)));
    }

    #[test]
    fn find_all_within_range() {
        let text = Rope::from_str("aa あa aa");
        // The range cuts the last "aa" in half.
        assert_eq!(
            find_all(&text, "a+", 1, 9).unwrap(),
            vec![(1, 2), (6, 7), (8, 9)]
        );
    }

    #[test]
    fn match_spanning_chunks() {
        let s = format!("{}needle{}", "x".repeat(3000), "y".repeat(3000));
        let text = Rope::from_str(&s);
        assert!(text.chunks().count() > 1);
        assert_eq!(
            find(&text, "x+needley+", 0, false).unwrap(),
            Some((0, s.len()))
        );
        assert_eq!(find(&text, "needle", 0, false).unwrap(), Some((3000, 3006)));
    }

    #[test]
    fn errors() {
        let text = Rope::from_str("あ");
        assert!(matches!(
            find(&text, "(", 0, false),
            Err(Error::InvalidPattern(message)) if message == "unclosed group"
        ));
        assert!(matches!(
            find(&text, "a", 1, false),
            Err(Error::InvalidPosition(1))
        ));
    }
}
