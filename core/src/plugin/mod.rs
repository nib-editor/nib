//! Loads WebAssembly plugins and calls into them with time and memory limits.

mod api;
mod manifest;

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use wasmtime::component::{Component, HasSelf, Linker, ResourceTable};
use wasmtime::{
    Cache, CacheConfig, Config, Engine, Store, StoreContextMut, StoreLimits, StoreLimitsBuilder,
    Trap, UpdateDeadline,
};
use wasmtime_wasi::p2::pipe::MemoryOutputPipe;
use wasmtime_wasi::{FsPerms, WasiCtx};

use crate::Error;
use crate::config::{Load, Settings};
use crate::editor::{CORE_COMMANDS, Editor, State};
use crate::events::Event;
use crate::input::KeyEvent;
use api::bindings;
use api::bindings::exports::nib::plugin::guest::KeyResult;

pub type PluginId = usize;

/// Reads the name of the plugin in `dir` from its manifest.
pub fn plugin_name(dir: &Path) -> Result<String, Error> {
    Ok(manifest::read(&dir.join("plugin.toml"))?.name)
}

/// What a plugin's manifest says, for handling plugins without loading
/// them, as `nib plugin add` does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    /// The `nib:plugin` version it was built for; see `API_VERSION`.
    pub api: String,
    pub capabilities: Vec<String>,
    pub events: Vec<String>,
    /// The languages it provides.
    pub languages: Vec<String>,
    /// The files of its languages, grammars and queries, by path in its
    /// directory.
    pub language_files: Vec<String>,
    /// It has code to run, not only data such as languages.
    pub has_code: bool,
    /// It is a base (docs/design/bases/base.md).
    pub base: bool,
}

/// Reads and checks the manifest of the plugin in `dir`.
pub fn read_manifest(dir: &Path) -> Result<PluginManifest, Error> {
    let manifest = manifest::read(&dir.join("plugin.toml"))?;
    Ok(PluginManifest {
        name: manifest.name,
        version: manifest.version,
        api: manifest.api,
        capabilities: manifest.capabilities,
        events: manifest.events,
        language_files: manifest
            .languages
            .iter()
            .flat_map(|l| std::iter::once(&l.grammar).chain(l.queries.values()))
            .cloned()
            .collect(),
        languages: manifest.languages.into_iter().map(|l| l.name).collect(),
        has_code: dir.join("plugin.wasm").is_file(),
        base: manifest.base,
    })
}

/// The base started when the chosen one is not available.
const FALLBACK_BASE: &str = "helix";

/// The version of `nib:plugin` this host implements.
pub const API_VERSION: &str = "0.7";

/// How often the epoch advances during a plugin call. Timeouts are
/// accurate to about one tick.
const EPOCH_TICK: Duration = Duration::from_millis(10);
/// A plugin that fails this many times within `CRASH_WINDOW` is disabled.
const MAX_CRASHES: usize = 3;
const CRASH_WINDOW: Duration = Duration::from_secs(60);
const STDERR_CAPACITY: usize = 64 * 1024;
/// At most this many events are delivered in one go, so plugins that keep
/// answering each other's events cannot hang the editor.
const MAX_EVENTS: usize = 1000;

#[derive(Clone, Debug)]
pub struct PluginOptions {
    /// Where compiled plugins are cached. `None` compiles on every load.
    pub cache_dir: Option<PathBuf>,
    /// Where each plugin's data directory is made, as `<name>/`, seen by the
    /// plugin as `/data`. `None` gives plugins none.
    pub data_dir: Option<PathBuf>,
    /// A call taking longer is counted as slow.
    pub warn_after: Duration,
    /// A call taking longer is stopped.
    pub call_timeout: Duration,
    pub init_timeout: Duration,
    /// Maximum size of a plugin's linear memory, in bytes.
    pub memory_limit: usize,
}

