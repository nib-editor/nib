//! Reading the keys of a normal or visual mode command, as vim reads them:
//! `["x][count]` then a command, or an operator, a count, and a motion or
//! text object. Keys are kept until they make a whole command.

use base_kit::doc::FindKind;
use nib_plugin::nib::plugin::types::{KeyCode, KeyEvent, Modifiers};

#[derive(Clone, Debug, PartialEq)]
pub enum Motion {
    Left,
    Right,
    Down,
    Up,
    /// `w`, `W` (big), `e`, `b`, `ge`.
    WordStart(bool),
    WordEnd(bool),
    WordBack(bool),
    WordEndBack(bool),
    LineStart,
    FirstNonBlank,
    LineEnd,
    LastNonBlank,
    /// `+` and Enter, `-`, `_`.
    NextLine,
    PrevLine,
    CurrentLine,
    /// `gg` and `G`: line `count`, or the first or last line.
    FileStart,
    FileEnd,
    /// `|`
    Column,
    Find(FindKind, char),
    /// `;`, and `,` reversed.
    RepeatFind(bool),
    /// `%`
    Bracket,
    ParagraphForward,
    ParagraphBack,
    /// `H`, `M`, `L`.
    ScreenTop,
    ScreenMiddle,
    ScreenBottom,
    /// `n`, and `N` reversed.
    SearchNext(bool),
    /// `*`, `#`, `g*`, `g#`.
    Star {
        forward: bool,
        whole: bool,
    },
    /// `/` and `?`: asks for the pattern.
    SearchPrompt {
        backward: bool,
    },
    /// What `/` and `?` became once the pattern was typed.
    Search {
        backward: bool,
        pattern: String,
    },
    /// `'a` and `` `a ``.
    Mark {
        name: char,
        exact: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Delete,
    Change,
    Yank,
    Indent,
    Unindent,
    Lower,
    Upper,
    Toggle,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Motion(Motion),
    /// `iw`, `a(`, and so on: `around` for `a`.
    Object {
        around: bool,
        c: char,
    },
    /// The operator twice, as `dd`: whole lines.
    Line,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InsertAt {
    /// `i`
    Before,
    /// `a`
    After,
    /// `I`
    FirstNonBlank,
    /// `gI`
    LineStart,
    /// `A`
    LineEnd,
    /// `o` and `O`
    Below,
    Above,
    /// `gi`: where insert mode was left.
    Last,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VisualKind {
    Chars,
    Lines,
    Block,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scroll {
    /// `zz`, `zt`, `zb`.
    Center,
    Top,
    Bottom,
    /// Ctrl-e and Ctrl-y.
    Lines(i32),
    /// Ctrl-d and Ctrl-u.
    HalfPage(i32),
    /// Ctrl-f and Ctrl-b.
    Page(i32),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Move(Motion),
    Operate(Op, Target),
    /// In visual mode: a text object to select.
    Select {
        around: bool,
        c: char,
    },
    /// In visual mode: an operator, or another key on the selection.
    OnSelection(char),
    /// `gJ` and the like: two-key commands on the selection.
    OnSelectionG(char),
    /// `r` in visual mode.
    ReplaceSelection(char),
    DeleteChar,
    DeleteCharBack,
    Substitute,
    SubstituteLine,
    ChangeToEnd,
    DeleteToEnd,
    YankToEnd,
    Put {
        before: bool,
    },
    Join {
        spaces: bool,
    },
    Replace(char),
    ReplaceMode,
    Insert(InsertAt),
    Undo,
    Redo,
    Repeat,
    ToggleCase,
    Visual(VisualKind),
    Reselect,
    CommandLine,
    SetMark(char),
    Record(char),
    Play(char),
    JumpBack,
    JumpForward,
    Scroll(Scroll),
    /// Ctrl-a, and Ctrl-x negated.
    Increment(i64),
    Definition,
    Hover,
    Window(char),
    WriteQuit,
    QuitForce,
    RepeatSubstitute,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cmd {
    pub register: Option<char>,
    pub count: Option<u64>,
    pub action: Action,
}

impl Cmd {
    pub fn count1(&self) -> u64 {
        self.count.unwrap_or(1)
    }
}

pub enum Parsed {
    /// The keys so far start a command; more are needed.
    Need,
    /// The keys make no command.
    Bad,
    Done(Cmd),
}

fn plain(key: &KeyEvent) -> Option<char> {
    match key.code {
        KeyCode::Char(c) if key.modifiers.is_empty() || key.modifiers == Modifiers::SHIFT => {
            Some(c)
        }
        _ => None,
    }
}

fn ctrl(key: &KeyEvent) -> Option<char> {
    match key.code {
        KeyCode::Char(c) if key.modifiers == Modifiers::CTRL => Some(c),
        _ => None,
    }
}

/// Reads the keys of one command, in normal mode or with `visual`.
pub fn parse(keys: &[KeyEvent], visual: bool) -> Parsed {
    let mut i = 0;
    let mut register = None;
    let mut count: Option<u64> = None;
    loop {
        let Some(key) = keys.get(i) else {
            return Parsed::Need;
        };
        match plain(key) {
            Some('"') => {
                let Some(name) = keys.get(i + 1) else {
                    return Parsed::Need;
                };
                let Some(name) = plain(name) else {
                    return Parsed::Bad;
                };
                register = Some(name);
                i += 2;
            }
            Some(d @ '0'..='9') if d != '0' || count.is_some() => {
                let digit = u64::from(d.to_digit(10).unwrap_or(0));
                count = Some(count.unwrap_or(0).saturating_mul(10).saturating_add(digit));
                i += 1;
            }
            _ => break,
        }
    }
    let rest = &keys[i..];
    let done = |action| {
        Parsed::Done(Cmd {
            register,
            count,
            action,
        })
    };
    match command(rest, visual) {
        Step::Need => Parsed::Need,
        Step::Bad => Parsed::Bad,
        Step::Done(Action::Operate(op, target), inner_count) => {
            // `2d3w` deletes six words.
            let count = match (count, inner_count) {
                (Some(a), Some(b)) => Some(a.saturating_mul(b)),
                (a, b) => a.or(b),
            };
            Parsed::Done(Cmd {
                register,
                count,
                action: Action::Operate(op, target),
            })
        }
        Step::Done(action, _) => done(action),
    }
}

enum Step {
    Need,
    Bad,
    /// The action, and a count typed after an operator.
    Done(Action, Option<u64>),
}

fn command(keys: &[KeyEvent], visual: bool) -> Step {
    let Some(key) = keys.first() else {
        return Step::Need;
    };
    let second = keys.get(1);
    let arg = || second.and_then(plain);
    let with_arg = |make: &dyn Fn(char) -> Action| match second {
        None => Step::Need,
        Some(k) => match plain(k) {
            Some(c) => Step::Done(make(c), None),
            None => Step::Bad,
        },
    };
    if let Some(c) = ctrl(key) {
        let action = match c {
            'r' => Action::Redo,
            'o' => Action::JumpBack,
            'i' => Action::JumpForward,
            'e' => Action::Scroll(Scroll::Lines(1)),
            'y' => Action::Scroll(Scroll::Lines(-1)),
            'd' => Action::Scroll(Scroll::HalfPage(1)),
            'u' => Action::Scroll(Scroll::HalfPage(-1)),
            'f' => Action::Scroll(Scroll::Page(1)),
            'b' => Action::Scroll(Scroll::Page(-1)),
            'a' if !visual => Action::Increment(1),
            'x' if !visual => Action::Increment(-1),
            'v' => Action::Visual(VisualKind::Block),
            'n' | 'j' => Action::Move(Motion::Down),
            'p' => Action::Move(Motion::Up),
            'h' => Action::Move(Motion::Left),
            'w' => return with_arg(&Action::Window),
            ']' => Action::Definition,
            _ => return Step::Bad,
        };
        return Step::Done(action, None);
    }
    if key.modifiers.is_empty() {
        let motion = match key.code {
            KeyCode::Left | KeyCode::Backspace => Some(Motion::Left),
            KeyCode::Right => Some(Motion::Right),
            KeyCode::Up => Some(Motion::Up),
            KeyCode::Down => Some(Motion::Down),
            KeyCode::Home => Some(Motion::LineStart),
            KeyCode::End => Some(Motion::LineEnd),
            KeyCode::Enter => Some(Motion::NextLine),
            KeyCode::Tab => return Step::Done(Action::JumpForward, None),
            KeyCode::PageDown => return Step::Done(Action::Scroll(Scroll::Page(1)), None),
            KeyCode::PageUp => return Step::Done(Action::Scroll(Scroll::Page(-1)), None),
            KeyCode::Delete => return Step::Done(Action::DeleteChar, None),
            _ => None,
        };
        if let Some(motion) = motion {
            return Step::Done(Action::Move(motion), None);
        }
    }
    let Some(c) = plain(key) else {
        return Step::Bad;
    };
    let op = match c {
        'd' => Some(Op::Delete),
        'c' => Some(Op::Change),
        'y' => Some(Op::Yank),
        '>' => Some(Op::Indent),
        '<' => Some(Op::Unindent),
        _ => None,
    };
    if let Some(op) = op {
        if visual {
            return Step::Done(Action::OnSelection(c), None);
        }
        return operator(op, &keys[1..], &[c]);
    }
    if c == 'g' {
        let Some(next) = arg() else {
            return if second.is_none() {
                Step::Need
            } else {
                Step::Bad
            };
        };
        let op = match next {
            '~' => Some(Op::Toggle),
            'u' => Some(Op::Lower),
            'U' => Some(Op::Upper),
            _ => None,
        };
        if let Some(op) = op {
            if visual {
                return Step::Done(Action::OnSelectionG(next), None);
            }
            return operator(op, &keys[2..], &['g', next]);
        }
        let action = match next {
            'g' => Action::Move(Motion::FileStart),
            'e' => Action::Move(Motion::WordEndBack(false)),
            'E' => Action::Move(Motion::WordEndBack(true)),
            '_' => Action::Move(Motion::LastNonBlank),
            'j' => Action::Move(Motion::Down),
            'k' => Action::Move(Motion::Up),
            '0' => Action::Move(Motion::LineStart),
            '*' => Action::Move(Motion::Star {
                forward: true,
                whole: false,
            }),
            '#' => Action::Move(Motion::Star {
                forward: false,
                whole: false,
            }),
            'J' if visual => Action::OnSelectionG('J'),
            'J' => Action::Join { spaces: false },
            'I' => Action::Insert(InsertAt::LineStart),
            'i' => Action::Insert(InsertAt::Last),
            'v' => Action::Reselect,
            'd' => Action::Definition,
            _ => return Step::Bad,
        };
        return Step::Done(action, None);
    }
    if let Some(step) = motion(keys) {
        return match step {
            Ok((motion, _)) => Step::Done(Action::Move(motion), None),
            Err(step) => step,
        };
    }
    if visual {
        return match c {
            'i' | 'a' => with_arg(&|obj| Action::Select {
                around: c == 'a',
                c: obj,
            }),
            'r' => with_arg(&Action::ReplaceSelection),
            'x' | 'X' | 's' | 'S' | 'C' | 'D' | 'Y' | 'R' | 'J' | 'p' | 'P' | 'o' | 'O' | 'I'
            | 'A' | '~' | 'u' | 'U' | ':' => Step::Done(Action::OnSelection(c), None),
            'v' => Step::Done(Action::Visual(VisualKind::Chars), None),
            'V' => Step::Done(Action::Visual(VisualKind::Lines), None),
            _ => Step::Bad,
        };
    }
    let action = match c {
        'x' => Action::DeleteChar,
        'X' => Action::DeleteCharBack,
        's' => Action::Substitute,
        'S' => Action::SubstituteLine,
        'C' => Action::ChangeToEnd,
        'D' => Action::DeleteToEnd,
        'Y' => Action::YankToEnd,
        'p' => Action::Put { before: false },
        'P' => Action::Put { before: true },
        'J' => Action::Join { spaces: true },
        'r' if second.is_some_and(|k| k.code == KeyCode::Enter) => {
            return Step::Done(Action::Replace('\n'), None);
        }
        'r' => return with_arg(&Action::Replace),
        'R' => Action::ReplaceMode,
        'i' => Action::Insert(InsertAt::Before),
        'a' => Action::Insert(InsertAt::After),
        'I' => Action::Insert(InsertAt::FirstNonBlank),
        'A' => Action::Insert(InsertAt::LineEnd),
        'o' => Action::Insert(InsertAt::Below),
        'O' => Action::Insert(InsertAt::Above),
        'u' => Action::Undo,
        '.' => Action::Repeat,
        '~' => Action::ToggleCase,
        'v' => Action::Visual(VisualKind::Chars),
        'V' => Action::Visual(VisualKind::Lines),
        ':' => Action::CommandLine,
        'm' => return with_arg(&Action::SetMark),
        'q' => return with_arg(&Action::Record),
        '@' => return with_arg(&Action::Play),
        '&' => Action::RepeatSubstitute,
        'K' => Action::Hover,
        'Z' => {
            return match arg() {
                None if second.is_none() => Step::Need,
                Some('Z') => Step::Done(Action::WriteQuit, None),
                Some('Q') => Step::Done(Action::QuitForce, None),
                _ => Step::Bad,
            };
        }
        'z' => {
            let scroll = match second {
                None => return Step::Need,
                Some(k) if k.code == KeyCode::Enter => Scroll::Top,
                Some(k) => match plain(k) {
                    Some('z' | '.') => Scroll::Center,
                    Some('t') => Scroll::Top,
                    Some('b' | '-') => Scroll::Bottom,
                    _ => return Step::Bad,
                },
            };
            Action::Scroll(scroll)
        }
        _ => return Step::Bad,
    };
    Step::Done(action, None)
}

/// After an operator's keys (`typed`): a count, then the operator again, a
/// text object, or a motion.
fn operator(op: Op, keys: &[KeyEvent], typed: &[char]) -> Step {
    let mut i = 0;
    let mut count: Option<u64> = None;
    while let Some(d) = keys.get(i).and_then(plain).filter(char::is_ascii_digit) {
        if d == '0' && count.is_none() {
            break;
        }
        let digit = u64::from(d.to_digit(10).unwrap_or(0));
        count = Some(count.unwrap_or(0).saturating_mul(10).saturating_add(digit));
        i += 1;
    }
    let rest = &keys[i..];
    let Some(key) = rest.first() else {
        return Step::Need;
    };
    let done = |target| Step::Done(Action::Operate(op, target), count);
    // `dd`, `g~~`, and `g~g~` work on lines.
    let last = *typed.last().unwrap_or(&' ');
    if plain(key) == Some(last) {
        return done(Target::Line);
    }
    if typed.len() == 2 && plain(key) == Some('g') {
        return match rest.get(1).and_then(plain) {
            None if rest.len() == 1 => Step::Need,
            Some(c) if c == last => done(Target::Line),
            _ => match motion(rest) {
                Some(Ok((motion, _))) => done(Target::Motion(motion)),
                Some(Err(step)) => step,
                None => Step::Bad,
            },
        };
    }
    if let Some(c @ ('i' | 'a')) = plain(key) {
        return match rest.get(1) {
            None => Step::Need,
            Some(k) => match plain(k) {
                Some(obj) => done(Target::Object {
                    around: c == 'a',
                    c: obj,
                }),
                None => Step::Bad,
            },
        };
    }
    match motion(rest) {
        Some(Ok((motion, _))) => done(Target::Motion(motion)),
        Some(Err(step)) => step,
        None => Step::Bad,
    }
}

/// A motion at the start of `keys`, and how many keys it took; `None` when
/// the first key is no motion.
fn motion(keys: &[KeyEvent]) -> Option<Result<(Motion, usize), Step>> {
    let key = keys.first()?;
    if key.modifiers.is_empty() {
        let special = match key.code {
            KeyCode::Left | KeyCode::Backspace => Some(Motion::Left),
            KeyCode::Right => Some(Motion::Right),
            KeyCode::Up => Some(Motion::Up),
            KeyCode::Down => Some(Motion::Down),
            KeyCode::Home => Some(Motion::LineStart),
            KeyCode::End => Some(Motion::LineEnd),
            KeyCode::Enter => Some(Motion::NextLine),
            _ => None,
        };
        if let Some(motion) = special {
            return Some(Ok((motion, 1)));
        }
    }
    let c = plain(key)?;
    let with_char = |make: &dyn Fn(char) -> Motion| match keys.get(1) {
        None => Err(Step::Need),
        Some(k) => match plain(k) {
            Some(c) => Ok((make(c), 2)),
            None => Err(Step::Bad),
        },
    };
    let one = |motion| Some(Ok((motion, 1)));
    match c {
        'h' => one(Motion::Left),
        'l' => one(Motion::Right),
        'j' => one(Motion::Down),
        'k' => one(Motion::Up),
        'w' => one(Motion::WordStart(false)),
        'W' => one(Motion::WordStart(true)),
        'e' => one(Motion::WordEnd(false)),
        'E' => one(Motion::WordEnd(true)),
        'b' => one(Motion::WordBack(false)),
        'B' => one(Motion::WordBack(true)),
        '0' => one(Motion::LineStart),
        '^' => one(Motion::FirstNonBlank),
        '$' => one(Motion::LineEnd),
        '+' => one(Motion::NextLine),
        '-' => one(Motion::PrevLine),
        '_' => one(Motion::CurrentLine),
        'G' => one(Motion::FileEnd),
        '|' => one(Motion::Column),
        ';' => one(Motion::RepeatFind(false)),
        ',' => one(Motion::RepeatFind(true)),
        '%' => one(Motion::Bracket),
        '}' => one(Motion::ParagraphForward),
        '{' => one(Motion::ParagraphBack),
        'H' => one(Motion::ScreenTop),
        'M' => one(Motion::ScreenMiddle),
        'L' => one(Motion::ScreenBottom),
        'n' => one(Motion::SearchNext(false)),
        'N' => one(Motion::SearchNext(true)),
        '*' => one(Motion::Star {
            forward: true,
            whole: true,
        }),
        '#' => one(Motion::Star {
            forward: false,
            whole: true,
        }),
        '/' => one(Motion::SearchPrompt { backward: false }),
        '?' => one(Motion::SearchPrompt { backward: true }),
        'f' => Some(with_char(&|c| Motion::Find(FindKind::Forward, c))),
        't' => Some(with_char(&|c| Motion::Find(FindKind::Till, c))),
        'F' => Some(with_char(&|c| Motion::Find(FindKind::Backward, c))),
        'T' => Some(with_char(&|c| Motion::Find(FindKind::TillBackward, c))),
        '\'' => Some(with_char(&|c| Motion::Mark {
            name: c,
            exact: false,
        })),
        '`' => Some(with_char(&|c| Motion::Mark {
            name: c,
            exact: true,
        })),
        'g' => match keys.get(1).and_then(plain) {
            None if keys.len() == 1 => Some(Err(Step::Need)),
            Some('g') => Some(Ok((Motion::FileStart, 2))),
            Some('e') => Some(Ok((Motion::WordEndBack(false), 2))),
            Some('E') => Some(Ok((Motion::WordEndBack(true), 2))),
            Some('_') => Some(Ok((Motion::LastNonBlank, 2))),
            Some('j') => Some(Ok((Motion::Down, 2))),
            Some('k') => Some(Ok((Motion::Up, 2))),
            Some('0') => Some(Ok((Motion::LineStart, 2))),
            _ => Some(Err(Step::Bad)),
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(text: &str) -> Vec<KeyEvent> {
        text.chars()
            .map(|c| KeyEvent {
                code: KeyCode::Char(c),
                modifiers: Modifiers::empty(),
            })
            .collect()
    }

    fn done(text: &str) -> Cmd {
        match parse(&keys(text), false) {
            Parsed::Done(cmd) => cmd,
            Parsed::Need => panic!("{text}: needs more"),
            Parsed::Bad => panic!("{text}: bad"),
        }
    }

    #[test]
    fn commands_read_as_vim_reads_them() {
        assert!(matches!(parse(&keys("d"), false), Parsed::Need));
        assert!(matches!(parse(&keys("2d3"), false), Parsed::Need));
        assert!(matches!(parse(&keys("\"a"), false), Parsed::Need));
        assert!(matches!(parse(&keys("dQ"), false), Parsed::Bad));
        let cmd = done("2d3w");
        assert_eq!(cmd.count, Some(6));
        assert_eq!(
            cmd.action,
            Action::Operate(Op::Delete, Target::Motion(Motion::WordStart(false)))
        );
        let cmd = done("\"ayy");
        assert_eq!(cmd.register, Some('a'));
        assert_eq!(cmd.action, Action::Operate(Op::Yank, Target::Line));
        assert_eq!(done("0").action, Action::Move(Motion::LineStart));
        assert_eq!(done("10j").count, Some(10));
        assert_eq!(
            done("ci(").action,
            Action::Operate(
                Op::Change,
                Target::Object {
                    around: false,
                    c: '('
                }
            )
        );
        assert_eq!(
            done("g~~").action,
            Action::Operate(Op::Toggle, Target::Line)
        );
        assert_eq!(
            done("gUgU").action,
            Action::Operate(Op::Upper, Target::Line)
        );
        assert_eq!(
            done("gUiw").action,
            Action::Operate(
                Op::Upper,
                Target::Object {
                    around: false,
                    c: 'w'
                }
            )
        );
        assert_eq!(
            done("dfx").action,
            Action::Operate(
                Op::Delete,
                Target::Motion(Motion::Find(FindKind::Forward, 'x'))
            )
        );
        assert_eq!(
            done("dgg").action,
            Action::Operate(Op::Delete, Target::Motion(Motion::FileStart))
        );
    }
}
