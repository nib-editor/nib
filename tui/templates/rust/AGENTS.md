# Notes for AI agents

This is a plugin for nib, a modal editor whose every feature is a WebAssembly plugin. The plugin is a Rust crate built for `wasm32-wasip2`.

## Read first

- The plugin guide: https://github.com/nib-editor/nib/blob/main/sdk/README.md. How plugins get keys, commands, and events, what they may not do, and how to test them.
- The API: https://github.com/nib-editor/nib/blob/main/api/wit/plugin.wit. Every function and type, with comments. The Rust bindings in `nib_plugin` follow it: `nib_plugin::nib::plugin::<interface>::<function>`.
- `plugin.toml`: the name, which is also the prefix of the plugin's commands; the capabilities it needs; the events it gets.

## Check your work

After every change:

```sh
nib plugin build   # plugin.wasm
nib plugin test    # tests/*.toml, without a terminal
```

Add a test to `tests/` for each thing the plugin does: set `text` (with `#[x|]#` for the cursor), send `keys` or call a `command`, and `expect` the `text`, `selections`, `message`, `screen`, `absent`, or `result`. The tests run with the standard Helix keymap, so keys work as in Helix. For timers, programs, or anything else that takes time, add `wait` (milliseconds) and steps; give the plugin `settings` and the working directory `files` when it needs them. The plugin guide's "Testing" section has the details.

## Rules

- Declare every capability the plugin uses in `plugin.toml`, and no more; nib refuses the rest.
- Keep each call short: nib stops a call after one second and restarts the plugin. Use timers (`timers::set`) and events for work that waits.
- A plugin is never called again while one of its calls runs; events arrive after the call.
- Positions are UTF-8 byte offsets into the buffer. Screen rows and columns are the core's business: use its functions for vertical moves and scrolling.
