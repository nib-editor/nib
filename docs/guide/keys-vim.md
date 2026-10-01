# Keys (vim base)

The `vim` base aims at Vim's feel: what your fingers remember should work as expected. Where Vim and Neovim differ, nib follows Neovim, because it is compared against `nvim --clean` in the tests. Use it with `base = "vim"` in `[core]` (see [Configuration](configuration.md#choosing-a-base)).

This page lists what exists. It is a large subset of Vim, not all of it; what is missing is at the end.

`C-x` is Ctrl-x. The core menu is on `C-g` (in Vim, `C-g` shows the file info).

## Modes

Normal, insert, replace, and visual (characters, lines, and blocks). The mode is shown at the left of the status line, and pending keys (such as `2d`) and macro recording (`recording @q`) at the right. A paste from the terminal goes in as typed in insert mode, as `p` in normal mode, and replaces the selection in visual mode.

## Motions

| Keys | What they do |
|------|--------------|
| `h` `j` `k` `l`, arrows | Left, down, up, right. `j` and `k` keep the column. |
| `w` `W` `b` `B` `e` `E` `ge` `gE` | Words. With an operator, they behave as in Vim (`dw` stays on the line, `cw` acts like `ce`). |
| `0` `^` `$` `g_` `\|` | Within the line |
| `gg` `G` `+` `-` `_` `Enter` | To a line. `{count}G` is a line number, `{count}%` a percentage of the file. |
| `f` `t` `F` `T` `;` `,` | Find a character on the line |
| `%` | Matching bracket |
| `{` `}` | Paragraphs (blank lines) |
| `H` `M` `L` | Top, middle, bottom of the screen |
| `/` `?` `n` `N` `*` `#` `g*` `g#` | Search. It wraps at the ends and says so. |
| `'a` `` `a `` `''` | Marks |

## Operators and text objects

| Keys | What they do |
|------|--------------|
| `d` `c` `y` `>` `<` `g~` `gu` `gU` | Operators. Doubled they work on the line (`dd`, `g~~`, `gUgU`). |
| `iw` `aw` `iW` `aW` | Words |
| `ip` `ap` | Paragraphs |
| `i(` `a(` `ib` `i{` `iB` `i[` `i<` | Brackets. A count goes outward. |
| `i"` `a"` `i'` ``i` `` | Quotes (within the line) |

## Other normal-mode keys

| Keys | What they do |
|------|--------------|
| `x` `X` `s` `S` `C` `D` `Y` | `dl`, `dh`, `cl`, `cc`, `c$`, `d$`, `y$` |
| `p` `P` | Paste: lines, characters, blocks |
| `J` `gJ` | Join lines |
| `r` `R` `~` | Replace |
| `i` `a` `I` `A` `gI` `o` `O` `gi` | Enter insert mode. A count repeats what you type. |
| `u` `C-r` `.` | Undo, redo, repeat |
| `v` `V` `C-v` `gv` | Visual mode |
| `"x` | Pick a register |
| `q` `@` `@@` `@:` | Macros, and repeat the last `:` command |
| `m` | Set a mark |
| `C-o` `C-i` (`Tab`) | Jump list |
| `zz` `zt` `zb` `C-e` `C-y` `C-d` `C-u` `C-f` `C-b` | Scrolling |
| `C-a` `C-x` | Add to / subtract from a number (decimal, `0x`, `0b`) |
| `gd` `C-]` / `K` | Go to definition / hover (needs a language server; see [LSP](lsp.md)) |
| `C-w` then `v` `s` `w` `h` `j` `k` `l` `q` `c` `o` | Split and move between windows |
| `ZZ` `ZQ` `&` | `:x`, `:q!`, `:&&` |
| `:` | Command line |

Visual mode: grow the selection with motions and text objects, and swap the ends with `o` and `O`. `d x X D y Y c s C S R > < ~ u U g~ gu gU J gJ r p P I A :` work on it. In block mode, `I` and `A` insert on every line.

Insert mode: characters, `Enter` (keeps the indent), `Tab`, `Backspace`, `Delete`, arrows, `Home`, `End`; `C-w` and `C-u` delete a word or the line (stopping once where insert began); `C-t` and `C-d` indent and unindent; `C-r` inserts a register; `C-v` inserts the next key as typed; `C-o` runs one normal-mode command; `C-n` / `C-p` open LSP completion; `Esc`, `C-[`, and `C-c` leave.

## Registers

`"` (unnamed), `a`-`z` (`A`-`Z` append), `0` (last yank), `1`-`9` (shifted line deletes), `-` (small deletes), `_` (discard), and `+` and `*` (the system clipboard).

## Command line

`:` accepts Vim's addresses (`%`, `.`, `$`, numbers, `'a`, `/pat/`, `?pat?`, `+n`, `-n`, `,`, `;`) and these commands:

| Command | What it does |
|---------|--------------|
| `:{n}` | Go to a line |
| `:s/pat/rep/[gine]` `:&` `:&&` | Substitute. `&` and `\0` are the match, `\1`-`\9` groups, `\r` a newline. |
| `:d` `:y` | Delete or yank lines, with a register and count |
| `:j[!]` `:>` `:<` `:m` `:t` `:co` | Join, indent, move, copy |
| `:g/pat/cmd` `:g!` `:v` | Run a command on matching (non-matching) lines |
| `:norm[al] keys` | Run normal-mode keys on each line |
| `:w [path]` `:wq` `:x` `:q[!]` `:qa` `:wa` `:up` | Save and quit |
| `:e path` `:sp` `:vs` `:on` `:clo` `:bn` `:bp` `:bd[!]` | Files and windows |
| `:reg` | Show the registers in a split called `*registers*` (`q` closes it) |
| `:noh` | Does nothing (there is no search highlight) |
| `:config` `:config-reload`, or any command by name such as `:lsp.definition` | nib's own commands |

Patterns are written the Vim way (magic) and converted for nib's regex engine. `\(`, `\|`, `\+`, `\=`, `\{n,m}`, `\{-}`, `\<`, `\>`, `\s`, `\d`, `\w`, `\a`, `\l`, `\u`, `\x`, `\h`, `\_s`, `\_.`, `\v`, `\V`, and `\c` are understood. In the command line, `C-w`, `C-u`, `C-b`, `C-e`, `C-h`, `C-r`, up / down and `C-p` / `C-n` (history), and `Tab` (command names) work.

## Leader

`<leader>` is `Space` by default. Plugins put their keys there: `Space f` for the file picker, `Space e` for all files, `Space ?` for commands, and `Space k` for the LSP hover. Change it with `leader` in `plugins/vim.toml` (`leader = ","`); then Space moves right as in Vim.

## Changing keys

```toml
# ~/.config/nib/plugins/vim.toml
leader = ","

[settings.keys.normal]
Y = "yy"
"C-s" = ":w<CR>"
"C-p" = "picker.files"
```

The sections are `[settings.keys.normal]`, `[settings.keys.insert]`, and `[settings.keys.visual]`. The right side is a command name, or keys written as in a Vim mapping (`<CR>`, `<Esc>`, `<C-w>`, `<lt>`). The keys on the right are not remapped again (like `noremap`).

## Differences from Vim

- The indent width comes from nib's `indent` setting (for `>>`, `:>`, `C-t`, `Tab`), not from `shiftwidth` and `expandtab`.
- `:s` has no `\u`, `\U`, `\l`, `\L`, `\zs`, `\ze`, or `\%` atoms.
- No `hlsearch` or `incsearch`.
- `.` does not repeat visual-mode operations.
- Macros are kept as key sequences, so you cannot paste one with `"qp` and edit it.
- No folds, `=`, `gq`, `!`, tag objects (`it`, `at`), sentence motions, the change list (`g;`), or `:set`.
- `C-n` and `C-p` in insert mode open the LSP completion, not word completion.
