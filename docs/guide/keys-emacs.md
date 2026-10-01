# Keys (emacs base)

The `emacs` base follows GNU Emacs as it is with `emacs -Q`: no modes, and what you type goes in at the cursor. Use it with `base = "emacs"` in `[core]` (see [Configuration](configuration.md#choosing-a-base)).

Notation: `C-x` is Ctrl-x, `M-x` is Meta (Alt) x, `RET`, `TAB`, `DEL` (backspace), and `SPC` are the named keys. `Esc` followed by a key is the same as Meta with it. The core menu is on `F10`, where Emacs has its menu bar.

## Ideas to know

- **Point and mark.** The cursor is the point. `C-SPC` sets the mark and turns the region on; the region is the text between mark and point and is shown as the selection. `C-g` turns it off. Moving with Shift held makes a region for as long as you move.
- **Prefix arguments.** `C-u` (4, then 16, ...), a number after it, `M-0` to `M-9`, and `M--` repeat or modify the next command. While you type one, it is shown at the bottom right.
- **Kill ring.** Killed text goes on a ring of up to 120 entries, and also to the system clipboard. Consecutive kills are appended into one entry. If something outside nib is on the clipboard, `C-y` brings it in.
- **Undo.** Typed characters are grouped, up to 20 per undo step.
- **Failures** such as `End of buffer` show in the echo area and stop a keyboard macro.

## Moving

| Keys | What they do |
|------|--------------|
| `C-f` `C-b` `C-n` `C-p`, arrows | Character, line. `C-n` and `C-p` keep the column. |
| `C-a` `C-e`, `Home` `End` | Line start, end |
| `M-m` | First non-blank |
| `M-f` `M-b`, `C-<right>` `C-<left>` | Word |
| `M-a` `M-e` | Sentence |
| `M-{` `M-}`, `C-<up>` `C-<down>` | Paragraph |
| `C-M-f` `C-M-b` `C-M-u` `C-M-d` | Balanced expressions: brackets, strings, symbols |
| `C-M-a` `C-M-e` | Start / end of a function (uses the syntax tree when the language has one) |
| `M-<` `M->` | Start / end of the buffer. This sets the mark. |
| `C-v` `M-v`, `PageDown` `PageUp` | A screen |
| `C-l` `M-r` | Recenter / move to the middle, top, bottom |
| `M-g g`, `M-g c`, `M-g TAB` | Go to a line, character position, column |

## Editing

| Keys | What they do |
|------|--------------|
| `RET` | New line, keeping the indent |
| `C-j` | New line only |
| `C-o` | Insert a newline after the point |
| `TAB` | Indent relative to the previous line |
| `C-x TAB` | Indent the region |
| `C-d` `DEL` | Delete forward / backward (`DEL` deletes an active region) |
| `C-k` | Kill to the end of the line. `C-u n` kills n lines, `C-u 0` to the start. |
| `C-S-<backspace>` | Kill the whole line |
| `M-d` `M-DEL` | Kill a word forward / backward |
| `M-k` `C-M-k` | Kill a sentence / balanced expression |
| `M-z` | Kill up to and including a character |
| `C-w` `M-w` | Kill / copy the region |
| `C-y` `M-y` | Yank, and replace the yank with the previous kill |
| `C-t` `M-t` `C-x C-t` | Transpose characters, words, lines |
| `M-u` `M-l` `M-c` | Upcase, downcase, capitalize a word |
| `C-x C-u` `C-x C-l` | Upcase / downcase the region |
| `M-\` `M-SPC` `M-^` `C-x C-o` | Delete spaces, collapse them, join with the line above, collapse blank lines |
| `C-q` | Insert the next key as typed |
| `M-/` | Complete from words in the buffer |
| `C-M-i` `M-TAB` | LSP completion (needs a language server; see [LSP](lsp.md)) |
| `C-/` `C-_` `C-x u` / `C-?` `C-M-_` | Undo / redo |

### Marking

| Keys | What they do |
|------|--------------|
| `C-SPC` `C-@` | Set the mark; twice turns the region off |
| `C-u C-SPC` | Go back to the mark, cycling the mark ring |
| `C-x C-x` | Swap point and mark |
| `C-x h` `M-h` `M-@` `C-M-SPC` `C-M-h` | Mark the buffer, paragraph, word, balanced expression, function |
| `C-x SPC` | Rectangle mark mode |
| `C-g` | Turn off the region and drop pending keys |

## Searching

| Keys | What they do |
|------|--------------|
| `C-s` `C-r` | Incremental search, forward / backward. Press again for the next match; with nothing typed, `C-s` repeats the last search. `DEL` undoes a step, `C-w` adds the word at point, `C-y` adds the kill, `RET` ends it, `C-g` cancels. |
| `C-M-s` `C-M-r` | Regexp search |
| `M-%` `C-M-%` | Query replace (plain, regexp). `y`/`SPC` replace, `n`/`DEL` skip, `!` all, `.` replace and stop, `,` replace and stay, `q`/`RET` stop. |
| `M-x replace-string`, `M-x replace-regexp` | Replace without asking |

A search with no capital letters ignores case. Replacements follow the case of the match. With an active region, replacement stays inside it. Regexps are written the Emacs way (`\(`, `\)`, `\|`, `\{n,m\}`, `\<`, `\w`, `\s-`, `[[:space:]]`, ...), and replacements understand `\&` and `\1`-`\9`.

## Rectangles, registers

| Keys | What they do |
|------|--------------|
| `C-x r k` `C-x r M-w` `C-x r d` `C-x r y` | Kill, copy, delete, yank a rectangle |
| `C-x r o` `C-x r c` `C-x r t` `C-x r N` | Open, clear, replace with a string, number lines |
| `C-x r s` `C-x r i` | Region to / from a register |
| `C-x r r` | Rectangle to a register |
| `C-x r SPC` `C-x r j` | Save / jump to a position register |
| `C-x r n` `C-x r +` | Number to a register / add to it |

## Keyboard macros

| Keys | What they do |
|------|--------------|
| `C-x (` `F3`, `C-x )` `F4` | Start, end recording (`Def` shows at the bottom right) |
| `C-x e`, `F4` | Run the last macro; press `e` again to repeat. A count runs it that many times, 0 until it fails. |
| `C-x z` | Repeat the last command; `z` again repeats again |

## Files, buffers, windows

| Keys | What they do |
|------|--------------|
| `C-x C-s` / `C-x s` | Save / ask for each modified file |
| `C-x C-w` `C-x C-f` `C-x i` | Save as, open, insert a file. File names complete with `TAB`. |
| `C-x b` / `C-x <right>` `C-x <left>` | Switch buffer / next, previous |
| `C-x k` | Close the buffer |
| `C-x C-c` | Quit, asking about unsaved files |
| `C-x 2` `C-x 3` `C-x o` `C-x 0` `C-x 1` | Split below, split right, other window, close, close others |
| `C-x =` | Show the character at point and the position |
| `M-.` / `M-,` | Go to definition / back (needs a language server) |
| `M-x` | Run a command by name: Emacs names (`kill-line`) or nib's (`lsp.definition`) |
| `C-h k` | Say what a key does, in `*Help*` (`q` closes it) |

## The minibuffer

`M-x`, `C-x C-f`, and the other questions are asked in an input box. Editing keys are `C-a`, `C-e`, `C-f`, `C-b`, `C-d`, `DEL`, `M-f`, `M-b`, `M-d`, `M-DEL`, `C-k`, `C-y`, and `C-q`. `TAB` completes as far as it can and lists candidates when it cannot go further. `M-p` `M-n` and the up and down arrows walk the history. `RET` accepts (an empty answer takes the `(default ...)` shown), and `C-g` or `Esc Esc` cancels.

## Leader

`C-c` is where plugins put keys: `C-c f` opens the file picker, `C-c e` all files, `C-c ?` the command list, and `C-c k` the LSP hover.

## Changing keys

```toml
# ~/.config/nib/plugins/emacs.toml
[settings.keys.global]
"C-x" = { "C-r" = "picker.files" }
"C-z" = "undo"
```

The right side is a command name, either nib's or an Emacs name that `M-x` accepts (`forward-char`, `kill-line`). A table is a prefix, added to built-in prefixes such as `C-x`.

## Differences from Emacs

- Undo is nib's linear undo. There is no "undo the undo"; use redo (`C-?`). There is no region-limited undo.
- `TAB` is `indent-relative` for every language, in steps of nib's `indent` width.
- `M-y` works only right after a yank, with no kill-ring browsing.
- Replacement strings do not support `\,` (Lisp), and patterns have no backreferences.
- `C-h` has only `C-h k`. There is no `M-:`, `M-;`, `C-x C-e`, dired, buffer list, or `C-z`.
- Keyboard-macro counters are missing.
- In a terminal `C-/` and `C-7`, and `C-SPC` and `C-2`, arrive as the same key, so `C-7` is undo and `C-2` sets the mark, as in a terminal Emacs.
- Character widths count tab as nib's `tab-width` and East Asian full-width characters as 2.
