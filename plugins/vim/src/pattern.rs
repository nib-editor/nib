//! vim's search patterns, as the core's regex engine reads them: `\(` for
//! a group, `\<` for the start of a word, `\{n,m}`, `\v`, `\c`, and so
//! on. What has no counterpart, such as `\zs`, is an error.

/// The pattern `vim` in the core's syntax.
pub fn translate(vim: &str) -> Result<String, String> {
    let mut out = String::new();
    // `\v`: every char but letters, digits, and `_` is special.
    let mut very_magic = false;
    // `\V`: only `\` is special.
    let mut very_nomagic = false;
    let mut ignore_case = false;
    let mut chars = vim.chars().peekable();
    // Inside `[...]`, chars keep their meaning.
    let mut in_class = false;
    while let Some(c) = chars.next() {
        if in_class {
            match c {
                ']' => {
                    in_class = false;
                    out.push(']');
                }
                '\\' => match chars.next() {
                    Some('n') => out.push_str("\\n"),
                    Some('t') => out.push_str("\\t"),
                    Some(']') => out.push_str("\\]"),
                    Some('\\') => out.push_str("\\\\"),
                    Some('-') => out.push_str("\\-"),
                    Some('^') => out.push_str("\\^"),
                    Some(other) => {
                        out.push_str("\\\\");
                        out.push(other);
                    }
                    None => out.push_str("\\\\"),
                },
                '[' => out.push_str("\\["),
                '&' | '~' => {
                    out.push('\\');
                    out.push(c);
                }
                c => out.push(c),
            }
            continue;
        }
        if c == '\\' {
            let Some(next) = chars.next() else {
                out.push_str("\\\\");
                break;
            };
            match next {
                'v' => very_magic = true,
                'm' | 'M' => very_magic = false,
                'V' => very_nomagic = true,
                'c' => ignore_case = true,
                'C' => {}
                '<' | '>' => out.push_str("\\b"),
                'n' => out.push_str("\\n"),
                't' => out.push_str("\\t"),
                's' | 'S' | 'd' | 'D' | 'w' | 'W' => {
                    out.push('\\');
                    out.push(next);
                }
                'a' => out.push_str("[A-Za-z]"),
                'A' => out.push_str("[^A-Za-z]"),
                'l' => out.push_str("[a-z]"),
                'L' => out.push_str("[^a-z]"),
                'u' => out.push_str("[A-Z]"),
                'U' => out.push_str("[^A-Z]"),
                'x' => out.push_str("[0-9A-Fa-f]"),
                'X' => out.push_str("[^0-9A-Fa-f]"),
                'h' => out.push_str("[A-Za-z_]"),
                'k' | 'i' | 'f' | 'p' => out.push_str("\\w"),
                '_' => match chars.next() {
                    Some('s') => out.push_str("\\s"),
                    Some('.') => out.push_str("(?s:.)"),
                    _ => return Err("E71: \\_ is not supported here".into()),
                },
                '{' if !very_magic => out.push_str(&braces(&mut chars)?),
                '(' | ')' | '|' | '+' | '=' | '?' if !very_magic => {
                    out.push(match next {
                        '=' => '?',
                        other => other,
                    });
                }
                '.' | '*' | '[' | '~' | '^' | '$' if very_nomagic => {
                    out.push(if next == '~' { '~' } else { next });
                    if next == '[' {
                        in_class = true;
                    }
                }
                'z' => {
                    return Err(format!(
                        "\\z{} is not supported",
                        chars.next().unwrap_or(' ')
                    ));
                }
                '%' => return Err("\\% is not supported".into()),
                other => {
                    out.push('\\');
                    out.push(other);
                }
            }
            continue;
        }
        if very_nomagic {
            out.push_str(&base_kit::regex_escape(&c.to_string()));
            continue;
        }
        match c {
            '[' => {
                in_class = true;
                out.push('[');
                if chars.peek() == Some(&'^') {
                    out.push(chars.next().unwrap_or('^'));
                }
                // A `]` first is a char of the class.
                if chars.peek() == Some(&']') {
                    chars.next();
                    out.push_str("\\]");
                }
            }
            '.' | '*' | '^' | '$' => out.push(c),
            '~' => out.push('~'),
            '<' | '>' if very_magic => out.push_str("\\b"),
            '{' if very_magic => out.push_str(&braces(&mut chars)?),
            '(' | ')' | '|' | '+' | '?' if very_magic => out.push(c),
            '=' if very_magic => out.push('?'),
            c => out.push_str(&base_kit::regex_escape(&c.to_string())),
        }
    }
    if in_class {
        return Err("E769: missing ] after [".into());
    }
    if ignore_case {
        out.insert_str(0, "(?i)");
    }
    Ok(out)
}

/// `\{n,m}` after its `\{`: `{n,m}`, or with `-` in front, its lazy form.
fn braces(chars: &mut std::iter::Peekable<std::str::Chars>) -> Result<String, String> {
    let mut inside = String::new();
    loop {
        match chars.next() {
            Some('\\') if chars.peek() == Some(&'}') => {
                chars.next();
                break;
            }
            Some('}') => break,
            Some(c) => inside.push(c),
            None => return Err("E554: syntax error in \\{...}".into()),
        }
    }
    let (lazy, counts) = match inside.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, inside.as_str()),
    };
    let quantifier = match counts {
        "" => "*".to_string(),
        n if !n.contains(',') => format!("{{{n}}}"),
        n if n.starts_with(',') => format!("{{0{n}}}"),
        n => format!("{{{n}}}"),
    };
    Ok(if lazy {
        format!("{quantifier}?")
    } else {
        quantifier
    })
}

/// `:s`'s replacement for a match of `matched`: `&` and `\0` are the match,
/// `\r` and `\n` a line break, and `\` takes the next char as it is.
pub fn replacement(template: &str, matched: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut chars = template.chars();
    while let Some(c) = chars.next() {
        match c {
            '&' => out.push_str(matched),
            '\\' => match chars.next() {
                Some('0') => out.push_str(matched),
                Some('1'..='9') => {
                    return Err("groups (\\1 to \\9) are not supported in :s yet".into());
                }
                Some('r' | 'n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            },
            c => out.push(c),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_patterns_become_the_cores() {
        assert_eq!(translate("foo.*bar").unwrap(), "foo.*bar");
        assert_eq!(translate(r"\(a\|b\)\+").unwrap(), "(a|b)+");
        assert_eq!(translate("(a|b)+").unwrap(), r"\(a\|b\)\+");
        assert_eq!(translate(r"\<word\>").unwrap(), r"\bword\b");
        assert_eq!(translate(r"a\{2,3}").unwrap(), "a{2,3}");
        assert_eq!(translate(r"a\{-}").unwrap(), "a*?");
        assert_eq!(translate(r"\v(a|b){2}").unwrap(), "(a|b){2}");
        assert_eq!(translate(r"\Va.b").unwrap(), r"a\.b");
        assert_eq!(translate(r"\cFoo").unwrap(), "(?i)Foo");
        assert_eq!(translate("[]a]").unwrap(), r"[\]a]");
        assert_eq!(translate(r"x\=").unwrap(), "x?");
        assert!(translate("[abc").is_err());
        assert!(translate(r"a\zsb").is_err());
    }

    #[test]
    fn replacements_take_the_match() {
        assert_eq!(replacement(r"<&>\&\r", "x").unwrap(), "<x>&\n");
        assert!(replacement(r"\1", "x").is_err());
    }
}
