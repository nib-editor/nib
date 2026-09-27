# Writing nib plugins

Everything nib does beyond holding text and drawing it is a plugin: the Helix keymap, syntax languages, the file picker, the LSP client. Yours run the same way, in a sandbox, with the same API. This guide covers what the types in [`api/wit/plugin.wit`](../api/wit/plugin.wit) do not say: how nib calls a plugin, what a plugin may do, and how to test and ship one. The WIT is the reference for every function and type.

The SDKs are here, one directory per language: [`rust/`](rust) and [`go/`](go).

## Start

```sh
nib plugin new wordcount     # a plugin in Rust; add --go for Go
cd wordcount
nib plugin build             # writes plugin.wasm
nib plugin test              # runs tests/*.toml without a terminal
nib --plugin .               # try it in nib
```

The new plugin shows the number of words in the status line and answers the `wordcount.count` command. It builds and passes its tests as made, so start from there and change one thing at a time. Its `AGENTS.md` tells AI coding agents the same things as this guide, in short.

To try changes without restarting nib, press Ctrl-g, choose the plugin, and reload it from disk.

## What a plugin is

A directory with two files:

```
wordcount/
├── plugin.toml
└── plugin.wasm   # a WebAssembly component for wasm32-wasip2
```

```toml
name = "wordcount"        # also the prefix of its commands and events
version = "0.1.0"
api = "0.4"               # the nib:plugin version it is built for
capabilities = []         # what it may do beyond the editor API
events = ["buffer-opened", "buffer-changed"]
```

- `name` is lowercase letters, digits, and `-`. `buffer`, `editor`, and `view` belong to the core.
- `api` is the `major.minor` of the `nib:plugin` package in the WIT. nib loads only plugins built for its own API, and says so when one is not.
- A plugin can also provide languages without any code: tree-sitter grammars and queries, listed under `[[languages]]` (see the standard ones in [`plugins/languages/`](../plugins/languages)).

### Capabilities

A plugin gets the editor API and nothing else, unless its manifest asks:

| Capability | Gives |
|------------|-------|
| `fs-read` / `fs-write` | Reading / writing under the working directory, through WASI. `fs-read` also allows `files.walk` |
| `process` | Starting programs with `process.spawn` |
| `network` | WASI sockets and HTTP |
| `clipboard` | The system clipboard, with `clipboard.get` and `clipboard.set` |

Declared capabilities are granted without asking; undeclared ones are not, and a misspelled one keeps the plugin from loading. People see each plugin's capabilities in the Ctrl-g menu, and again when an update asks for more. Ask for what the plugin uses and nothing else.

## How nib calls a plugin

A plugin exports four functions (the `guest` interface):

| Function | When |
|----------|------|
| `init(config)` | Once, after loading. `config` is the `[settings]` table of the user's `plugins/<name>.toml`, as JSON. Register commands and push input layers here |
| `handle-key(key)` | A key reached the plugin's layer on the input stack. Return `handled`, or `pass` to hand it to the layer below |
| `run-command(name, args)` | Someone called one of its commands. `name` is as registered, without the prefix; `args` and the result are JSON |
| `on-event(event)` | An event it subscribed to happened |

Rules every plugin lives by:

- **One call at a time.** nib never calls a plugin while one of its calls is running. Events a call causes, including the plugin's own, arrive after the call returns, in the order they happened.
- **Calls are short.** `init` may take 5 seconds and every other call 1 second, unless the user raises the limit. A call that runs over is stopped, and so is one the user interrupts with Ctrl-g. Wait for things with timers and events, not in a loop.
- **Failure is cleaned up.** When a plugin traps, runs over, or is stopped, nib removes everything it made (commands, input layers, status items, panels, popups, decorations, timers, processes) and starts a new instance, calling `init` again. State in memory is lost; keep what must last in the plugin's data directory, `/data` (below). After three failures in a minute, the plugin is disabled until the user enables it again.
- **Memory is limited**, to 256 MiB by default.

## The API

Positions are UTF-8 byte offsets into a buffer. Screen rows and columns are the core's business: ask it to move vertically or scroll, and it takes wrapping, tabs, and wide characters into account.

### Editor

`editor.active-view()` is the view with the focus; `view.buffer()` the buffer it shows; `editor.buffers()` every open buffer.

- Change text with `view.apply(base-version, edits, after, undo)`. Each edit replaces `start..end` of the buffer as it was before the change, and edits must not overlap. If the buffer is no longer at `base-version`, nothing changes and `stale-version` comes back, so read `buffer.version()` in the same call.
- `undo` is `new-step` to start an undo step, or `merge` to add to the last one: an insert mode makes its first key a new step and merges the rest, so undo takes back the whole insertion.
- `next-grapheme` and `prev-grapheme` step over what shows as one character; `find` and `find-all` search with regular expressions without copying the buffer into the plugin.
- `move-vertically` and `scroll` do what `j`, `k`, and `Ctrl-d` need. `move-vertically` returns the column it aimed for; pass it back on the next move to keep the column across short lines.
- Using a buffer or view handle after it closed traps the plugin.

