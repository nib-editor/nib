# Plugins

Nearly everything in nib is a plugin: the keys, the languages, the file picker, the LSP client. They are WebAssembly programs that run in a sandbox, so a plugin can do only what it has declared. This page is about finding, installing, and managing plugins. For writing one, see the [plugin author guide](../../sdk/README.md).

## What is installed

```sh
nib plugin list
```

lists every plugin: whether it is enabled, where it is loaded from (built in, installed, or a `path`), where it came from if it was installed, and which settings file it has. It shows the bundled ones too: `helix`, `vim`, `emacs`, `nano`, `picker`, `lsp`, `indent`, and the languages.

## Finding and installing

```sh
nib plugin search            # everything on the list
nib plugin search status     # names or descriptions containing "status"
nib plugin add wordcount     # by name, as found on the list
```

The list is [`plugins.toml` in nib-editor/plugins](https://github.com/nib-editor/plugins). nib reads it from GitHub each time you search or install by name, so these two need a network connection. Being listed does not mean a plugin has been reviewed; nib's protection is the permission prompt below. Add yours with a pull request there.

You can install from anywhere:

```sh
nib plugin add someone/nib-foo                  # the latest GitHub release
nib plugin add someone/nib-foo@v0.2.0           # that tag's release
nib plugin add https://example.com/foo-0.1.0.nib.tar.gz
nib plugin add ./foo-0.1.0.nib.tar.gz           # a local archive
nib plugin add wordcount@v0.1.0                 # a name from the list, at a tag
```

For GitHub, the release must have exactly one file ending in `.nib.tar.gz`. nib uses `curl` to download, over HTTPS only.

### What `add` does

1. Downloads the archive and unpacks it into a temporary directory. Absolute paths, `..`, and symbolic links are refused, and so is an archive that unpacks to more than 100 MiB.
2. Checks the plugin: its manifest has to be readable, it must be built for the same plugin API version as your nib, and its name must not clash with a bundled or already installed plugin (a name is also the namespace of its commands).
3. **Asks you.** It shows the plugin's name, version, source, the permissions it declares, and the events it subscribes to, and asks whether to install. Nothing from the plugin runs before you answer. Use `--yes` to skip the question in a script; without a terminal and without `--yes`, nothing is installed.
4. Installs it to `~/.local/share/nib/installed/<name>/`, and records the source, the version, and the permissions you agreed to in `installed.toml`.

The plugin loads the next time nib starts. Plugins get the permissions they declared without asking again, so this question is the one place to say no.

## Permissions

A plugin has none by default. It declares what it uses in its manifest, and nib lists these in the question above and in the core menu:

| Permission | Lets the plugin |
|------------|-----------------|
| `process` | Run external programs |
| `fs-read` | Read files |
| `fs-write` | Write files |
| `network` | Use the network |
| `clipboard` | Read and write the system clipboard |

A plugin cannot ask for more later: nib does not hand out a permission the manifest does not name.

## Updating and removing

```sh
nib plugin update            # all installed plugins
nib plugin update foo
nib plugin remove foo
```

- `update` fetches the latest release from the recorded source. It does nothing if the archive has not changed. If the new version wants more permissions than you agreed to, it shows the difference and asks again.
- Plugins installed at a tag (`@v0.2.0`), from an archive URL, or from a local file are not updated.
- `remove` deletes the installed files and the record. Your `plugins/<name>.toml` and the data the plugin kept are left alone, and nib tells you so.

## The core menu

`C-g` (or your `menu-key`) opens the core menu, which is for managing plugins from inside nib. It works even when a plugin is stuck. Type to filter the list (`rsa` finds `Restart all plugins`), use the arrow keys or `C-p` / `C-n` to move, `enter` to choose, and `esc` to go back or close.

The first list has every plugin, with its version and state (`running`, `running, the base`, `base, not in use`, `disabled`, `languages`, or the error), followed by:

- `Open config.toml`, `Open the settings directory`, and `Reload the settings`
- `Add a plugin`, which takes the same forms as `nib plugin add`: a name, `name@tag`, `owner/repo[@tag]`, or a URL. It shows what it found and the permissions it asks for, asks before installing, and loads the plugin at once.
- `Restart all plugins`
- `Save all and quit` and `Quit`

Choosing a plugin shows what it can do (its permissions, the time limit, how many calls were slow, and its settings file) and these actions: `Restart`, `Disable` or `Enable` (or `Use as the base`), and `Open its settings`. Installed plugins also have `Update` and `Remove`, and plugins loaded with `--plugin` have `Reload from disk`.

`Update` checks in the background and keeps nib responsive. If the new version asks for more permissions, the menu shows them and asks first. The plugin is reloaded without restarting nib. `Remove` stops the plugin and deletes its files and record, but keeps your settings and the plugin's data; an installed plugin that was already loaded stays listed as disabled until the next start.

## Problems

- **A plugin will not load.** If the plugin was built for a different plugin API version than your nib, nib skips it and says why, then starts without it. A plugin that fails to load never stops nib itself. Update the plugin, or update nib.
- **A plugin hangs, loops, or crashes.** nib stops a call that takes longer than the time limit, and treats a crash or a memory overrun the same way: it drops the plugin's instance, starts a fresh one, and shows what went wrong. A plugin changes text only through transactions, so a failure does not corrupt your buffer. If a plugin fails three times within a minute, nib disables it and says so; bring it back with "restart all" in the core menu. To raise or remove the time limit, see [Configuration](configuration.md#pluginsnametoml); `C-g` stops a call that has no limit.