impl Default for PluginOptions {
    fn default() -> Self {
        Self {
            cache_dir: None,
            data_dir: None,
            warn_after: Duration::from_millis(16),
            call_timeout: Settings::default().plugin_timeout,
            init_timeout: Settings::default().plugin_init_timeout,
            memory_limit: Settings::default().plugin_memory,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginInfo {
    pub name: String,
    pub version: String,
    /// One line on what it is, from its manifest.
    pub description: Option<String>,
    pub enabled: bool,
    /// Calls that took longer than `PluginOptions::warn_after`.
    pub slow_calls: u32,
    /// Why the plugin last failed.
    pub last_error: Option<String>,
    /// Loaded from a directory, so it can be reloaded from disk.
    pub reloadable: bool,
    /// Has code to run; otherwise it only provides data, such as languages.
    pub has_code: bool,
    /// A call taking longer is stopped; `None` for no limit.
    pub timeout: Option<Duration>,
    /// What it may do beyond the editor API, from its manifest.
    pub capabilities: Vec<String>,
    /// Loaded lazily and not started yet.
    pub waiting: bool,
    /// A base; only the one in use runs.
    pub base: bool,
}

/// The key that stops a plugin stuck in a call. It is not the menu key:
/// Emacs users press it all the time, and it means the same there.
pub const INTERRUPT_KEY: KeyEvent = KeyEvent::ctrl('g');

/// Stops the plugin call running when it is used, from any thread. The
/// frontend uses it when `INTERRUPT_KEY` is pressed, so a plugin stuck in a
/// call can be stopped. Calls that start afterwards are not affected.
#[derive(Clone, Debug)]
pub struct Interrupter(Arc<AtomicU64>);

impl Interrupter {
    pub fn interrupt(&self) {
        self.0.fetch_add(1, Ordering::AcqRel);
    }
}

/// Why a call stopped when Ctrl-g was pressed during it.
#[derive(Debug)]
struct Interrupted;

impl std::fmt::Display for Interrupted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("stopped with Ctrl-g")
    }
}

impl std::error::Error for Interrupted {}

/// The file beside a plugin's manifest with its settings, commented out,
/// that goes into the template of its settings file.
pub const SETTINGS_EXAMPLE: &str = "settings.example.toml";

/// Where a plugin's manifest and code come from.
pub enum PluginSource<'a> {
    Dir(&'a Path),
    /// Built into the editor: the manifest and the other files by path.
    Bytes {
        manifest: &'a str,
        files: &'static [(&'static str, &'static [u8])],
    },
}

impl PluginSource<'_> {
    /// Built-in files are borrowed, not copied: languages keep their
    /// grammars until first used, megabytes for the standard ones.
    fn read(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>, String> {
        match self {
            PluginSource::Dir(dir) => match std::fs::read(dir.join(path)) {
                Ok(bytes) => Ok(Some(Cow::Owned(bytes))),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(err) => Err(format!("{path}: {err}")),
            },
            PluginSource::Bytes { files, .. } => Ok(files
                .iter()
                .find(|(name, _)| *name == path)
                .map(|(_, bytes)| Cow::Borrowed(*bytes))),
        }
    }
}

#[derive(Default)]
pub(crate) struct Plugins {
    pub(crate) options: PluginOptions,
    /// Created when the first plugin is loaded, so starting without plugins
    /// costs nothing.
    runtime: Option<Runtime>,
    entries: Vec<Plugin>,
    /// Plugins that failed in a call made from another plugin, with the
    /// reason. They are restarted once the outermost call returns.
    failures: Vec<(PluginId, String)>,
    /// Counts interrupt requests; see `Interrupter`.
    interrupts: Arc<AtomicU64>,
    /// The base in use, by name: the one started last. It stays while
    /// disabled, so its menu key does too.
    base: Option<String>,
}

/// A plugin's code.
enum Code {
    /// Only data, such as a language.
    None,
    /// Compiled when first started: a base waiting to be switched to. The
    /// compiled code of a base takes megabytes, and the bytes built into
    /// nib take none.
    Wasm(Cow<'static, [u8]>),
    Compiled(Component),
}

impl Plugins {
    /// The runtime, started with the first plugin that has code.
    fn runtime(&mut self) -> Result<&Runtime, String> {
        if self.runtime.is_none() {
            let runtime = Runtime::new(&self.options)
                .map_err(|err| format!("starting the plugin runtime failed: {err}"))?;
            self.runtime = Some(runtime);
        }
        Ok(self.runtime.as_ref().expect("created above"))
    }
}

struct Runtime {
    engine: Engine,
    linker: Linker<PluginData>,
    ticker: Ticker,
}

/// Advances the engine's epoch while a plugin call runs, and sleeps
/// otherwise, so an idle editor never wakes up for it.
struct Ticker {
    /// Calls in progress; nested calls count too.
    calls: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: thread::Thread,
}

impl Ticker {
    fn start(engine: Engine) -> std::io::Result<Self> {
        let calls = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (thread_calls, thread_stop) = (calls.clone(), stop.clone());
        let handle = thread::Builder::new()
            .name("nib-epoch".into())
            .spawn(move || {
                while !thread_stop.load(Ordering::Relaxed) {
                    if thread_calls.load(Ordering::Acquire) > 0 {
                        thread::sleep(EPOCH_TICK);
                        engine.increment_epoch();
                    } else {
                        // An unpark that comes first makes this return at once.
                        thread::park();
                    }
                }
            })?;
        Ok(Self {
            calls,
            stop,
            thread: handle.thread().clone(),
        })
    }

    fn begin(&self) {
        if self.calls.fetch_add(1, Ordering::AcqRel) == 0 {
            self.thread.unpark();
        }
    }

    fn end(&self) {
        self.calls.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Drop for Ticker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.unpark();
    }
}

struct Plugin {
    name: String,
    version: String,
    description: Option<String>,
    /// `None` for plugins built into the editor.
    dir: Option<PathBuf>,
    code: Code,
    /// `[settings]` from its `plugins/<name>.toml`, as JSON.
    config: String,
    limits: Limits,
    /// The kinds of events it gets.
    subscriptions: Vec<String>,
    capabilities: Vec<String>,
    /// Out of `Plugins` while a call runs: calling it again then would
    /// re-enter it.
    instance: Option<Instance>,
    in_call: bool,
    /// Loaded lazily and not started yet.
    waiting: bool,
    enabled: bool,
    crashes: Vec<Instant>,
    slow_calls: u32,
    last_error: Option<String>,
    base: bool,
    menu_key: Option<KeyEvent>,
    /// Keys for its commands under the base's leader, from its manifest.
    keys: Vec<(String, String)>,
}

/// A key an enabled plugin suggests under the base's leader.
#[derive(Clone, Debug)]
pub(crate) struct LeaderKey {
    pub plugin: PluginId,
    pub name: String,
    pub keys: String,
    pub command: String,
}

/// Limits for one plugin: its own from `plugins/<name>.toml`, or the
/// defaults.
#[derive(Clone, Copy)]
struct Limits {
    /// `None` for no limit.
    call: Option<Duration>,
    init: Option<Duration>,
    memory: usize,
}

/// The call a store is in, for the checks on each epoch tick.
struct CallClock {
    started: Instant,
    /// The CPU time the thread had used when it started.
    cpu: Option<Duration>,
    limit: Option<Duration>,
    /// The interrupt count when it started.
    interrupts: u64,
}

impl CallClock {
    fn new(limit: Option<Duration>, interrupts: u64) -> Self {
        Self {
            started: Instant::now(),
            cpu: crate::time::thread_cpu(),
            limit,
            interrupts,
        }
    }

