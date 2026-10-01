# Languages

nib colors text and understands its structure with [tree-sitter](https://tree-sitter.github.io). A language is a plugin that holds only data: a grammar, and queries that say how to color it and what counts as a function or a parameter. Plugins can read the resulting syntax tree, which is how the helix base selects "the function around the cursor".

## Bundled languages

| Language | Files |
|----------|-------|
| Bash | `.sh`, `.bash` |
| Go | `.go` |
| JSON | `.json` |
| Markdown | `.md`, `.markdown` |
| Python | `.py`, `.pyi` |
| Rust | `.rs` |
| TOML | `.toml` |
| YAML | `.yaml`, `.yml` |

A language is chosen by the file's extension. A file with another extension, or none, is shown as plain text.

The grammars are loaded the first time a file of that language is shown, so having many languages costs nothing at startup. The first screen is drawn before the file is parsed, in the background, and your typing never waits for the parser.

## Markdown, and languages inside languages

Some text contains another language, and nib colors each part with its own grammar:

- Markdown fenced code blocks, by the language named after the fence (or its file extension).
- Markdown front matter, as YAML or TOML.
- Markdown's inline parts: emphasis, code spans, and links.
- Rust doc comments, as Markdown. A code block without a language in a doc comment is colored as Rust, as rustdoc treats it.

The syntax-tree commands of the helix base work inside these too, so `maf` in a Rust code block inside a Markdown file selects that Rust function.

## What a language provides

| Feature | Where you see it |
|---------|------------------|
| Colors | The file's text. Colors come from the [theme](configuration.md#theme), so you can change any of them. |
| Text objects | `mi` / `ma` followed by `f`, `t`, `a`, `c`, `T`; `]f` and `[f` and their relatives; see [keys](keys-helix.md#text-objects-and-syntax-selection). |
| Syntax-aware selection | `A-o`, `A-i`, `A-n`, `A-p` in the helix base |
| Brackets | `mm` and the highlight of the bracket that matches the one under the cursor |
| Indent settings | The bundled `indent` plugin sets defaults per language: Go uses tabs, YAML and JSON use two spaces, and the rest use the `[core]` values. Change them with `[settings.languages.<name>]` in `plugins/indent.toml`. |

Language servers are separate; see [LSP](lsp.md).

## Adding a language

A language is a plugin with a `[[languages]]` table: a name, the file extensions, a tree-sitter grammar compiled to WebAssembly, and queries. You can install one made by someone else with `nib plugin add` (see [Plugins](plugins.md)), or write your own. The plugin author guide ([`sdk/README.md`](../../sdk/README.md)) says how, and the bundled languages in [`plugins/languages/`](../../plugins/languages) are examples.

Only a `highlights` query is needed for colors. `textobjects` adds the selections above, and `injections` makes nib color another language inside this one.