### Input

Keys go down a stack of layers, top first. `input.push-layer()` puts one on top for the plugin and `input.pop-layer()` takes it off; a keymap pushes its layer in `init` and keeps it. A plugin without a layer gets no keys. The menu key (Ctrl-g unless the user changes it) never reaches plugins.

### Commands

`commands.register("count", "…")` makes `wordcount.count`. Anyone can call it with `commands.call(name, args)`: users from a keymap or the command line, other plugins, and tests. Calls are synchronous and return the result. Calling into a plugin that is already in a call, such as your own, is an error.

The core's commands (arguments are JSON):

| Command | Does |
|---------|------|
| `buffer.open` | Opens `{"path": "…"}` |
| `buffer.save` | Saves the shown buffer |
| `buffer.next` / `buffer.previous` | Shows the next / previous buffer |
| `config.open` | Opens config.toml, or `{"plugin": name}`'s settings file |
| `config.reload` | Reads the settings again, as saving one of their files does |
| `buffer.close` | Closes the shown buffer; `{"force": true}` drops unsaved changes. Its handles trap from then on |
| `view.split` | Splits the view, `{"direction": "vertical"}` or `"horizontal"` |
| `view.close` / `view.only` | Closes the focused view / all others |
| `view.focus` | Moves the focus, `{"to": "next"}`, `"left"`, `"right"`, `"up"`, or `"down"` |
| `editor.quit` | Quits; `{"force": true}` drops unsaved changes |

### Events

A plugin gets the kinds of events listed under `events` in its manifest:

| Event | When |
|-------|------|
| `buffer-opened`, `buffer-saved` | A buffer was opened, saved. Buffers opened before the plugin loaded are announced after it does |
| `buffer-changed` | A buffer changed. The changes come in the order that turns the old text into the new, each with its line and column, as LSP's `didChange` wants them |
| `<plugin>.<name>` | A plugin called `events.emit(name, json)`. `helix.mode_changed` tells when the Helix keymap changes modes |
| `editor.buffer_closed` | A buffer was closed: `{"path": …}`. The core emits it |
| `editor.syntax_updated` | A buffer's syntax tree caught up with its edits: `{"path": …, "version": …}`. The core emits it |

These come to the plugin that asked for them, without being listed: `timer` (from `timers.set`), `process-output` and `process-exit` (from `process.spawn`), and `files-listed` (from `files.walk`).

Commands ask someone to do something; events tell whoever cares that something happened.

### Timers

`timers.set(ms)` sends one `timer` event after `ms` milliseconds and returns its id; `timers.cancel(id)` takes it back. To repeat, set the next one when the event comes. Timers fire between keys, never in the middle of handling one.

### UI

| Function | Shows |
|----------|-------|
| `ui.set-status(id, side, priority, line)` | An item in the status line, left or right, ordered by priority |
| `ui.show-message(text)` | A message until the next key |
| `ui.panel(lines)` | Lines at the bottom of the screen. A resource: `update` it, drop it to close it |
| `ui.popup(anchor, lines)` | A box over the text, below a position or in the corner. A resource like a panel |
| `ui.set-decorations(buffer, namespace, decorations)` | Styles on ranges of a buffer, which move with later edits |
| `ui.set-notes(buffer, namespace, notes)` | Text after the end of lines, as for diagnostics |

Text is a list of spans, each with a style named after the theme (`"keyword"`, `"ui.selection"`, `"diagnostic.error"`); the core lays it out and cuts it to fit.

### Syntax

The core parses buffers with tree-sitter, and plugins read the trees: `syntax.node-at`, `parent`, `children`, and `captures`, which runs one of the language's queries (`"textobjects"`, say) over a range. Answers are always up to date, even right after an edit in the same call. Buffers without a language answer with nothing, so fall back to working on text. Inside a language injected into another, such as a Rust code block in Markdown, answers come from that language's tree: `node-at` gives its nodes, the parent of its outermost node is the node around it outside, and `captures` runs each language's own query, so the same text objects work there.

nib parses on a thread of its own, so an answer right after an edit waits for that parse. For what a plugin reads on every key, such as the bracket to highlight, read the tree when `editor.syntax_updated` comes instead, as the Helix keymap does after keys that change text.

### And more

- `settings`: the tab width and indent of a buffer, and changing them for one buffer.
- `files.walk`: lists files under a directory, honoring `.gitignore`, on a background thread; the names arrive as `files-listed` events. Needs `fs-read`.
- `clipboard`: the system clipboard. Needs `clipboard`.
- `process.spawn`: starts a program; its output and exit arrive as events, and it is killed when the plugin stops. Needs `process`.
- `/data`: a directory of the plugin's own, through WASI's file API (`std::fs` in Rust, `os` in Go), with no capability to declare. What the plugin writes there outlives restarts and updates; other plugins cannot see it. Tests get an empty one each.