    /// How long the call has run, against its limit: the CPU time it used,
    /// so a busy machine, or a wait for the system such as starting a
    /// program, does not count; the time on the clock where the CPU time
    /// cannot be told.
    fn used(&self) -> Duration {
        match (self.cpu, crate::time::thread_cpu()) {
            (Some(start), Some(now)) => now.saturating_sub(start),
            _ => self.started.elapsed(),
        }
    }
}

struct Instance {
    store: Store<PluginData>,
    bindings: bindings::Plugin,
    stderr: MemoryOutputPipe,
}

/// The data of a plugin's store.
pub(crate) struct PluginData {
    /// The editor state, present only during a call.
    state: Option<State>,
    /// The other plugins, present only during a call, so it can call them.
    plugins: Option<Plugins>,
    plugin: PluginId,
    /// It may start programs.
    can_spawn: bool,
    /// It may list files: "fs-read" or "fs-write".
    can_read_files: bool,
    /// It may make directories: "fs-write".
    can_write_files: bool,
    can_use_clipboard: bool,
    /// It is a base, so it may say what mode it is in.
    is_base: bool,
    clock: CallClock,
    interrupts: Arc<AtomicU64>,
    wasi: WasiCtx,
    table: ResourceTable,
    limits: StoreLimits,
}

impl Runtime {
    fn new(options: &PluginOptions) -> wasmtime::Result<Self> {
        let mut config = Config::new();
        config.epoch_interruption(true);
        if let Some(dir) = &options.cache_dir {
            let mut cache = CacheConfig::new();
            cache.with_directory(dir);
            config.cache(Some(Cache::new(cache)?));
        }
        let engine = Engine::new(&config)?;

        let mut linker = Linker::new(&engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
        bindings::Plugin::add_to_linker::<PluginData, HasSelf<PluginData>>(&mut linker, |data| {
            data
        })?;

        let ticker = Ticker::start(engine.clone())?;
        Ok(Self {
            engine,
            linker,
            ticker,
        })
    }
}

impl Editor {
    /// Takes effect for plugins loaded afterwards.
    pub fn set_plugin_options(&mut self, options: PluginOptions) {
        self.plugins.options = options;
    }

    /// Where compiled plugins are cached. Takes effect for plugins loaded
    /// afterwards.
    pub fn set_plugin_cache_dir(&mut self, dir: Option<PathBuf>) {
        self.state_mut().languages.cache_dir = dir.clone();
        self.plugins.options.cache_dir = dir;
    }

    /// Where plugins' data directories are made. Takes effect for plugins
    /// started afterwards.
    pub fn set_plugin_data_dir(&mut self, dir: Option<PathBuf>) {
        self.plugins.options.data_dir = dir;
    }

    /// Loads the plugin in `dir` and calls its `init` with its table from
    /// config.toml.
    pub fn load_plugin(&mut self, dir: &Path) -> Result<(), Error> {
        self.add_plugin(&PluginSource::Dir(dir))
    }

    /// Loads a plugin built into the editor from its manifest and its other
    /// files by path, such as `plugin.wasm`.
    pub fn load_builtin_plugin(
        &mut self,
        manifest: &str,
        files: &'static [(&'static str, &'static [u8])],
    ) -> Result<(), Error> {
        self.add_plugin(&PluginSource::Bytes { manifest, files })
    }

    /// Loads plugins in order, as `load_plugin` and `load_builtin_plugin`
    /// do, going on past those that fail. Taking their compiled code from
    /// the cache is most of a plugin's startup, milliseconds each, so this
    /// thread and one more share the compiling; then they are added and
    /// started in order.
    pub fn load_plugins(&mut self, sources: &[PluginSource]) -> Vec<Result<(), Error>> {
        let read: Vec<_> = sources.iter().map(|s| self.read_plugin(s)).collect();
        let work: Vec<(usize, &manifest::Manifest, &[u8])> = read
            .iter()
            .enumerate()
            .filter_map(|(i, read)| match read {
                Ok((manifest, Some(wasm))) if !self.standby(manifest) => {
                    Some((i, manifest, &wasm[..]))
                }
                _ => None,
            })
            .collect();
        let mut components: Vec<Option<Result<Component, Error>>> =
            read.iter().map(|_| None).collect();
        if !work.is_empty() {
            let engine = self.engine().cloned();
            let next = AtomicUsize::new(0);
            let compile = || {
                let mut done = Vec::new();
                while let Some(&(i, manifest, wasm)) =
                    work.get(next.fetch_add(1, Ordering::Relaxed))
                {
                    let component = match &engine {
                        Ok(engine) => compile_component(engine, manifest, wasm),
                        Err(err) => Err(Error::Plugin(err.to_string())),
                    };
                    done.push((i, component));
                }
                done
            };
            let compiled = thread::scope(|scope| {
                // One more thread than this one: each keeps a few MB of the
                // code it compiled scattered over its allocator, and more
                // barely shorten startup (docs/design/architecture.md).
                let other = scope.spawn(compile);
                let mut compiled = compile();
                compiled.extend(other.join().expect("compiling plugins panicked"));
                compiled
            });
            for (i, component) in compiled {
                components[i] = Some(component);
            }
        }
        let mut results: Vec<_> = sources
            .iter()
            .zip(read)
            .zip(components)
            .map(|((source, read), component)| {
                let (manifest, wasm) = read?;
                let code = match (component, wasm) {
                    (Some(component), _) => Code::Compiled(component?),
                    (None, Some(wasm)) => Code::Wasm(wasm),
                    (None, None) => Code::None,
                };
                self.add_compiled(source, manifest, code)
            })
            .collect();
        if let Some(fallback) = self.fall_back_to_a_base() {
            results.push(Err(Error::Plugin(fallback)));
        }
        results
    }

    /// Starts a base when the chosen one is missing or failed, since
    /// without one no key edits: helix if it is there. Says which.
    fn fall_back_to_a_base(&mut self) -> Option<String> {
        if self.plugins.base.is_some() {
            return None;
        }
        let bases = || {
            self.plugins
                .entries
                .iter()
                .enumerate()
                .filter(|(_, p)| p.base)
        };
        let (id, _) = bases()
            .find(|(_, p)| p.name == FALLBACK_BASE)
            .or_else(|| bases().next())?;
        let chosen = self.state().settings.base.clone();
        let name = self.plugins.entries[id].name.clone();
        Some(match self.restart_plugin(id) {
            Ok(()) => format!("{chosen} is not available as the base; using {name}"),
            Err(err) => format!("{chosen} is not available as the base, nor {name}: {err}"),
        })
    }

