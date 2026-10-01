# Configuration

nib is configured with plain TOML files. There is no scripting language: settings are declarative, and anything that needs logic is a plugin.

If a file has a mistake, nib still starts with the defaults and shows the reason in the status line.

## Where the files are

```
~/.config/nib/
├── config.toml          # the core: [core] and [theme]
└── plugins/
    ├── helix.toml       # one file per plugin, named after the plugin
    └── lsp.toml
```

The directory is under `$XDG_CONFIG_HOME` if that is set, and under `%APPDATA%` on Windows when `HOME` is not set. Nothing is created until you ask:

| Command | What it does |
|---------|--------------|
| `nib config` (or `nib config edit`) | Opens `config.toml` in nib. If it does not exist, nib shows a commented template; the file is created when you save. |
| `nib config path` | Prints where nib reads its settings from. |
| `nib config init` | Writes a commented `config.toml` and the settings file of your base plugin. Existing files are never overwritten. |
| `nib plugin list` | Lists every plugin, whether it is enabled, where it is loaded from, and which settings file it has. |

Every line in the templates is commented out and shows the default, so you only uncomment what you change.

Plugins cannot change these files. That is deliberate: a plugin must not be able to raise its own limits or take over a reserved key.

## config.toml

```toml
[core]
tab-width = 4              # 1 to 16
indent = 4                 # spaces per indent (1 to 16), or "tab"
scroll-margin = 5          # lines kept visible above and below the cursor
base = "helix"             # the way of editing; see "Choosing a base"
menu-key = "C-g"           # opens the core menu; plugins never see this key
open-directory = "picker.directory"
plugin-timeout-ms = 1000
plugin-init-timeout-ms = 5000
plugin-memory-mib = 256
```

- `menu-key` replaces the key the base plugin uses for the core menu (`C-g` for helix). Key names are written as in Helix: `C-s`, `A-x`, `S-tab`, `ret`, `esc`, `space`.
- `open-directory` is the command that runs when you give nib a directory (`nib src/`). It is called with `{"path": "<dir>"}`. `picker.directory` lists the directory, and `picker.files` picks a file under it.
- The three `plugin-*` values are the limits for every plugin. A call that takes longer than `plugin-timeout-ms` is stopped, and a plugin's memory cannot grow past `plugin-memory-mib`. Both can be overridden per plugin (below).
- `tab-width` and `indent` are the global values. A plugin may override them for a single buffer; the bundled `indent` plugin does so per language (tabs for Go, two spaces for YAML, and so on). To change that, write `[settings.languages.<name>]` with `indent` and `tab-width` in `plugins/indent.toml`.

The old `[core] plugin-dirs` and `[plugins.<name>]` tables are no longer read. nib tells you where they moved: `path` and `[settings]` in `plugins/<name>.toml`.

### Choosing a base

A base is a plugin that provides a whole way of editing: keys, modes, and the command line. The bundled ones are `helix`, `vim`, `emacs`, and `nano`. Only one runs.

The first time nib starts without a `config.toml`, it asks which one you want. To change it later, set `base` in `[core]` and restart.

### Theme

`[theme]` overrides the built-in theme, which uses the terminal's own 16 colors, so it follows your terminal's palette.

```toml
[theme]
keyword = "magenta"
comment = { fg = "bright-black", italic = true }
"ui.selection" = { bg = "#303040" }
```

- A color alone sets the text color. A table can set `fg`, `bg`, `bold`, `italic`, `underline`, and `reverse`.
- Colors are `black`, `red`, `green`, `yellow`, `blue`, `magenta`, `cyan`, `white`, `bright-<color>`, `"#rrggbb"`, a number from 0 to 255, or `"default"`.
- Names with a dot are quoted in TOML. If a name has no style of its own, nib falls back to the part before the last dot: `function.method` uses `function`.
- Syntax colors use tree-sitter capture names (`keyword`, `function`, `string`, `comment`, `type`, `constant`, `number`, `property`, and so on). `text.title`, `text.emphasis`, `text.strong`, `text.literal`, and `text.uri` color Markdown.
- UI parts include `ui.mode.normal`, `ui.mode.insert`, `ui.mode.select`, `ui.selection`, `ui.menu.selected`, `ui.window`, `ui.popup`, `ui.popup.title`, `ui.popup.key`, `ui.directory`, `ui.cursor.match`, and the LSP ones `diagnostic.error`, `diagnostic.warning`, `diagnostic.info`, `diagnostic.hint`, `diagnostic.underline`.

