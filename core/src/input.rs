//! Keys in a form that does not depend on the terminal.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeyCode {
    Char(char),
    Enter,
    Escape,
    Tab,
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    F(u8),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub super_: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeyEvent {
    pub code: KeyCode,
    pub modifiers: Modifiers,
}

impl KeyEvent {
    pub fn new(code: KeyCode) -> Self {
        Self {
            code,
            modifiers: Modifiers::default(),
        }
    }

    pub const fn ctrl(c: char) -> Self {
        Self {
            code: KeyCode::Char(c),
            modifiers: Modifiers {
                ctrl: true,
                alt: false,
                shift: false,
                super_: false,
            },
        }
    }
}

/// Shows the key for people, e.g. "Ctrl-g" or "Alt-Enter".
impl std::fmt::Display for KeyEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (on, name) in [
            (self.modifiers.ctrl, "Ctrl-"),
            (self.modifiers.alt, "Alt-"),
            (self.modifiers.shift, "Shift-"),
            (self.modifiers.super_, "Super-"),
        ] {
            if on {
                f.write_str(name)?;
            }
        }
        match self.code {
            KeyCode::Char(' ') => f.write_str("Space"),
            KeyCode::Char(c) => write!(f, "{c}"),
            KeyCode::F(n) => write!(f, "F{n}"),
            code => write!(f, "{code:?}"),
        }
    }
}

/// Parses Helix's key notation: optional `C-`, `A-`, `S-` prefixes, then a
/// char or a name such as `ret`, `esc`, `space`, or `F5`.
impl std::str::FromStr for KeyEvent {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut modifiers = Modifiers::default();
        let mut rest = s;
        loop {
            let flag = match rest.get(..2) {
                Some("C-") => &mut modifiers.ctrl,
                Some("A-") => &mut modifiers.alt,
                Some("S-") => &mut modifiers.shift,
                _ => break,
            };
            // "C--" is Ctrl with the minus key, not a prefix.
            if rest.len() == 2 {
                break;
            }
            *flag = true;
            rest = &rest[2..];
        }
        let code = match rest {
            "ret" | "enter" => KeyCode::Enter,
            "esc" => KeyCode::Escape,
            "tab" => KeyCode::Tab,
            "backspace" => KeyCode::Backspace,
            "del" => KeyCode::Delete,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" => KeyCode::PageUp,
            "pagedown" => KeyCode::PageDown,
            "space" => KeyCode::Char(' '),
            "minus" => KeyCode::Char('-'),
            "lt" => KeyCode::Char('<'),
            "gt" => KeyCode::Char('>'),
            _ => {
                let mut chars = rest.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => KeyCode::Char(c),
                    _ => match rest.strip_prefix('F').and_then(|n| n.parse().ok()) {
                        Some(n @ 1..=24) => KeyCode::F(n),
                        _ => return Err(format!("unknown key {s:?}")),
                    },
                }
            }
        };
        // A char already says whether it is shifted, as the frontend
        // reports it: "S-a" is 'A'.
        if let KeyCode::Char(c) = code
            && modifiers.shift
        {
            modifiers.shift = false;
            return Ok(Self {
                code: KeyCode::Char(c.to_ascii_uppercase()),
                modifiers,
            });
        }
        Ok(Self { code, modifiers })
    }
}

/// Parses a sequence of keys: chars as they are, and a key in the notation
/// above between `<` and `>`, as in `ihello<esc>` or `<C-w>v`. `<lt>` is a
/// `<`. A line break and a tab in the text are Enter and Tab.
pub fn parse_keys(keys: &str) -> Result<Vec<KeyEvent>, String> {
    let mut parsed = Vec::new();
    let mut rest = keys;
    while let Some(c) = rest.chars().next() {
        if c == '<' {
            let end = rest
                .find('>')
                .ok_or_else(|| format!("unclosed `<` in {keys:?}; write `<lt>` for a `<`"))?;
            let name = &rest[1..end];
            parsed.push(match name {
                "lt" => KeyEvent::new(KeyCode::Char('<')),
                "gt" => KeyEvent::new(KeyCode::Char('>')),
                _ => name.parse()?,
            });
            rest = &rest[end + 1..];
            continue;
        }
        parsed.push(KeyEvent::new(match c {
            '\n' => KeyCode::Enter,
            '\t' => KeyCode::Tab,
            c => KeyCode::Char(c),
        }));
        rest = &rest[c.len_utf8()..];
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_key_sequences() {
        let keys = parse_keys("ia<esc><C-w>v<lt>\n").unwrap();
        assert_eq!(
            keys,
            [
                KeyEvent::new(KeyCode::Char('i')),
                KeyEvent::new(KeyCode::Char('a')),
                KeyEvent::new(KeyCode::Escape),
                KeyEvent::ctrl('w'),
                KeyEvent::new(KeyCode::Char('v')),
                KeyEvent::new(KeyCode::Char('<')),
                KeyEvent::new(KeyCode::Enter),
            ]
        );
        assert!(parse_keys("a<esc").is_err());
        assert!(parse_keys("<nope>").is_err());
    }

    fn parse(s: &str) -> KeyEvent {
        s.parse().unwrap()
    }

    #[test]
    fn parses_helix_notation() {
        assert_eq!(parse("C-g"), KeyEvent::ctrl('g'));
        assert_eq!(parse("x"), KeyEvent::new(KeyCode::Char('x')));
        assert_eq!(parse("esc"), KeyEvent::new(KeyCode::Escape));
        assert_eq!(parse("F5"), KeyEvent::new(KeyCode::F(5)));
        assert_eq!(parse("S-a"), KeyEvent::new(KeyCode::Char('A')));
        let key = parse("C-A-ret");
        assert_eq!(key.code, KeyCode::Enter);
        assert!(key.modifiers.ctrl && key.modifiers.alt);
        assert_eq!(parse("C--").code, KeyCode::Char('-'));
        assert_eq!(parse("A-gt").code, KeyCode::Char('>'));
        assert!("C-nope".parse::<KeyEvent>().is_err());
        assert!("F99".parse::<KeyEvent>().is_err());
    }
}