    fn add_plugin(&mut self, source: &PluginSource) -> Result<(), Error> {
        let (manifest, component) = self.compile(source)?;
        self.add_compiled(
            source,
            manifest,
            component.map_or(Code::None, Code::Compiled),
        )
    }

    /// Bases other than the chosen one wait, stopped, to be switched to.
    fn standby(&self, manifest: &manifest::Manifest) -> bool {
        manifest.base && manifest.name != self.state().settings.base
    }

    fn add_compiled(
        &mut self,
        source: &PluginSource,
        manifest: manifest::Manifest,
        code: Code,
    ) -> Result<(), Error> {
        if self.plugins.entries.iter().any(|p| p.name == manifest.name) {
            return Err(Error::Plugin(format!("{}: already loaded", manifest.name)));
        }
        self.add_languages(&manifest, source)
            .map_err(|err| Error::Plugin(format!("{}: {err}", manifest.name)))?;
        let settings = self.plugin_config(&manifest.name);
        let options = &self.plugins.options;
        let limits = Limits {
            call: settings
                .timeout
                .map_or(Some(options.call_timeout), |t| t.limit()),
            init: settings
                .init_timeout
                .map_or(Some(options.init_timeout), |t| t.limit()),
            memory: settings.memory.unwrap_or(options.memory_limit),
        };
        let lazy = settings.load == Load::Lazy && !matches!(code, Code::None);
        let standby = self.standby(&manifest);
        let menu_key = manifest.menu_key.as_deref().map(|key| {
            key.parse()
                .expect("the manifest's menu key was checked when it was read")
        });
        // For the template of its settings file (docs/design/plugins/files.md).
        if let Ok(Some(example)) = source.read(SETTINGS_EXAMPLE) {
            let example = String::from_utf8_lossy(&example).into_owned();
            self.state_mut()
                .settings_examples
                .insert(manifest.name.clone(), example);
        }
        let id = self.plugins.entries.len();
        self.plugins.entries.push(Plugin {
            name: manifest.name.clone(),
            version: manifest.version,
            description: manifest.description,
            dir: match *source {
                PluginSource::Dir(dir) => Some(dir.to_path_buf()),
                PluginSource::Bytes { .. } => None,
            },
            code,
            config: settings.settings,
            limits,
            subscriptions: manifest.events,
            capabilities: manifest.capabilities,
            instance: None,
            in_call: false,
            waiting: lazy && !manifest.base,
            enabled: !standby,
            crashes: Vec::new(),
            slow_calls: 0,
            last_error: None,
            base: manifest.base,
            menu_key,
            keys: manifest.keys.into_iter().collect(),
        });
        if standby || (lazy && !manifest.base) {
            self.sync_leader_keys(id);
            return Ok(());
        }
        let started = if manifest.base {
            self.restart_plugin(id)
        } else {
            self.start_plugin(id)
        };
        if let Err(message) = started {
            self.plugins.entries.pop();
            return Err(Error::Plugin(format!("{}: {message}", manifest.name)));
        }
        self.sync_leader_keys(id);
        Ok(())
    }

    /// Reads and checks the manifest, then compiles the component.
    /// Registers the languages a plugin provides, and gives them to open
    /// buffers of their file types.
    fn add_languages(
        &mut self,
        manifest: &manifest::Manifest,
        source: &PluginSource,
    ) -> Result<(), String> {
        for language in &manifest.languages {
            let grammar = source
                .read(&language.grammar)?
                .ok_or_else(|| format!("{} is missing", language.grammar))?;
            let mut queries = BTreeMap::new();
            for (name, path) in &language.queries {
                let bytes = source
                    .read(path)?
                    .ok_or_else(|| format!("{path} is missing"))?;
                let text = String::from_utf8(bytes.into_owned())
                    .map_err(|_| format!("{path} is not UTF-8"))?;
                queries.insert(name.clone(), text);
            }
            self.state_mut().languages.add(
                &language.name,
                language.file_types.clone(),
                grammar,
                queries,
            );
        }
        if !manifest.languages.is_empty() {
            self.state_mut().attach_syntax();
        }
        Ok(())
    }

    /// Reads and checks the manifest, then compiles the component, if the
    /// plugin has code.
    fn compile(
        &mut self,
        source: &PluginSource,
    ) -> Result<(manifest::Manifest, Option<Component>), Error> {
        let (manifest, wasm) = self.read_plugin(source)?;
        let Some(wasm) = wasm else {
            return Ok((manifest, None));
        };
        let component = compile_component(self.engine()?, &manifest, &wasm)?;
        Ok((manifest, Some(component)))
    }

    /// Reads and checks the manifest, and reads the code if the plugin has
    /// any; a language, for one, is only data.
    fn read_plugin(&self, source: &PluginSource) -> Result<ReadPlugin, Error> {
        let manifest = match source {
            PluginSource::Dir(dir) => manifest::read(&dir.join("plugin.toml"))?,
            PluginSource::Bytes { manifest, .. } => manifest::parse(manifest, "built-in plugin")?,
        };
        let fail = |message: String| Error::Plugin(format!("{}: {message}", manifest.name));
        if manifest.api != API_VERSION {
            return Err(fail(format!(
                "needs API {}, but nib provides {API_VERSION}",
                manifest.api
            )));
        }
        let wasm = source.read("plugin.wasm").map_err(fail)?;
        Ok((manifest, wasm))
    }

    /// The engine plugins run on, started with the first plugin that has
    /// code.
    fn engine(&mut self) -> Result<&Engine, Error> {
        let runtime = self.plugins.runtime().map_err(Error::Plugin)?;
        Ok(&runtime.engine)
    }

