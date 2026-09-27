# Contributing

nib is at an early, experimental stage. Design discussion in issues is more useful than feature requests right now.

## Development

Plugins are built for `wasm32-wasip2`, so install that target for the toolchain you use:

```sh
rustup target add wasm32-wasip2
```

Building the core needs [CMake](https://cmake.org/), which wasmtime's C API uses; tree-sitter loads grammars through it. `cargo xtask build-plugins` downloads grammars with `curl`.

The Go SDK's test plugin needs [TinyGo](https://tinygo.org/) 0.42 or later (with Go 1.25 to 1.27), [binaryen](https://github.com/WebAssembly/binaryen)'s `wasm-opt`, and [wasm-tools](https://github.com/bytecodealliance/wasm-tools). Without TinyGo, `cargo xtask build-plugins` skips it and its test passes without running. After changing `api/wit/`, copy it to `sdk/go/wit/deps/nib-plugin/` and run `go generate` in `sdk/go`.

Build the plugins before building or testing the editor. `nib` embeds the standard plugins, and the core's tests run them:

```sh
cargo xtask build-plugins
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run --release -p nib-editor -- FILE
```

`target/debug` keeps the artifacts of earlier builds when versions or dependencies change, and grows to many gigabytes over time. `cargo clean --profile dev` removes it and keeps release builds and the built plugins.

The plugins live in their own workspace, so check them with `--manifest-path plugins/Cargo.toml`: clippy with `--target wasm32-wasip2`, and their unit tests natively. CI runs the same checks.

```sh
cargo fmt --all --manifest-path plugins/Cargo.toml
cargo clippy --manifest-path plugins/Cargo.toml --workspace --target wasm32-wasip2 -- -D warnings
cargo test --manifest-path plugins/Cargo.toml --workspace
```

## Licensing of contributions

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in nib by you, as defined in the Apache-2.0 license, shall be dual licensed as in [README](README.md#license), without any additional terms or conditions.

## Code from other projects

Some code may be adapted from [Helix](https://github.com/helix-editor/helix), which is licensed under MPL-2.0. MPL-2.0 is file-level copyleft, so:

- Prefer reading Helix to understand an approach and then writing it yourself. Copy or port code only when it is clearly worth it.
- Files containing code adapted from Helix stay under MPL-2.0. Keep the original copyright notice and add an MPL-2.0 header to the file.
- Put adapted code in its own files. Pasting MPL code into an existing MIT OR Apache-2.0 file makes that file MPL-2.0 too.
- List every such file in [THIRD_PARTY.md](THIRD_PARTY.md) with the upstream path and commit it came from.
- Update the `license` field of the crate that contains the file to `(MIT OR Apache-2.0) AND MPL-2.0`.
- Do not mix MPL-2.0 code into the plugin SDKs (`sdk/`) or the API definitions (`api/`). These must stay MIT OR Apache-2.0 so plugin authors are not constrained.
