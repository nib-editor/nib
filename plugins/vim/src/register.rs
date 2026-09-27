//! vim's registers: the unnamed one, `a` to `z` (`A` to `Z` add to them),
//! `0` for the last yank, `1` to `9` for deleted lines, `-` for small
//! deletions, `_` that keeps nothing, and `+` and `*` for the clipboard.

use std::collections::BTreeMap;

use nib_plugin::nib::plugin::clipboard;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    Chars,
    /// Whole lines, each ending with a line break.
    Lines,
    /// A block of the same columns on each line.
    Block,
}

#[derive(Clone, Debug)]
pub struct Value {
    pub text: String,
    pub shape: Shape,
}

#[derive(Default)]
pub struct Registers {
    named: BTreeMap<char, Value>,
    /// The register `"` stands for: the last one written.
    unnamed: Option<char>,
}

/// Why text went into a register.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Why {
    Yank,
    /// A deletion; `big` for lines, or text over lines, which go to `1`.
    Delete {
        big: bool,
    },
}

impl Registers {
    /// Stores `value` in `name`, or in the unnamed register and the
    /// numbered or small-delete one as vim does.
    pub fn store(&mut self, name: Option<char>, value: Value, why: Why) -> Result<(), String> {
        match name {
            Some('_') => return Ok(()),
            Some('+' | '*') => return clipboard::set(&value.text),
            Some(c @ 'A'..='Z') => {
                let lower = c.to_ascii_lowercase();
                let value = match self.named.remove(&lower) {
                    Some(old) => append(old, value),
                    None => value,
                };
                self.named.insert(lower, value);
                self.unnamed = Some(lower);
            }
            Some(c @ ('a'..='z' | '0'..='9' | '-')) => {
                self.named.insert(c, value.clone());
                self.unnamed = Some(c);
                if let Why::Delete { big: true } = why {
                    self.shift(value);
                }
            }
            Some(other) => return Err(format!("E354: Invalid register name: '{other}'")),
            None => {
                let at = match why {
                    Why::Yank => '0',
                    Why::Delete { big: true } => {
                        self.shift(value.clone());
                        '1'
                    }
                    Why::Delete { big: false } => '-',
                };
                self.named.insert(at, value);
                self.unnamed = Some(at);
            }
        }
        Ok(())
    }

    /// Moves `1` to `8` down a place, and puts `value` in `1`.
    fn shift(&mut self, value: Value) {
        for n in (1..9).rev() {
            let from = char::from_digit(n, 10).unwrap_or('1');
            let to = char::from_digit(n + 1, 10).unwrap_or('9');
            if let Some(v) = self.named.remove(&from) {
                self.named.insert(to, v);
            }
        }
        self.named.insert('1', value);
    }

    pub fn get(&self, name: Option<char>) -> Result<Option<Value>, String> {
        match name {
            None | Some('"') => Ok(self.unnamed.and_then(|c| self.named.get(&c)).cloned()),
            Some('+' | '*') => clipboard::get().map(|text| {
                let shape = if text.ends_with('\n') {
                    Shape::Lines
                } else {
                    Shape::Chars
                };
                Some(Value { text, shape })
            }),
            Some('_') => Ok(None),
            Some(c) => Ok(self.named.get(&c.to_ascii_lowercase()).cloned()),
        }
    }
}

/// `old` with `new` after it, as `"Ay` does.
fn append(old: Value, new: Value) -> Value {
    match (old.shape, new.shape) {
        (Shape::Lines, _) | (_, Shape::Lines) => {
            let mut text = old.text;
            if !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(&new.text);
            if !text.ends_with('\n') {
                text.push('\n');
            }
            Value {
                text,
                shape: Shape::Lines,
            }
        }
        _ => Value {
            text: old.text + &new.text,
            shape: old.shape,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(text: &str) -> Value {
        Value {
            text: text.into(),
            shape: Shape::Chars,
        }
    }

    #[test]
    fn deletions_and_yanks_go_where_vim_puts_them() {
        let mut regs = Registers::default();
        regs.store(None, chars("yanked"), Why::Yank).unwrap();
        regs.store(None, chars("word"), Why::Delete { big: false })
            .unwrap();
        let lines = Value {
            text: "a line\n".into(),
            shape: Shape::Lines,
        };
        regs.store(None, lines, Why::Delete { big: true }).unwrap();
        regs.store(None, chars("x\ny"), Why::Delete { big: true })
            .unwrap();
        let text = |regs: &Registers, c| regs.get(Some(c)).unwrap().unwrap().text;
        assert_eq!(text(&regs, '0'), "yanked");
        assert_eq!(text(&regs, '-'), "word");
        assert_eq!(text(&regs, '1'), "x\ny");
        assert_eq!(text(&regs, '2'), "a line\n");
        assert_eq!(regs.get(None).unwrap().unwrap().text, "x\ny");
        regs.store(Some('a'), chars("one"), Why::Yank).unwrap();
        regs.store(Some('A'), chars(" two"), Why::Yank).unwrap();
        assert_eq!(text(&regs, 'a'), "one two");
        assert_eq!(regs.get(None).unwrap().unwrap().text, "one two");
        regs.store(Some('_'), chars("gone"), Why::Yank).unwrap();
        assert_eq!(regs.get(None).unwrap().unwrap().text, "one two");
    }
}