    /// Stops the plugin, forgets its failures, and starts it again. A base
    /// takes the place of the one in use, which comes back if it fails.
    pub(crate) fn restart_plugin(&mut self, id: PluginId) -> Result<(), String> {
        let previous = match self.plugins.entries[id].base {
            true => self.stop_other_bases(id),
            false => None,
        };
        self.stop_plugin(id);
        let plugin = &mut self.plugins.entries[id];
        plugin.crashes.clear();
        plugin.enabled = true;
        let result = self.start_plugin(id);
        let plugin = &mut self.plugins.entries[id];
        match &result {
            Ok(()) if plugin.base => self.plugins.base = Some(plugin.name.clone()),
            Ok(()) => {}
            Err(_) => {
                plugin.enabled = false;
                if let Some(previous) = previous {
                    let _ = self.restart_plugin(previous);
                }
            }
        }
        self.sync_leader_keys(id);
        result
    }

    /// Puts the plugin's leader keys where plugins can read them while it
    /// is enabled, in the order plugins were loaded.
    fn sync_leader_keys(&mut self, id: PluginId) {
        let plugin = &self.plugins.entries[id];
        let keys: Vec<LeaderKey> = match plugin.enabled {
            true => plugin
                .keys
                .iter()
                .map(|(keys, command)| LeaderKey {
                    plugin: id,
                    name: plugin.name.clone(),
                    keys: keys.clone(),
                    command: command.clone(),
                })
                .collect(),
            false => Vec::new(),
        };
        let leader_keys = &mut self.state_mut().leader_keys;
        leader_keys.retain(|key| key.plugin != id);
        leader_keys.extend(keys);
        leader_keys.sort_by_key(|key| key.plugin);
    }

    /// Stops the running bases other than `id`, as only one runs. Returns
    /// the one that was in use.
    fn stop_other_bases(&mut self, id: PluginId) -> Option<PluginId> {
        let running: Vec<PluginId> = (0..self.plugins.entries.len())
            .filter(|&other| {
                let plugin = &self.plugins.entries[other];
                other != id && plugin.base && plugin.enabled
            })
            .collect();
        for &other in &running {
            self.disable_plugin(other);
        }
        running.first().copied()
    }

    /// The base in use, if it is running.
    pub(crate) fn running_base(&self) -> Option<PluginId> {
        let name = self.plugins.base.as_ref()?;
        self.plugins
            .entries
            .iter()
            .position(|p| &p.name == name && p.enabled && p.instance.is_some())
    }

    /// The base in use, by name.
    pub fn base_in_use(&self) -> Option<&str> {
        self.plugins.base.as_deref()
    }

    /// The plugin by name.
    pub(crate) fn plugin_id(&self, name: &str) -> Option<PluginId> {
        self.plugins.entries.iter().position(|p| p.name == name)
    }

    /// The key that opens the core menu: the user's, or the base's, or
    /// Ctrl-g.
    pub fn menu_key(&self) -> KeyEvent {
        let base = || {
            let name = self.plugins.base.as_ref()?;
            self.plugins
                .entries
                .iter()
                .find(|p| &p.name == name)?
                .menu_key
        };
        self.state()
            .settings
            .menu_key
            .or_else(base)
            .unwrap_or(KeyEvent::ctrl('g'))
    }

    pub(crate) fn disable_plugin(&mut self, id: PluginId) {
        self.stop_plugin(id);
        self.plugins.entries[id].enabled = false;
        self.sync_leader_keys(id);
    }

    /// Compiles the plugin again from its directory and restarts it.
    pub(crate) fn reload_plugin(&mut self, id: PluginId) -> Result<(), String> {
        let plugin = &self.plugins.entries[id];
        let Some(dir) = plugin.dir.clone() else {
            return Err("it is built in and has no files to reload".into());
        };
        let name = plugin.name.clone();
        let (manifest, component) = self
            .compile(&PluginSource::Dir(&dir))
            .map_err(|err| err.to_string())?;
        if manifest.name != name {
            return Err(format!("its name changed to {}", manifest.name));
        }
        self.add_languages(&manifest, &PluginSource::Dir(&dir))?;
        let plugin = &mut self.plugins.entries[id];
        plugin.version = manifest.version;
        plugin.subscriptions = manifest.events;
        plugin.capabilities = manifest.capabilities;
        plugin.code = component.map_or(Code::None, Code::Compiled);
        self.restart_plugin(id)
    }

    /// Restarts every plugin, including disabled ones, forgetting their past
    /// failures.
    pub(crate) fn restart_plugins(&mut self) {
        let mut failures = Vec::new();
        for id in 0..self.plugins.entries.len() {
            let plugin = &self.plugins.entries[id];
            if plugin.base && self.plugins.base.as_ref() != Some(&plugin.name) {
                continue;
            }
            if let Err(err) = self.restart_plugin(id) {
                failures.push(format!("{}: {err}", self.plugins.entries[id].name));
            }
        }
        let message = match (self.plugins.entries.is_empty(), failures.is_empty()) {
            (true, _) => "no plugins are loaded".to_string(),
            (false, true) => "plugins restarted".to_string(),
            (false, false) => format!("restarting failed: {}", failures.join("; ")),
        };
        self.state_mut().message = Some(message);
    }

    pub fn plugins(&self) -> Vec<PluginInfo> {
        self.plugins
            .entries
            .iter()
            .map(|p| PluginInfo {
                name: p.name.clone(),
                version: p.version.clone(),
                description: p.description.clone(),
                enabled: p.enabled,
                slow_calls: p.slow_calls,
                last_error: p.last_error.clone(),
                reloadable: p.dir.is_some(),
                has_code: !matches!(p.code, Code::None),
                timeout: p.limits.call,
                capabilities: p.capabilities.clone(),
                waiting: p.waiting,
                base: p.base,
            })
            .collect()
    }

    /// Returns whether the plugin handled the key. A failing plugin counts
    /// as having handled it, so the key does not fall through.
    pub(crate) fn plugin_handle_key(&mut self, id: PluginId, key: KeyEvent) -> bool {
        let key = api::key_event(key);
        let result = self.call_plugin(id, |bindings, store| {
            bindings.nib_plugin_guest().call_handle_key(store, key)
        });
        !matches!(result, Some(KeyResult::Pass))
    }