## plugins/&lt;name&gt;.toml

One file holds everything about one plugin. The file name is the plugin name. The keys outside `[settings]` are read by nib; `[settings]` is handed to the plugin.

```toml
# ~/.config/nib/plugins/lsp.toml
enabled = true             # false: do not load it (this works for bundled plugins too)
path = "~/dev/my-lsp"      # load it from this directory instead
timeout-ms = 2000          # this plugin's call limit; "none" for no limit
init-timeout-ms = 5000
memory-mib = 512
load = "lazy"              # start it when first used; the default is "start"

[settings]
servers.rust.command = ["rust-analyzer"]
```

- Every key is optional. Without `timeout-ms`, `init-timeout-ms`, or `memory-mib`, the plugin gets the `[core]` values.
- `timeout-ms = "none"` removes the time limit. A plugin stuck in a loop can still be stopped with `C-g`, so this is safe to use while developing a plugin.
- `path` replaces a bundled plugin of the same name, which is how you try a modified copy of a standard plugin. The plugin's own name must match the file name, or nib reports an error.
- `load = "lazy"` still loads the plugin, but does not start it until one of its commands is called or one of the events it subscribes to arrives. Keys do not start a plugin, so a plugin that adds its own key layer (such as a base) should not be lazy.
- What goes under `[settings]` is up to each plugin. `nib config init` writes the example for your base plugin, and the plugin's own documentation lists the rest.

`nib plugin list` shows which of these files nib found.

### Reloading

Saving a file in the settings directory from inside nib reloads it. You can also run the core command `config.reload`; in the helix base it is `:config-reload`. In the helix base, `:config` opens `config.toml` and `:config <name>` opens `plugins/<name>.toml`.

- Applied at once: `[core]` values, the theme, and each plugin's `[settings]` (only the plugins whose settings changed are restarted).
- Applied at the next start: `enabled`, `path`, and `load`, and the plugin limits. nib tells you when a change needs a restart.
- If the new file has a mistake, the current settings stay and nib shows the reason.

## Remapping keys in the helix base

The helix base reads `[settings.keys.<mode>]` from `plugins/helix.toml`. The notation follows Helix's `[keys.*]`:

```toml
# ~/.config/nib/plugins/helix.toml
[settings.keys.normal]
"C-s" = "buffer.save"                   # a nib command
g = { a = "goto_last_accessed_file" }   # a table is a prefix key

[settings.keys.insert]
j = { k = "normal_mode" }               # "jk" leaves insert mode
```

- The right-hand side is a helix command name (`move_next_word_start`), or a nib command with a dot in its name (`buffer.save`, `picker.files`). Many Helix commands are supported, but not all, so a configuration copied from Helix may include names nib does not know.
- Your bindings are looked up before the default keys, and hide a default on the same key.
- An unknown command or a malformed key is reported when nib starts, and only that binding is skipped.
- After a prefix key in insert mode, a key that is not in the table types the prefix character first and then handles the key as usual, so `jj` does not swallow the `j`.

The other bases read keys from their own files. `nib config init` writes a commented example for the base you chose.

## Other directories

| Purpose | Location |
|---------|----------|
| Installed plugins and the data plugins keep | `~/.local/share/nib/` (under `$XDG_DATA_HOME`, or `%LOCALAPPDATA%\nib` on Windows) |
| Compiled plugin cache | `~/.cache/nib/` (under `$XDG_CACHE_HOME`, or `%LOCALAPPDATA%\nib` on Windows) |

The cache can be deleted at any time. It only makes the next start faster.
