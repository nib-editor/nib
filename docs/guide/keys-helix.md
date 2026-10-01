# Keys (helix base)

nib's default way of editing is the `helix` base: you select first, then act, as in [Helix](https://helix-editor.com). It implements a subset of Helix's keys. This page lists what is there; a key that is not listed is not implemented.

Other bases have their own keys: [vim](keys-vim.md), [emacs](keys-emacs.md), and [nano](keys-nano.md). See [Configuration](configuration.md#choosing-a-base) for choosing one.

Notation: `C-x` is Ctrl-x, `A-x` is Alt-x, and `space` is the space bar. Keys written together (`gg`) are pressed one after another.

## Always available

| Key | What it does |
|-----|--------------|
| `C-g` | Opens the core menu: plugins, settings, and quitting. It works even when a plugin is stuck, and it also stops a plugin call that is running. The key can be changed with `menu-key` in `config.toml`. |

## Modes

| Key | What it does |
|-----|--------------|
| `i` `a` | Insert before / after the selection |
| `I` `A` | Insert at the first non-blank character / at the end of the line |
| `o` `O` | Open a new line below / above and insert |
| `v` | Toggle select mode, where motions extend the selection instead of replacing it |
| `esc` | Back to normal mode (leaves select mode too) |

The mode shows at the left of the status line (`NOR`, `INS`, `SEL`).

## Counts

A number before a key repeats it: `3w`, `5j`, `10x`. In `gg`, a count is a line number: `12gg` goes to line 12.

## Motions

Motions move the cursor, and in normal mode they also select what they pass over (`w` selects the word it jumped across). In select mode they extend the selection.

| Key | What it does |
|-----|--------------|
| `h` `j` `k` `l`, arrow keys | Left, down, up, right |
| `w` `b` `e` | Next word start, previous word start, word end |
| `W` `B` `E` | The same, with words separated only by whitespace |
| `f` *c*, `t` *c* | To / until the next *c* |
| `F` *c*, `T` *c* | To / until the previous *c* |
| `gg` | First line (or line *N* with a count) |
| `ge` | Last line |
| `gh` `gl` | Start / end of the line |
| `gs` | First non-blank character of the line |
| `C-d` `C-u` | Half a page down / up |
| `C-f` `C-b` | A page down / up |
| `mm` | Matching bracket |

`f`, `t`, `F`, `T` search only the line the cursor is on, unlike Helix.

## Selecting

| Key | What it does |
|-----|--------------|
| `x` | Select the whole line. Press again to extend by a line (`3x` selects three). |
| `%` | Select the whole buffer |
| `;` | Collapse the selection to a single character |
| `,` | Keep only the main selection |
| `A-;` | Swap the cursor and the anchor of the selection |
| `C` | Copy the selection to the next line (a second cursor) |
| `s` | Select every match of a regular expression within the selection. Type the expression, then `ret`. |

nib supports several selections at once, and every command acts on all of them.

### Text objects and syntax selection

For languages with a grammar, these use the syntax tree. In a buffer without one they do nothing, except the brackets and quotes, which fall back to plain text.

| Key | What it does |
|-----|--------------|
| `mi` *x* / `ma` *x* | Select inside / around the thing at the cursor. *x* is a bracket (`(`, `[`, `{`, or the closing one) or a quote (`"`, `'`, `` ` ``), or one of: `f` function, `t` class or type, `a` parameter, `c` comment, `T` test |
| `]f` `[f` | Next / previous function |
| `]t` `[t` | Next / previous class or type |
| `]a` `[a` | Next / previous parameter |
| `]c` `[c` | Next / previous comment |
| `]T` `[T` | Next / previous test |
| `A-o` (or `A-up`) | Grow the selection to the enclosing syntax node |
| `A-i` (or `A-down`) | Shrink back to what it was before `A-o` |
| `A-n` (or `A-right`) | Select the next sibling node |
| `A-p` (or `A-left`) | Select the previous sibling node |

`mm` and the bracket objects look at the syntax tree first, so brackets inside strings and comments are not mistaken for code.

## Editing

| Key | What it does |
|-----|--------------|
| `d` | Delete the selection (and keep it in the register) |
| `c` | Delete the selection and start inserting |
| `y` | Copy the selection |
| `p` `P` | Paste after / before the selection |
| `r` *c* | Replace every selected character with *c* |
| `>` `<` | Indent / unindent the selected lines |
| `J` | Join the selected lines |
| `u` | Undo |
| `U` | Redo |
| `.` | Repeat the last insert (from `i`, `a`, `I`, `A`, `o`, `O`, or `c` through to `esc`) |

### Registers

`"` followed by a character picks the register used by the next `y`, `d`, `c`, `p`, or `P`. The default register is `"`. `_` throws the text away. `+` is the system clipboard.

`space y` copies to the system clipboard, and `space p` / `space P` paste from it; these are the same as `"+y`, `"+p`, and `"+P`. If the system has no clipboard (over SSH without a display, for example), `+` is a clipboard inside nib.

## Searching

| Key | What it does |
|-----|--------------|
| `/` `?` | Search forward / backward for a regular expression. Type it, then `ret`. |
| `n` `N` | Next / previous match |
| `*` | Use the selected text as the search pattern. Press `n` to jump. |

## Insert mode

| Key | What it does |
|-----|--------------|
| Any character | Insert it |
| `ret` | New line, keeping the indentation of the line above |
| `tab` | Insert one indent (spaces or a tab, depending on `indent`) |
| `backspace` `del` | Delete before / after the cursor |
| arrow keys | Move |
| `C-x` | Ask the language server for completions (needs a language server) |
| `esc` | Back to normal mode |

## Buffers and windows

| Key | What it does |
|-----|--------------|
| `gn` `gp` | Next / previous buffer |
| `C-w` *k* or `space w` *k* | Window commands: `v` split side by side, `s` split top and bottom, `w` next window, `h` `j` `k` `l` move to the window on that side, `q` close this window, `o` close all the others |

A split shows the same buffer twice if you like; the selections follow each other.

## Space menu

`space` opens a menu. Other plugins add keys to it, so the list depends on what is installed. With the standard plugins:

| Key | What it does |
|-----|--------------|
| `space f` | Pick a file to open, with fuzzy search. It respects `.gitignore` and skips hidden files. |
| `space e` | The same, but including hidden and ignored files |
| `space ?` | Pick a command by name |
| `space k` | Show what the language server says about the symbol at the cursor |
| `space y` `space p` `space P` | The system clipboard (above) |
| `space w` | Window commands (above) |

`gd` goes to the definition, using the language server.

In the pickers, type to filter, `up` / `down` or `C-p` / `C-n` to choose, `ret` to confirm, and `esc` to close. While you type in any prompt (a picker, `:`, `/`, `s`), `C-c` cancels, `C-w` deletes the previous word, `C-u` / `C-k` delete to the start / end, `C-a` / `C-e` jump to the start / end, and `A-b` / `A-f` move by words.

## Command line

`:` opens a command line.

| Command | What it does |
|---------|--------------|
| `:w` `:write` | Save |
| `:q` `:quit` | Quit. `:q!` quits without saving. |
| `:wq` `:x` | Save and quit |
| `:o` *path*, `:open`, `:e`, `:edit` | Open a file |
| `:bc` `:buffer-close` | Close the buffer. `:bc!` drops unsaved changes. |
| `:config` | Open `config.toml`. `:config` *name* opens `plugins/`*name*`.toml`. |
| `:config-reload` | Read the settings again |
| `:config-dir` | Open the settings directory |

Any plugin command can be run by its full name: `:lsp.definition`, or with arguments, `:buffer.open path=src/main.rs`. The arguments are `key=value` pairs separated by spaces, or JSON. While you type, the matching commands show above the line with their descriptions, and `tab` / `shift-tab` cycle through them.

If a command fails, its error is shown in the status line.

## Prefix hints

After `g`, `m`, `mi`, `ma`, `[`, `]`, `space`, and `C-w`, a popup at the bottom right lists what the next key does. It appears at once and closes on the next key.

## Changing keys

Bindings go in `plugins/helix.toml`. See [Configuration](configuration.md#remapping-keys-in-the-helix-base).

## Differences from Helix

- Only the keys above exist. Helix keys that are not listed (for example `X`, `z`, `C-o`, `C-i`, and many `space` commands) are not implemented.
- `f`, `t`, `F`, `T` stay within the current line.
- `*` sets the search pattern but does not jump.
- `Esc` in normal mode does not collapse the selection; use `;`.