    /// As `plugin_handle_key`, for pasted text.
    pub(crate) fn plugin_handle_paste(&mut self, id: PluginId, text: &str) -> bool {
        let result = self.call_plugin(id, |bindings, store| {
            bindings.nib_plugin_guest().call_handle_paste(store, text)
        });
        !matches!(result, Some(KeyResult::Pass))
    }

    /// Instantiates the plugin and calls `init`. On failure the plugin is
    /// left stopped.
    fn start_plugin(&mut self, id: PluginId) -> Result<(), String> {
        let result = start_in(&mut self.plugins, &mut self.state, id);
        self.handle_nested_failures();
        result
    }

    /// The handle a frontend uses to stop a plugin stuck in a call.
    pub fn interrupter(&self) -> Interrupter {
        Interrupter(self.plugins.interrupts.clone())
    }

    /// Starts the lazy plugin whose command `name` is, if it has not
    /// started yet.
    fn wake_for_command(&mut self, name: &str) {
        if let Some(id) = waiting_for_command(&self.plugins, self.state(), name) {
            let _ = self.start_plugin(id);
        }
    }

    /// Calls into a running plugin, handling a failure by restarting or
    /// disabling it. Returns `None` if the plugin is not running or failed.
    fn call_plugin<R>(
        &mut self,
        id: PluginId,
        f: impl FnOnce(&bindings::Plugin, &mut Store<PluginData>) -> wasmtime::Result<R>,
    ) -> Option<R> {
        // A stopped plugin has nothing to call and has not failed again.
        self.plugins.entries[id].instance.as_ref()?;
        let timeout = self.plugins.entries[id].limits.call;
        let result = self.invoke(id, timeout, f);
        let value = match result {
            Ok(value) => Some(value),
            Err(err) => {
                let reason = describe_failure(&self.plugins.entries[id], &err);
                self.plugin_failed(id, reason);
                None
            }
        };
        self.handle_nested_failures();
        value
    }

    fn invoke<R>(
        &mut self,
        id: PluginId,
        timeout: Option<Duration>,
        f: impl FnOnce(&bindings::Plugin, &mut Store<PluginData>) -> wasmtime::Result<R>,
    ) -> wasmtime::Result<R> {
        call_in(&mut self.plugins, &mut self.state, id, timeout, f)
    }

    /// Restarts or disables the plugins that failed in calls from other
    /// plugins.
    fn handle_nested_failures(&mut self) {
        while !self.plugins.failures.is_empty() {
            let (id, reason) = self.plugins.failures.remove(0);
            self.plugin_failed(id, reason);
        }
    }

    fn plugin_failed(&mut self, id: PluginId, reason: String) {
        self.stop_plugin(id);

        let plugin = &mut self.plugins.entries[id];
        let now = Instant::now();
        plugin
            .crashes
            .retain(|&time| now.duration_since(time) < CRASH_WINDOW);
        plugin.crashes.push(now);
        plugin.last_error = Some(reason.clone());
        let name = plugin.name.clone();

        let message = if plugin.crashes.len() >= MAX_CRASHES {
            plugin.enabled = false;
            format!("plugin {name} disabled after repeated errors: {reason}")
        } else {
            match self.start_plugin(id) {
                Ok(()) => format!("plugin {name} restarted after an error: {reason}"),
                Err(err) => {
                    self.plugins.entries[id].enabled = false;
                    format!("plugin {name} disabled, restarting failed: {err}")
                }
            }
        };
        self.state_mut().message = Some(message);
    }

    /// Drops the instance and everything the plugin put into the editor.
    fn stop_plugin(&mut self, id: PluginId) {
        self.plugins.entries[id].instance = None;
        self.state_mut().remove_plugin_parts(id);
    }

    /// Calls a command a plugin registered, or a core command.
    pub fn call_command(&mut self, name: &str, args: &str) -> Result<String, String> {
        if let Some(redirected) = self.state().redirect(name) {
            let (name, args) = redirected?;
            return self.call_command(&name, &args);
        }
        self.wake_for_command(name);
        let command = self
            .state()
            .commands
            .iter()
            .find(|command| command.name == name)
            .cloned();
        let result = match command {
            Some(command) => self
                .call_plugin(command.owner, |bindings, store| {
                    bindings
                        .nib_plugin_guest()
                        .call_run_command(store, &command.short, args)
                })
                .unwrap_or_else(|| Err(format!("{name}: the plugin is not running"))),
            None => self.state_mut().run_command(name, args),
        };
        self.reload_config_if_asked();
        self.deliver_events();
        result
    }

    /// Every command, core and registered, with its description.
    pub fn commands(&self) -> Vec<(String, String)> {
        let core = CORE_COMMANDS
            .iter()
            .map(|&(name, description)| (name.to_string(), description.to_string()));
        let registered = self
            .state()
            .commands
            .iter()
            .map(|command| (command.name.clone(), command.description.clone()));
        core.chain(registered).collect()
    }

    /// Delivers the queued events, and the ones plugins emit meanwhile, in
    /// order.
    pub fn deliver_events(&mut self) -> bool {
        let mut delivered = 0;
        while let Some((target, event)) = self.state_mut().pop_event() {
            if delivered == MAX_EVENTS {
                let state = self.state_mut();
                let dropped = 1 + state.events.len();
                state.events.clear();
                state.message = Some(format!(
                    "dropped {dropped} events: plugins sent more than {MAX_EVENTS} at once"
                ));
                break;
            }
            delivered += 1;
            let receivers: Vec<PluginId> = match target {
                Some(id) => vec![id],
                None => (0..self.plugins.entries.len())
                    .filter(|&id| {
                        self.plugins.entries[id]
                            .subscriptions
                            .iter()
                            .any(|kind| kind == event.kind())
                    })
                    .collect(),
            };
            for id in receivers {
                if self.plugins.entries[id].waiting {
                    let _ = self.start_plugin(id);
                }
                self.call_plugin(id, |bindings, store| {
                    bindings
                        .nib_plugin_guest()
                        .call_on_event(store, &api::wit_event(&event))
                });
            }
        }
        delivered > 0
    }

