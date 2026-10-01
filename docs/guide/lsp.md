# LSP

nib talks to language servers through the bundled `lsp` plugin. The core knows nothing about the protocol; the plugin starts the server as an external process and exchanges messages with it, so it needs the `process` permission (it declares that itself).

You get:

- **Diagnostics.** The range is underlined, the message is shown at the end of the line, and the status line counts errors, warnings, and so on.
- **Hover.** The documentation of the symbol under the cursor, in a popup.
- **Go to definition.**
- **Completion.**

## Install a server

nib does not install servers. Put the server's command on your `PATH`. These are set up by default:

| Language | Server |
|----------|--------|
| Rust | `rust-analyzer` |
| Go | `gopls` |
| Python | `pyright-langserver --stdio` |

A server is started the first time you open a file of its language, and there is one per language. Its root is the directory you ran nib from. If it cannot start, nib says so once for that language and does not try again until you restart nib.

## Using it

In the helix base:

| Key | What it does |
|-----|--------------|
| `space k` | Hover |
| `gd` | Go to the definition |
| `C-x` in insert mode | Complete |

In the vim base, `gd` or `C-]` goes to the definition, `K` shows the hover, and `C-n` / `C-p` in insert mode open completion. In the emacs base, `M-.` goes to the definition and `C-M-i` completes.

Every feature is also a command, so any base can call it by name from the command line (`:lsp.hover`), or bind it to a key:

| Command | What it does |
|---------|--------------|
| `lsp.hover` | Show what the server says about the symbol at the cursor |
| `lsp.definition` | Go to the definition |
| `lsp.complete` | Open the completion list |
| `lsp.diagnostics` | List the diagnostics of the open files in a split called `*diagnostics*`, one per line as `path:line:column: severity: message`. It updates as they change. Press `ret` on a line to jump to it in the view above, and `q` to close the list. |
| `lsp.status` | Report each server as `rust ready`, `starting`, or `stopped`. Not bound to a key by default. |

For example, to put the status under `space l` in the helix base:

```toml
# plugins/helix.toml
[settings.keys.normal]
space = { l = "lsp.status" }
```

### Completion

The list opens when you press the completion key. It also opens by itself: when you have typed two or more identifier characters, or `.` or `::`, and then paused for about 150 ms. It narrows as you type, ignoring case.

In the helix base, choose with `C-n` / `down` and `C-p` / `up`, and accept with `tab` or `ret`. Any other key, `esc` included, closes the list and then does its usual job. Other bases have their own keys for choosing; they are in their key lists.

Snippets are inserted as plain text: the placeholders such as `$1` and `${1:name}` are removed and only the words are kept.

## Configuring servers

Servers are configured in `plugins/lsp.toml`, under `[settings.servers.<language>]`. The language name is the name from the [language](languages.md) plugin, such as `rust`.

```toml
# ~/.config/nib/plugins/lsp.toml
[settings.servers.python]
command = ["pylsp"]

[settings.servers.rust]
command = ["rust-analyzer"]
```

A language written here replaces the default for that language, and one with no entry here keeps its default. A language that has no default needs an entry to get a server at all.

To turn LSP off, set `enabled = false` in `plugins/lsp.toml` (see [Configuration](configuration.md#pluginsnametoml)).

## Notes

- A server needs time to load a project the first time. Until it has, hover and completion may answer "no hover information" or "no completions". Try again after a few seconds. A file that is not part of a project the server knows (a Rust file outside a Cargo package, say) gets fewer answers.
- Positions are counted the way the server prefers: in UTF-8 if it supports that, otherwise in UTF-16. With UTF-16 servers, nib sends the whole file after every change instead of the changed parts, which can be slow on very large files.
- Diagnostics arrive both ways servers send them. rust-analyzer reports type errors when nib asks for them, a short time after you stop typing, and reports `cargo check` results on its own, and nib shows both.
- The server's standard error is discarded. If the server exits, nib shows a message and treats it as stopped.