## Testing

`nib plugin test` runs the plugin in an editor without a terminal, with the standard plugins except `lsp`, and without the user's settings. Each test sets up a buffer, sends keys or calls a command, and checks what came out:

```toml
# tests/wordcount.toml

[[test]]
name = "counts again after an edit"
file = "notes.md"          # the buffer's file name, for its language; test.txt if not given
text = "#[o|]#ne\n"        # the text and the selection (below)
keys = "itwo <esc>"        # keys, sent to the Helix keymap

[test.expect]
text = "two one\n"
screen = ["2 words"]

[[test]]
name = "answers its command"
text = "one two\n"
command = "wordcount.count"
args = "{}"

[test.expect]
result = "2"
```

| In `expect` | Checks |
|-------------|--------|
| `text` | The buffer's text |
| `selections` | The text with the selections marked |
| `message` | The message line |
| `screen` | Each string is somewhere on the 80 × 24 screen |
| `absent` | No string is anywhere on the screen |
| `result` | What `command` returned, compared as JSON |
| `error` | That `command` failed, with this in its message |

Put `with = ["helix"]` at the top of a file to load only the standard plugins named. Each test runs in a new editor, in a directory of its own that holds the test's file and any `files` it lists, so a plugin that lists or reads files sees the same ones wherever the tests run.

**Waiting.** For what takes time, such as timers, programs, and language servers, give a step `wait` in milliseconds: nib runs as it would in a terminal until the expectations hold, and fails the test if they still do not when the time is up. With nothing to expect, it just lets the time pass. Steps run in order, so a test can wait for something to be ready before sending keys. A step that calls a command and expects its `result` calls it again while waiting, for commands that report a status.

```toml
with = ["helix", "rust"]
# The plugin's [settings]; {dir} is this file's directory.
settings = { servers.rust.command = ["python3", "{dir}/fake_server.py"] }

[[test]]
name = "hover shows where the cursor is"
file = "main.rs"
text = "f#[n|]# main() {}\n"
files = { "Cargo.toml" = "[package]\n" }   # more files in the working directory

[[test.step]]
command = "lsp.status"
wait = 10000
expect.result = "rust ready"

[[test.step]]
keys = "<space>k"
wait = 10000
expect.screen = ["hover at 0:1"]
```

`settings` goes at the top of a file for all its tests, or in one test. The standard plugins' tests in [`plugins/*/tests/`](../plugins) are examples, among them a fake language server in Python for the `lsp` plugin.

**Selections** are written as in Helix's tests: `#[` and `]#` around the primary range, `#(` and `)#` around others, and `|` where the cursor is. `#[h|]#ello` is a cursor on the `h`; `#[|hello]#` selects `hello` backward. Without marks, the cursor is on the first character.

**Keys** are characters as they are, and named or modified keys between `<` and `>`: `<esc>`, `<ret>`, `<tab>`, `<backspace>`, `<space>`, `<C-w>`, `<A-o>`, `<S-tab>`, `<F5>`, and `<lt>` for `<`.

A failing test prints what was expected beside what came out, and the command's exit code says whether all passed.

## Shipping

Push a tag matching `version` in `plugin.toml`; the release workflow that `nib plugin new` made builds the plugin and publishes `NAME-VERSION.nib.tar.gz`. People install it with:

```sh
nib plugin add OWNER/REPO
```

To make it findable by name, add it to [nib-editor/plugins](https://github.com/nib-editor/plugins). `nib plugin pack DIR` makes the archive by hand, with only what nib reads: the manifest, `plugin.wasm`, language files, and licenses.

## SDKs

### Rust

```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
nib-plugin = { git = "https://github.com/nib-editor/nib", tag = "sdk/rust/v0.4.2" }
```

Implement `nib_plugin::exports::nib::plugin::guest::Guest` and export it with `nib_plugin::export!(YourType)`. The API is under `nib_plugin::nib::plugin::<interface>`. Build for `wasm32-wasip2`; `nib plugin build` does it.

### Go

Built with [TinyGo](https://tinygo.org/) 0.42 or later, since Go itself cannot make components yet; see [`go/`](go). Implement `nib.Plugin` (embed `nib.Base` to skip what you do not use) and pass it to `nib.Register` in `init`. The API is in the packages `github.com/nib-editor/nib/sdk/go/nib/plugin/<interface>`. Handles to resources, including the ones events carry, are yours to drop with `ResourceDrop`.

### Versions

SDKs are versioned apart from the editor: an SDK's version changes only when the API in `api/` changes. The one exception is a change that makes the SDK unreachable at its current version, such as the Go module path moving; that gets a patch release. Their tags carry the directory: `sdk/rust/v0.4.2`, `sdk/go/v0.4.1`.