    /// When the next timer is due.
    pub fn next_timer(&self) -> Option<Instant> {
        self.state().timers.iter().map(|timer| timer.due).min()
    }

    /// Sends the timers that are due to their plugins.
    pub fn run_timers(&mut self) {
        let due = self.state_mut().take_due_timers(Instant::now());
        for timer in due {
            self.state_mut()
                .push_event(Some(timer.owner), Event::Timer(timer.id));
        }
        self.after_plugins_ran();
    }
}

/// Lends the editor state and the other plugins to plugin `id`'s store for
/// the duration of `f`. Its instance is out of `plugins` meanwhile, so a
/// call back into it finds it busy.
fn call_in<R>(
    plugins: &mut Plugins,
    state: &mut Option<State>,
    id: PluginId,
    timeout: Option<Duration>,
    f: impl FnOnce(&bindings::Plugin, &mut Store<PluginData>) -> wasmtime::Result<R>,
) -> wasmtime::Result<R> {
    let mut instance = plugins.entries[id]
        .instance
        .take()
        .ok_or_else(|| wasmtime::Error::msg("plugin is not running"))?;
    plugins.entries[id].in_call = true;
    let warn_after = plugins.options.warn_after;
    plugins
        .runtime
        .as_ref()
        .expect("runtime exists")
        .ticker
        .begin();

    let data = instance.store.data_mut();
    data.clock = CallClock::new(timeout, data.interrupts.load(Ordering::Acquire));
    data.state = state.take();
    data.plugins = Some(std::mem::take(plugins));
    // Checked on every tick by `check_call`.
    instance.store.set_epoch_deadline(1);
    let started = Instant::now();
    let result = f(&instance.bindings, &mut instance.store);
    let elapsed = started.elapsed();
    let data = instance.store.data_mut();
    *state = data.state.take();
    *plugins = data.plugins.take().expect("plugins come back after a call");

    plugins
        .runtime
        .as_ref()
        .expect("runtime exists")
        .ticker
        .end();
    let plugin = &mut plugins.entries[id];
    plugin.in_call = false;
    if elapsed > warn_after {
        plugin.slow_calls += 1;
    }
    plugin.instance = Some(instance);
    result
}

impl PluginData {
    /// Calls `command` of plugin `owner` from within this plugin's call,
    /// passing on what this plugin was lent.
    pub(crate) fn call_plugin_command(
        &mut self,
        owner: PluginId,
        name: &str,
        short: &str,
        args: &str,
    ) -> wasmtime::Result<Result<String, String>> {
        let plugins = self
            .plugins
            .as_mut()
            .ok_or_else(|| wasmtime::Error::msg("plugins are only reachable during a call"))?;
        let plugin = &plugins.entries[owner];
        if plugin.in_call {
            return Ok(Err(format!(
                "{name}: {} is in a call already and cannot be called back",
                plugin.name
            )));
        }
        if plugin.instance.is_none() {
            return Ok(Err(format!("{name}: {} is not running", plugin.name)));
        }
        let timeout = plugin.limits.call;
        let result = call_in(
            plugins,
            &mut self.state,
            owner,
            timeout,
            |bindings, store| {
                bindings
                    .nib_plugin_guest()
                    .call_run_command(store, short, args)
            },
        );
        match result {
            Ok(result) => Ok(result),
            Err(err) => {
                let plugins = self.plugins.as_mut().expect("still lent");
                let plugin = &mut plugins.entries[owner];
                let reason = describe_failure(plugin, &err);
                let message = format!("{name}: {} failed: {reason}", plugin.name);
                plugin.instance = None;
                plugins.failures.push((owner, reason));
                Ok(Err(message))
            }
        }
    }

    /// Starts the lazy plugin whose command `name` is, if it has not
    /// started yet, from within this plugin's call.
    pub(crate) fn wake_for_command(&mut self, name: &str) -> wasmtime::Result<()> {
        let plugins = self
            .plugins
            .as_mut()
            .ok_or_else(|| wasmtime::Error::msg("plugins are only reachable during a call"))?;
        let Some(state) = self.state.as_ref() else {
            return Ok(());
        };
        if let Some(id) = waiting_for_command(plugins, state, name)
            && let Err(reason) = start_in(plugins, &mut self.state, id)
        {
            let plugins = self.plugins.as_mut().expect("still lent");
            plugins.failures.push((id, reason));
        }
        Ok(())
    }

