# Keys (nano base)

The `nano` base follows GNU nano: no modes, and what you type goes in at the cursor. Use it with `base = "nano"` in `[core]` (see [Configuration](configuration.md#choosing-a-base)).

Notation: `^X` is Ctrl-x, and `M-x` is Meta (Alt) x. As in nano, `Esc` followed by a key is the same as Meta with it.

## Keys

| Keys | What they do |
|------|--------------|
| `^G` | The core menu (nano's help key is used by nib for this) |
| `^X` | Exit. If there are unsaved changes, it asks `Save modified buffer?`; answer `Y`, `N`, or `^C`. |
| `^O` | Write out. It asks for the file name with the current path filled in; a different name saves there and becomes the file's name. |
| `^R` | Insert a file (from the working directory) |
| `^W` / `M-W` / `M-Q` | Search (plain text, not a regexp) / next / previous. It wraps at the ends. |
| `^\` | Replace: asks what to find and what to put, replaces every match, and says how many |
| `^K` / `^U` / `M-6` | Cut the line (or the selection) / paste / copy |
| `M-A` | Set or clear the mark. From the mark to the cursor is the selection, and moving extends it. |
| `M-U` / `M-E` | Undo / redo |
| `^C` | Show the cursor position (line, column, character) |
| `^_` (`^/`) | Go to a line (and column): `Enter line number, column number:` |
| `^T` | The command list (nano's Execute) |
| `^A` `^E`, `Home` `End` | Line start / end |
| `^P` `^N` `^B` `^F`, arrows | Up, down, left, right |
| `^Y` `^V`, `PageUp` `PageDown` | A screen up / down |
| `^Space` `M-Space`, `^→` `^←` | Next / previous word |
| `M-\` `M-/` | Start / end of the file |
| `M-]` | Matching bracket |
| `^H` `Backspace`, `^D` `Delete` | Delete the character before / after the cursor, or the selection |
| `M-X` | Show or hide the two lines of key hints at the bottom (shown at first) |

`^K` pressed repeatedly adds the cut lines to one buffer. There is a single cut buffer. Typed characters that follow each other are one undo step. Pasting from the terminal inserts at the cursor, even with the mark set.

In a prompt (such as `^W`), `^C` cancels, and `^A`, `^E`, `^B`, `^F`, `^H`, and `^D` edit; `^K` clears the line. When a list is open (completion, a picker), choose with the up and down arrows or `^P` / `^N`, and accept with `Tab` or `Enter`.

## Leader

Plugins put their keys under the `M-` keys that nano itself does not use. A plugin's `f` is `M-F` (the file picker), and a two-key sequence `c d` is `M-C` and then `d`. The keys nano uses (`M-A`, `M-E`, `M-Q`, `M-U`, `M-W`, `M-X`, `M-6`, `M-]`, `M-\`, `M-/`, `M-Space`) are never taken, so a plugin key on one of those letters is not reachable here.

## Changing keys

```toml
# ~/.config/nib/plugins/nano.toml
[settings.keys.global]
"C-s" = "writeout"
"C-q" = "exit"
```

The right side is a command name, or the name of a nano function as in nanorc's `bind`: `exit`, `writeout`, `insert`, `whereis`, `replace`, `cut`, `paste`, `copy`, `mark`, `undo`, `redo`, `location`, `gotoline`, `execute`, and so on.

## Differences from nano

- `^G` is nib's core menu, not the help.
- `^T` lists nib's commands instead of running an external command.
- `^\` replaces everything at once, without asking about each match.
- No `^J` (justify), `M-#` (line numbers), or spell checking.
- No title bar at the top; the file name is in the status line.
