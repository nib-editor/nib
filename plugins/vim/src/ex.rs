//! Reading `:` command lines as vim does: a range of lines, a command, a
//! `!`, and its argument, as in `:%s/a/b/g` or `:.,+2d`.

/// A line address, before it is resolved against the buffer.
#[derive(Clone, Debug, PartialEq)]
pub enum Base {
    /// A line number, counted from 1.
    Line(u64),
    /// `.`
    Current,
    /// `$`
    Last,
    /// `'a`
    Mark(char),
    /// `/pat/` and `?pat?`
    Pattern { pattern: String, backward: bool },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Address {
    pub base: Base,
    /// `+2`, `-`, and so on after it.
    pub offset: i64,
    /// After `;`: counted from the address before, not the cursor.
    pub from_previous: bool,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Ex {
    /// The addresses given; `%` gives the first and last lines.
    pub range: Vec<Address>,
    pub name: String,
    pub bang: bool,
    pub arg: String,
}

/// Reads a command line, without its `:`.
pub fn parse(line: &str) -> Result<Ex, String> {
    let mut rest = line.trim_start_matches([' ', ':']);
    let mut range = Vec::new();
    if let Some(after) = rest.strip_prefix('%') {
        range.push(Address {
            base: Base::Line(1),
            offset: 0,
            from_previous: false,
        });
        range.push(Address {
            base: Base::Last,
            offset: 0,
            from_previous: false,
        });
        rest = after;
    } else {
        let mut from_previous = false;
        while let Some((mut address, after)) = address(rest)? {
            address.from_previous = from_previous;
            range.push(address);
            rest = after.trim_start();
            from_previous = rest.starts_with(';');
            match rest.strip_prefix([',', ';']) {
                Some(after) => rest = after.trim_start(),
                None => break,
            }
        }
    }
    let name_len = if rest.starts_with("config-reload") {
        "config-reload".len()
    } else if rest.starts_with(|c: char| c.is_ascii_alphabetic()) {
        let letters = rest
            .find(|c: char| !c.is_ascii_alphabetic())
            .unwrap_or(rest.len());
        // A command of nib's or a plugin's, such as `buffer.open`.
        if rest[letters..].starts_with('.') {
            rest.find(char::is_whitespace).unwrap_or(rest.len())
        } else {
            letters
        }
    } else {
        // `:s/a/b/`-like commands made of one char, such as `&` and `>`.
        rest.chars().next().map_or(0, char::len_utf8)
    };
    let name = rest[..name_len].to_string();
    rest = &rest[name_len..];
    let bang = rest.starts_with('!');
    if bang {
        rest = &rest[1..];
    }
    Ok(Ex {
        range,
        name,
        bang,
        arg: rest.trim_start().to_string(),
    })
}

/// One address at the start of `text`, and what is after it.
fn address(text: &str) -> Result<Option<(Address, &str)>, String> {
    let mut rest = text.trim_start();
    let base = match rest.chars().next() {
        Some('.') => {
            rest = &rest[1..];
            Base::Current
        }
        Some('$') => {
            rest = &rest[1..];
            Base::Last
        }
        Some('\'') => {
            let mark = rest[1..].chars().next().ok_or("E20: Mark not set")?;
            rest = &rest[1 + mark.len_utf8()..];
            Base::Mark(mark)
        }
        Some(d) if d.is_ascii_digit() => {
            let end = rest
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len());
            let n = rest[..end].parse().map_err(|_| "E16: Invalid range")?;
            rest = &rest[end..];
            Base::Line(n)
        }
        Some(sep @ ('/' | '?')) => {
            let body = &rest[1..];
            let end = body.find(sep).unwrap_or(body.len());
            let pattern = body[..end].to_string();
            rest = body.get(end + 1..).unwrap_or("");
            Base::Pattern {
                pattern,
                backward: sep == '?',
            }
        }
        Some('+' | '-') => Base::Current,
        _ => return Ok(None),
    };
    let mut offset = 0i64;
    loop {
        let sign = match rest.chars().next() {
            Some('+') => 1,
            Some('-') => -1,
            _ => break,
        };
        rest = &rest[1..];
        let end = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        let n: i64 = if end == 0 {
            1
        } else {
            rest[..end].parse().map_err(|_| "E16: Invalid range")?
        };
        offset += sign * n;
        rest = &rest[end..];
    }
    Ok(Some((
        Address {
            base,
            offset,
            from_previous: false,
        },
        rest,
    )))
}

/// `:s`'s argument: `/pattern/replacement/flags`, with any separator.
/// Empty parts are `None` when not given at all.
pub fn substitute_parts(arg: &str) -> Option<(String, Option<String>, String)> {
    let mut chars = arg.chars();
    let sep = chars.next()?;
    if sep.is_alphanumeric() || sep == '\\' || sep == '"' || sep == '|' || sep == ' ' {
        return None;
    }
    let rest: String = chars.collect();
    let (pattern, after) = split_unescaped(&rest, sep);
    let Some(after) = after else {
        return Some((pattern, None, String::new()));
    };
    let (replacement, flags) = split_unescaped(&after, sep);
    Some((pattern, Some(replacement), flags.unwrap_or_default()))
}

/// `text` up to the first `sep` without a `\` before it, and what is after.
/// `\sep` becomes `sep`; other escapes stay.
fn split_unescaped(text: &str, sep: char) -> (String, Option<String>) {
    let mut out = String::new();
    let mut chars = text.char_indices();
    while let Some((i, c)) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some((_, next)) if next == sep => out.push(sep),
                Some((_, next)) => {
                    out.push('\\');
                    out.push(next);
                }
                None => out.push('\\'),
            }
        } else if c == sep {
            return (out, Some(text[i + c.len_utf8()..].to_string()));
        } else {
            out.push(c);
        }
    }
    (out, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(n: u64) -> Address {
        Address {
            base: Base::Line(n),
            offset: 0,
            from_previous: false,
        }
    }

    #[test]
    fn ranges_names_and_arguments() {
        let ex = parse("%s/a/b/g").unwrap();
        assert_eq!(ex.range.len(), 2);
        assert_eq!(ex.name, "s");
        assert_eq!(ex.arg, "/a/b/g");
        let ex = parse(".,+2d x").unwrap();
        assert_eq!(
            ex.range[1],
            Address {
                base: Base::Current,
                offset: 2,
                from_previous: false,
            }
        );
        assert!(parse("2;+1d").unwrap().range[1].from_previous);
        assert_eq!((ex.name.as_str(), ex.arg.as_str()), ("d", "x"));
        let ex = parse("12").unwrap();
        assert_eq!(ex.range, [line(12)]);
        assert!(ex.name.is_empty());
        let ex = parse("q!").unwrap();
        assert!(ex.bang);
        let ex = parse("'<,'>normal Ax").unwrap();
        assert_eq!(ex.range[0].base, Base::Mark('<'));
        assert_eq!((ex.name.as_str(), ex.arg.as_str()), ("normal", "Ax"));
        let ex = parse("g/foo/d").unwrap();
        assert_eq!((ex.name.as_str(), ex.arg.as_str()), ("g", "/foo/d"));
        let ex = parse("s#a#b#").unwrap();
        assert_eq!((ex.name.as_str(), ex.arg.as_str()), ("s", "#a#b#"));
        assert_eq!(parse("set nu").unwrap().name, "set");
        assert_eq!(parse("split").unwrap().name, "split");
        assert_eq!(parse("buffer.open path=x").unwrap().name, "buffer.open");
        assert_eq!(parse("$-1").unwrap().range[0].offset, -1);
        let ex = parse("1m3").unwrap();
        assert_eq!((ex.name.as_str(), ex.arg.as_str()), ("m", "3"));
        assert_eq!(parse("config-reload").unwrap().name, "config-reload");
    }

    #[test]
    fn substitutions_split_at_their_separator() {
        assert_eq!(
            substitute_parts("/a\\/b/c/g"),
            Some(("a/b".into(), Some("c".into()), "g".into()))
        );
        assert_eq!(
            substitute_parts("/a"),
            Some(("a".into(), None, String::new()))
        );
        assert_eq!(
            substitute_parts("/a\\.b//"),
            Some(("a\\.b".into(), Some(String::new()), String::new()))
        );
    }
}