    /// The name of this plugin, for names it registers or emits.
    pub(crate) fn plugin_name(&self) -> wasmtime::Result<&str> {
        let plugins = self
            .plugins
            .as_ref()
            .ok_or_else(|| wasmtime::Error::msg("plugins are only reachable during a call"))?;
        Ok(&plugins.entries[self.plugin].name)
    }
}

/// Instantiates plugin `id` and calls its `init`. On failure it is left
/// stopped, with the reason as its last error.
fn start_in(plugins: &mut Plugins, state: &mut Option<State>, id: PluginId) -> Result<(), String> {
    plugins.entries[id].waiting = false;
    if let Code::Wasm(wasm) = &plugins.entries[id].code {
        let wasm = wasm.clone();
        let component =
            Component::new(&plugins.runtime()?.engine, &wasm).map_err(|err| format!("{err:#}"))?;
        plugins.entries[id].code = Code::Compiled(component);
    }
    let Code::Compiled(component) = &plugins.entries[id].code else {
        // Data only: nothing runs.
        return Ok(());
    };
    let component = component.clone();
    let runtime = plugins.runtime.as_ref().expect("runtime exists");
    let plugin = &mut plugins.entries[id];
    let limits = plugin.limits;

    let stderr = MemoryOutputPipe::new(STDERR_CAPACITY);
    let data_dir = plugins
        .options
        .data_dir
        .as_ref()
        .map(|dir| dir.join(&plugin.name));
    let wasi = wasi_context(&plugin.capabilities, data_dir.as_deref(), stderr.clone())?;
    let data = PluginData {
        state: None,
        plugins: None,
        plugin: id,
        can_spawn: plugin.capabilities.iter().any(|c| c == "process"),
        can_read_files: plugin
            .capabilities
            .iter()
            .any(|c| c == "fs-read" || c == "fs-write"),
        can_write_files: plugin.capabilities.iter().any(|c| c == "fs-write"),
        can_use_clipboard: plugin.capabilities.iter().any(|c| c == "clipboard"),
        is_base: plugin.base,
        clock: CallClock::new(limits.init, plugins.interrupts.load(Ordering::Acquire)),
        interrupts: plugins.interrupts.clone(),
        wasi,
        table: ResourceTable::new(),
        limits: StoreLimitsBuilder::new().memory_size(limits.memory).build(),
    };
    let mut store = Store::new(&runtime.engine, data);
    store.limiter(|data| &mut data.limits);
    store.epoch_deadline_callback(check_call);
    store.set_epoch_deadline(1);
    let bindings = bindings::Plugin::instantiate(&mut store, &component, &runtime.linker)
        .map_err(|err| format!("{err:#}"))?;
    plugin.instance = Some(Instance {
        store,
        bindings,
        stderr,
    });

    let config = plugin.config.clone();
    // A base starting says its mode anew; the one before is gone.
    if plugin.base
        && let Some(state) = state
    {
        state.mode = crate::events::Mode::default();
    }
    let result = call_in(plugins, state, id, limits.init, |bindings, store| {
        bindings.nib_plugin_guest().call_init(store, &config)
    });
    let result = match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(message)) => Err(message),
        Err(err) => Err(describe_failure(&plugins.entries[id], &err)),
    };
    if let Err(message) = &result {
        let plugin = &mut plugins.entries[id];
        plugin.last_error = Some(message.clone());
        plugin.instance = None;
        if let Some(state) = state {
            state.remove_plugin_parts(id);
        }
    }
    result
}

/// Runs on every epoch tick of a call: stops it if Ctrl-g was pressed since
/// it began or its time is up, and lets it go on for a tick otherwise.
fn check_call(store: StoreContextMut<PluginData>) -> wasmtime::Result<UpdateDeadline> {
    let data = store.data();
    if data.interrupts.load(Ordering::Acquire) != data.clock.interrupts {
        return Err(Interrupted.into());
    }
    if data
        .clock
        .limit
        .is_some_and(|limit| data.clock.used() > limit)
    {
        return Ok(UpdateDeadline::Interrupt);
    }
    Ok(UpdateDeadline::Continue(1))
}

/// The lazy plugin not started yet that `name` would be a command of, if no
/// plugin has registered `name`.
fn waiting_for_command(plugins: &Plugins, state: &State, name: &str) -> Option<PluginId> {
    if state.commands.iter().any(|command| command.name == name) {
        return None;
    }
    let (plugin, _) = name.split_once('.')?;
    plugins
        .entries
        .iter()
        .position(|p| p.waiting && p.name == plugin)
}

/// WASI with the plugin's data directory as `/data`, and what
/// `capabilities` allow: the working directory for the file ones, and the
/// host's network for "network". Nothing else.
fn wasi_context(
    capabilities: &[String],
    data_dir: Option<&Path>,
    stderr: MemoryOutputPipe,
) -> Result<WasiCtx, String> {
    let has = |name: &str| capabilities.iter().any(|c| c == name);
    let mut builder = WasiCtx::builder();
    builder.stderr(stderr);
    if let Some(dir) = data_dir {
        let fail = |err: &dyn std::fmt::Display| format!("{}: {err}", dir.display());
        std::fs::create_dir_all(dir).map_err(|err| fail(&err))?;
        builder
            .preopened_dir(dir, "/data", FsPerms::ReadWrite)
            .map_err(|err| fail(&err))?;
    }
    let perms = if has("fs-write") {
        Some(FsPerms::ReadWrite)
    } else if has("fs-read") {
        Some(FsPerms::ReadOnly)
    } else {
        None
    };
    if let Some(perms) = perms {
        builder
            .preopened_dir(".", ".", perms)
            .map_err(|err| format!("opening the working directory failed: {err}"))?;
    }
    if has("network") {
        builder
            .inherit_network()
            .allow_ip_name_lookup(true)
            .allow_tcp(true)
            .allow_udp(true);
    }
    Ok(builder.build())
}

fn describe_failure(plugin: &Plugin, err: &wasmtime::Error) -> String {
    if let Some(Trap::Interrupt) = err.downcast_ref::<Trap>() {
        return "it took too long".into();
    }
    if err.downcast_ref::<Interrupted>().is_some() {
        return Interrupted.to_string();
    }
    // A Rust plugin prints its panic message to stderr before trapping.
    let panic = plugin
        .instance
        .as_ref()
        .and_then(|instance| panic_message(&instance.stderr.contents()));
    panic.unwrap_or_else(|| format!("{err}"))
}

/// Finds the message in Rust's "thread '...' panicked at file:line:col:"
/// output, which is on the line after that header.
fn panic_message(stderr: &[u8]) -> Option<String> {
    let stderr = String::from_utf8_lossy(stderr);
    let mut lines = stderr.lines();
    lines.find(|line| line.contains("panicked at"))?;
    lines.next().map(|line| format!("panicked: {line}"))
}

/// A plugin's manifest, and its code if it has any.
type ReadPlugin = (manifest::Manifest, Option<Cow<'static, [u8]>>);

fn compile_component(
    engine: &Engine,
    manifest: &manifest::Manifest,
    wasm: &[u8],
) -> Result<Component, Error> {
    Component::new(engine, wasm).map_err(|err| Error::Plugin(format!("{}: {err:#}", manifest.name)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_panic_message() {
        let stderr = b"thread '<unnamed>' panicked at src/lib.rs:36:30:\nasked to panic\nnote: run with `RUST_BACKTRACE=1`\n";
        assert_eq!(
            panic_message(stderr).as_deref(),
            Some("panicked: asked to panic")
        );
        assert_eq!(panic_message(b"just output\n"), None);
    }
}
