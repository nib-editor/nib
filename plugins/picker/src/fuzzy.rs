//! Scores how well a query matches a path, for sorting candidates.

/// The char indices of `text` that `query`'s chars match, as `score`
/// takes them: each at its first place after the one before.
pub fn positions(query: &str, text: &str) -> Vec<usize> {
    let mut found = Vec::new();
    let mut chars = text.chars().enumerate();
    for wanted in query.chars().flat_map(char::to_lowercase) {
        match chars.find(|(_, c)| c.to_lowercase().eq(std::iter::once(wanted))) {
            Some((i, _)) => found.push(i),
            None => break,
        }
    }
    found
}

/// The score of `path` for `query`, or `None` if the query's chars do not
/// all appear in order. Case is ignored. Matches at the start of a path
/// segment or a word, runs of chars, and matches in the file name score
/// higher; longer paths score a little lower.
pub fn score(query: &str, path: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let name_start = path.rfind(['/', '\\']).map_or(0, |i| i + 1);
    let mut chars = path.char_indices().peekable();
    let mut score = 0i64;
    let mut previous: Option<usize> = None;
    let mut before = None;
    for wanted in query.chars().flat_map(char::to_lowercase) {
        loop {
            let (at, c) = chars.next()?;
            let matched = c.to_lowercase().eq(std::iter::once(wanted));
            let last = before.replace(c);
            if !matched {
                continue;
            }
            score += 1;
            if previous.is_some_and(|p| p + 1 == at) {
                score += 5;
            }
            let starts_word = match last {
                None => true,
                Some(last) => {
                    matches!(last, '/' | '\\' | '_' | '-' | '.' | ' ')
                        || (last.is_lowercase() && c.is_uppercase())
                }
            };
            if starts_word {
                score += 8;
            }
            if at >= name_start {
                score += 2;
            }
            previous = Some(at);
            break;
        }
    }
    Some(score * 16 - path.len() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn best<'a>(query: &str, paths: &[&'a str]) -> &'a str {
        paths
            .iter()
            .filter_map(|p| Some((score(query, p)?, *p)))
            .max_by_key(|&(s, _)| s)
            .unwrap()
            .1
    }

    #[test]
    fn needs_every_char_in_order() {
        assert!(score("abc", "a/b/c").is_some());
        assert!(score("cba", "a/b/c").is_none());
        assert!(score("ABC", "abc").is_some());
    }

    #[test]
    fn prefers_names_and_word_starts() {
        let paths = [
            "core/src/editor.rs",
            "docs/roadmap.md",
            "core/Cargo.toml",
            "Cargo.toml",
        ];
        assert_eq!(best("cargo", &paths), "Cargo.toml");
        assert_eq!(best("edit", &paths), "core/src/editor.rs");
        assert_eq!(best("rm", &paths), "docs/roadmap.md");
    }
}
