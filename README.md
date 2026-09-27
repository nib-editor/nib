# nib

A modal text editor where everything — including the default keymap — is a WebAssembly plugin.

> [!WARNING]
> nib is an early experiment. It is not usable as a daily editor yet, and the plugin API will change without notice.

## Idea

- **Small core.** The core owns buffers, selections, rendering, and the plugin host. Nothing else.
- **Plugins in any language.** Plugins are WebAssembly components built against a single API definition (WIT). Write them in Rust, Go, or anything that targets the component model.
- **Defaults are plugins too.** The standard feature set, starting with a Helix-style keymap, is built with the same public API that third-party plugins use.

## Status

What works today, all as plugins on the same public API:

- A Helix-style keymap (`helix`), with a file picker (`picker`, `Space f`), split views (`Ctrl-w`), and the system clipboard (`Space y` / `Space p`)
- Other editors' ways of editing to choose from with `base` in `config.toml`: nano's (`nano`), Vim's (`vim`, checked key for key against Neovim), and Emacs's (`emacs`, checked against GNU Emacs)
- Syntax highlighting and text objects with tree-sitter for Bash, Go, JSON, Markdown, Python, Rust, TOML, and YAML (one data-only plugin per language), with languages inside others: the code blocks and inline elements of Markdown, and the Markdown of Rust doc comments
- LSP diagnostics, hover, go to definition, and completion (`lsp`); rust-analyzer, gopls, and pyright are set up by default
- Installing plugins from GitHub releases, URLs, or by name
- Plugin SDKs for Rust and Go, with a template, a build command, and tests that run without a terminal

`Ctrl-g` opens the core menu and stops a plugin that hangs; it is the one key no plugin can take.

## Installing

There are no prebuilt binaries yet; they come with 1.0.0. Build and install `nib` from source, with Rust and its `wasm32-wasip2` target, CMake, and `curl`:

```sh
rustup target add wasm32-wasip2
cargo xtask build-plugins      # the standard plugins, which nib embeds
cargo install --path tui       # installs `nib`
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for the tests and the rest of the toolchain.

## Using

```sh
nib FILE...
nib config init                # write commented settings files to start from
nib config path                # show where the settings are
```

Plugins made by others are installed as prebuilt `.nib.tar.gz` archives. nib shows the capabilities a plugin asks for and asks before installing it:

```sh
nib plugin search [WORD]       # find plugins in the index
nib plugin add wordcount       # install by name from the index, or wordcount@v0.1.0
nib plugin add owner/repo      # or from a GitHub release, a URL, or a file
nib plugin update
nib plugin list
nib plugin remove wordcount
```

To write one, start from a template that builds and passes its tests, and read the [plugin guide](sdk/README.md):

```sh
nib plugin new NAME [--go]     # a plugin in Rust, or in Go
nib plugin build               # writes plugin.wasm
nib plugin test                # runs tests/*.toml without a terminal
nib --plugin .                 # try it
```

## Related repositories

- [nib-editor/plugins](https://github.com/nib-editor/plugins): the index that `nib plugin search` and `nib plugin add NAME` read. Add a plugin to it by pull request.
- [nib-editor/plugin-example](https://github.com/nib-editor/plugin-example): an example plugin in Go with a release workflow, to start a plugin from.

## Layout

```
core/      editor core (buffers, selections, rendering, plugin host)
tui/       terminal frontend, builds the `nib` binary
api/       plugin API definitions (WIT) — the single source of truth
sdk/       plugin SDKs per language
plugins/   standard plugins
docs/      design documents
bench/     latency benchmark against other editors
```

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. Plugin SDKs are under the same terms, so plugin authors are free to choose any license for their plugins.

See [CONTRIBUTING.md](CONTRIBUTING.md) for how contributions are licensed.
