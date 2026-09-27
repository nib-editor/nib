//! Syntax trees with tree-sitter. Grammars are WebAssembly modules that
//! plugins provide; parsing and highlighting stay in the core because every
//! plugin shares them and they run on every edit and frame.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::thread;

use ropey::Rope;
use streaming_iterator::StreamingIterator;
use tree_sitter::{
    InputEdit, Language, Node, Parser, Point, Query, QueryCursor, Range as TsRange, TextProvider,
    Tree, WasmStore,
};
use wasmtime::{Cache, CacheConfig, Config, Engine};

use crate::background::Inbox;
use crate::grid::Style;
use crate::ui::Theme;

pub(crate) struct Languages {
    /// Created with the first grammar. Unlike the plugin engine, it has no
    /// epoch interruption, which tree-sitter's stores do not expect.
    engine: Option<Engine>,
    /// For loading grammars, and for what the main thread parses: injected
    /// layers, and trees the syntax API needs before the thread is done.
    parser: Parser,
    list: Vec<Entry>,
    pub cache_dir: Option<PathBuf>,
    /// Set when buffers are parsed on the syntax thread, to wake the main
    /// loop when a tree is ready (docs/architecture.md, "解析のスレッド").
    background: Option<Arc<Inbox>>,
    /// Started with the first parse it gets.
    worker: Option<Worker>,
    last_job: u64,
    /// Where injected layers changed while updating them, for the buffer's
    /// kept colors to be painted again there.
    repaint: Vec<Range<usize>>,
}

/// The syntax thread: it parses what it is sent, one job after another.
struct Worker {
    jobs: mpsc::Sender<Job>,
    done: mpsc::Receiver<Done>,
}

struct Job {
    id: u64,
    language: Language,
    /// Only these of the text, for an injected layer; all of it if empty.
    ranges: Vec<TsRange>,
    text: Rope,
    old: Option<Tree>,
}

pub(crate) struct Done {
    id: u64,
    tree: Option<Tree>,
}

impl Done {
    pub fn id(&self) -> u64 {
        self.id
    }
}

impl Worker {
    fn start(engine: &Engine, inbox: Arc<Inbox>) -> Result<Self, String> {
        // A store of its own: languages loaded in the main thread's store
        // are added to this one when first used.
        let mut parser = Parser::new();
        let store = WasmStore::new(engine).map_err(|err| err.to_string())?;
        parser
            .set_wasm_store(store)
            .map_err(|err| err.to_string())?;
        let (jobs, job_queue) = mpsc::channel::<Job>();
        let (finished, done) = mpsc::channel();
        thread::Builder::new()
            .name("nib-syntax".into())
            .spawn(move || {
                for job in job_queue {
                    let tree = parse_text(
                        &mut parser,
                        &job.language,
                        &job.ranges,
                        &job.text,
                        job.old.as_ref(),
                    );
                    if finished.send(Done { id: job.id, tree }).is_err() {
                        return;
                    }
                    inbox.wake();
                }
            })
            .map_err(|err| err.to_string())?;
        Ok(Self { jobs, done })
    }
}

/// Parses `ranges` of `text` (all of it if there are none), reusing `old`.
fn parse_text(
    parser: &mut Parser,
    language: &Language,
    ranges: &[TsRange],
    text: &Rope,
    old: Option<&Tree>,
) -> Option<Tree> {
    if parser.set_language(language).is_err() || parser.set_included_ranges(ranges).is_err() {
        return None;
    }
    let mut read = |byte: usize, _: Point| -> &[u8] {
        if byte >= text.len_bytes() {
            return &[];
        }
        let (chunk, start, _, _) = text.chunk_at_byte(byte);
        &chunk.as_bytes()[byte - start..]
    };
    parser.parse_with_options(&mut read, old, None)
}

struct Entry {
    name: String,
    file_types: Vec<String>,
    state: EntryState,
}

/// Grammars load the first time a file of their type is shown, so languages
/// that are not used cost nothing at startup.
// One per language, so its size does not matter.
#[allow(clippy::large_enum_variant)]
enum EntryState {
    Pending {
        grammar: Cow<'static, [u8]>,
        /// Sources by name.
        queries: BTreeMap<String, String>,
    },
    Loaded {
        language: Language,
        highlights: Option<Query>,
        /// Run after every parse, so compiled with the grammar.
        injections: Option<Query>,
        /// Compiling `highlights` and `injections`, which for Rust takes as
        /// long as parsing a file of a few thousand lines, alongside the
        /// first parse.
        compiling: Option<thread::JoinHandle<CompiledQueries>>,
        /// The other queries, compiled when first run.
        queries: BTreeMap<String, QueryState>,
    },
    Failed(String),
}

type CompiledQueries = Result<(Option<Query>, Option<Query>), String>;

enum QueryState {
    Source(String),
    Ready(Query),
    /// Reported once; runs as a query without matches afterwards.
    Failed,
}

/// A syntax node as the plugin API hands it out: a value, since tree-sitter
/// nodes borrow their tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeInfo {
    pub id: u64,
    pub kind: String,
    pub named: bool,
    pub range: Range<usize>,
}

/// The syntax of one buffer.
pub(crate) struct BufferSyntax {
    pub language: usize,
    /// `None` until parsed, and after changes too big to apply as an edit.
    pub tree: Option<Tree>,
    /// The text changed since `tree` was parsed.
    pub dirty: bool,
    /// Where the text changed since the last parse.
    edited: Option<Range<usize>>,
    /// The last tree the injections were brought up to date with, moved
    /// along with the edits since, to find where they may have changed.
    base: Option<Tree>,
    /// The parse under way on the syntax thread, and the edits since it
    /// started, to apply to its tree.
    job: Option<u64>,
    since_job: Vec<InputEdit>,
    injections: Injections,
    /// The colors of the last frame, kept for the next (docs/architecture.md,
    /// "構文木"). Drawing only reads the syntax, hence the cell.
    painted: RefCell<Option<Painted>>,
    /// The first parse leaves injections for later, so a file just opened
    /// shows the colors of its own language sooner.
    injections_pending: bool,
}

/// Highlight colors of a range of the text, as painted for a frame.
struct Painted {
    range: Range<usize>,
    /// One per byte of `range`.
    styles: Vec<Option<Style>>,
    /// Where the colors are no longer right, to paint again first.
    stale: Vec<Range<usize>>,
}

/// How deep injections nest: Rust, the Markdown of its doc comments, the
/// Rust of their code blocks, and the Markdown of those doc comments.
const MAX_DEPTH: usize = 4;

/// A language injected into another, such as the code blocks of Markdown
/// or the doc comments of Rust, parsed from only its ranges of the text.
/// Positions in its tree count from the start of the buffer.
struct Layer {
    key: LayerKey,
    language: usize,
    /// Sorted and apart; never empty.
    ranges: Vec<TsRange>,
    /// Only near the screen: a Markdown file has a layer for every
    /// paragraph, and parsing them all, or telling them all about every
    /// edit, takes milliseconds.
    tree: Option<Tree>,
    /// An edit reached into `ranges` since the layer was parsed.
    touched: bool,
    injections: Injections,
    /// How deep it is among injections, 1 for a layer in the buffer's own
    /// language.
    depth: usize,
    /// The language whose tree it was found in.
    parent: Option<usize>,
    /// Its parse under way on the syntax thread, and the edits since it
    /// started.
    job: Option<u64>,
    since_job: Vec<InputEdit>,
    /// Where the text changed since the layer's own injections were last
    /// looked for, to look again there once it is parsed on the thread.
    edited: Option<Range<usize>>,
}

/// The layer whose parse is `job`.
fn find_job(injections: &mut Injections, job: u64) -> Option<&mut Layer> {
    for layer in &mut injections.layers {
        if layer.job == Some(job) {
            return Some(layer);
        }
        if let Some(found) = find_job(&mut layer.injections, job) {
            return Some(found);
        }
    }
    None
}

/// The parses of layers under way.
fn layer_jobs(injections: &Injections, jobs: &mut Vec<u64>) {
    for layer in &injections.layers {
        jobs.extend(layer.job);
        layer_jobs(&layer.injections, jobs);
    }
}

/// Stops waiting for the parses of layers, as when the thread is gone.
/// They count as edited, so they are parsed again.
fn forget_layer_jobs(injections: &mut Injections) {
    for layer in &mut injections.layers {
        if layer.job.take().is_some() {
            layer.since_job.clear();
            layer.touched = true;
        }
        forget_layer_jobs(&mut layer.injections);
    }
}

/// The languages injected into a tree.
#[derive(Default)]
struct Injections {
    /// What the injections query found, in order. Kept to look again only
    /// where the tree changed: the query over a whole tree of a few thousand
    /// lines takes milliseconds.
    found: Vec<Injection>,
    layers: Vec<Layer>,
}

/// One match of an injections query.
struct Injection {
    language: usize,
    /// The pattern, if all its matches of the same language make one layer
    /// (`injection.combined`).
    combined: Option<usize>,
    /// Sorted and apart; never empty.
    ranges: Vec<TsRange>,
}

impl Injection {
    fn key(&self) -> LayerKey {
        match self.combined {
            Some(pattern) => LayerKey::Combined(pattern, self.language),
            None => LayerKey::At(self.language, self.ranges[0].start_byte),
        }
    }

    fn span(&self) -> (usize, usize) {
        let last = self.ranges.last().expect("never empty");
        (self.ranges[0].start_byte, last.end_byte)
    }
}

/// Where to look for injections: a tree, which covers `host` (all of the
/// text if empty), and the regions to look in (everywhere if `None`).
struct Found<'a> {
    tree: &'a Tree,
    host: &'a [TsRange],
    regions: Option<&'a [Range<usize>]>,
    /// The language whose tree this one is injected into, for
    /// `injection.parent`; `None` for a buffer's own tree.
    parent: Option<usize>,
}

/// Which layer after a parse is which before it.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum LayerKey {
    /// A layer of one match, by where it starts.
    At(usize, usize),
    /// The layer of all the matches of a combined pattern and language,
    /// whose first match can come and go.
    Combined(usize, usize),
}

/// The layers that `found` makes, of those with the keys `only` if given:
/// one for each match, but one for all the matches of a combined pattern
/// and language.
fn group(
    found: &[Injection],
    only: Option<&HashSet<LayerKey>>,
) -> Vec<(LayerKey, usize, Vec<TsRange>)> {
    let mut layers = Vec::new();
    let mut combined: Vec<(LayerKey, usize, Vec<TsRange>)> = Vec::new();
    for injection in found {
        let key = injection.key();
        if only.is_some_and(|only| !only.contains(&key)) {
            continue;
        }
        let (language, ranges) = (injection.language, injection.ranges.iter().copied());
        match key {
            LayerKey::At(..) => layers.push((key, language, ranges.collect())),
            LayerKey::Combined(..) => match combined.iter_mut().find(|(k, _, _)| *k == key) {
                Some((_, _, all)) => all.extend(ranges),
                None => combined.push((key, language, ranges.collect())),
            },
        }
    }
    for (key, language, mut ranges) in combined {
        // tree-sitter wants them in order and apart.
        ranges.sort_by_key(|r| r.start_byte);
        let mut end = 0;
        ranges.retain(|r| {
            let apart = r.start_byte >= end;
            end = end.max(r.end_byte);
            apart
        });
        layers.push((key, language, ranges));
    }
    layers
}

/// Where to look for injections again after parsing `old` into `new`:
/// where the syntax changed, and where the text did, since an edit inside
/// a comment changes no syntax but the text an injection covers.
fn changed_regions(old: &Tree, new: &Tree, edited: Option<Range<usize>>) -> Vec<Range<usize>> {
    let mut regions: Vec<Range<usize>> = old
        .changed_ranges(new)
        .map(|r| r.start_byte..r.end_byte)
        .chain(edited)
        // Matches that only touch the edges count too.
        .map(|r| r.start.saturating_sub(1)..r.end + 1)
        .collect();
    regions.sort_by_key(|r| r.start);
    let mut merged: Vec<Range<usize>> = Vec::new();
    for region in regions {
        match merged.last_mut() {
            Some(last) if region.start <= last.end => last.end = last.end.max(region.end),
            _ => merged.push(region),
        }
    }
    merged
}

impl Default for Languages {
    fn default() -> Self {
        Self {
            engine: None,
            parser: Parser::new(),
            list: Vec::new(),
            cache_dir: None,
            background: None,
            worker: None,
            last_job: 0,
            repaint: Vec::new(),
        }
    }
}

impl Languages {
    /// Registers a language, replacing one of the same name. Its grammar is
    /// loaded when first needed.
    pub fn add(
        &mut self,
        name: &str,
        file_types: Vec<String>,
        grammar: Cow<'static, [u8]>,
        queries: BTreeMap<String, String>,
    ) {
        let entry = Entry {
            name: name.to_string(),
            file_types,
            state: EntryState::Pending { grammar, queries },
        };
        match self.list.iter().position(|e| e.name == name) {
            Some(i) => self.list[i] = entry,
            None => self.list.push(entry),
        }
    }

    fn ensure_loaded(&mut self, id: usize) -> Result<(), String> {
        match &self.list[id].state {
            EntryState::Loaded { .. } => return Ok(()),
            EntryState::Failed(err) => return Err(err.clone()),
            EntryState::Pending { .. } => {}
        }
        let EntryState::Pending {
            grammar,
            mut queries,
        } = std::mem::replace(&mut self.list[id].state, EntryState::Failed(String::new()))
        else {
            unreachable!("checked above");
        };
        let name = self.list[id].name.clone();
        let highlights = queries.remove("highlights");
        let injections = queries.remove("injections");
        self.list[id].state = match self.load(&name, &grammar) {
            Ok(language) => EntryState::Loaded {
                compiling: Some(compile_queries(&language, highlights, injections)),
                language,
                highlights: None,
                injections: None,
                queries: queries
                    .into_iter()
                    .map(|(name, source)| (name, QueryState::Source(source)))
                    .collect(),
            },
            Err(err) => EntryState::Failed(format!("{name}: {err}")),
        };
        match &self.list[id].state {
            EntryState::Failed(err) => Err(err.clone()),
            _ => Ok(()),
        }
    }

    /// Waits for the queries of language `id` to compile, if they still
    /// are. A query that fails to compile fails the language.
    fn finish_loading(&mut self, id: usize) -> Result<(), String> {
        let entry = &mut self.list[id];
        let EntryState::Loaded {
            compiling,
            highlights,
            injections,
            ..
        } = &mut entry.state
        else {
            return Ok(());
        };
        let Some(compiling) = compiling.take() else {
            return Ok(());
        };
        match compiling.join() {
            Ok(Ok(compiled)) => {
                (*highlights, *injections) = compiled;
                Ok(())
            }
            failed => {
                let err = match failed {
                    Ok(Err(err)) => format!("{}: {err}", entry.name),
                    _ => format!("{}: compiling its queries crashed", entry.name),
                };
                entry.state = EntryState::Failed(err.clone());
                Err(err)
            }
        }
    }

    fn load(&mut self, name: &str, grammar: &[u8]) -> Result<Language, String> {
        if self.engine.is_none() {
            let mut config = Config::new();
            if let Some(dir) = &self.cache_dir {
                let mut cache = CacheConfig::new();
                cache.with_directory(dir);
                config.cache(Some(Cache::new(cache).map_err(|err| err.to_string())?));
            }
            self.engine = Some(Engine::new(&config).map_err(|err| err.to_string())?);
        }
        let engine = self.engine.as_ref().expect("created above");
        let mut store = match self.parser.take_wasm_store() {
            Some(store) => store,
            None => WasmStore::new(engine).map_err(|err| err.to_string())?,
        };
        let loaded = store.load_language(name, grammar);
        self.parser
            .set_wasm_store(store)
            .map_err(|err| err.to_string())?;
        loaded.map_err(|err| err.to_string())
    }

    pub fn name(&self, id: usize) -> &str {
        &self.list[id].name
    }

    /// The language for a file, by its extension.
    pub fn for_path(&self, path: &Path) -> Option<usize> {
        let extension = path.extension()?.to_str()?;
        self.list
            .iter()
            .position(|e| e.file_types.iter().any(|t| t == extension))
    }

    /// The language an injection names, by its name or a file type, as in
    /// the info string of a Markdown code block.
    fn for_injection(&self, name: &str) -> Option<usize> {
        let name = name.to_ascii_lowercase();
        self.list
            .iter()
            .position(|e| e.name == name)
            .or_else(|| self.list.iter().position(|e| e.file_types.contains(&name)))
    }

    /// Parses the syntax of a buffer: its tree, reusing the old one for the
    /// parts that did not change, and the languages injected into it. Loads
    /// grammars first if needed.
    pub fn parse(&mut self, syntax: &mut BufferSyntax, text: &Rope) -> Result<(), String> {
        let tree = self.parse_in(syntax.language, text, &[], syntax.tree.as_ref())?;
        // This overtakes a parse on the syntax thread; its tree is dropped.
        syntax.job = None;
        syntax.since_job.clear();
        self.take_tree(syntax, tree, text);
        Ok(())
    }

    /// Whether buffers are parsed on the syntax thread.
    pub fn in_background(&self) -> bool {
        self.background.is_some()
    }

    /// Parses on the syntax thread, waking `inbox`'s loop when a tree is
    /// ready, or on the main thread again if `None`.
    pub fn set_background(&mut self, inbox: Option<Arc<Inbox>>) {
        self.background = inbox;
        self.worker = None;
    }

    /// Sends the buffer to the syntax thread if it changed since its last
    /// parse and no parse of it is under way.
    pub fn start_parse(&mut self, syntax: &mut BufferSyntax, text: &Rope) -> Result<(), String> {
        if !syntax.dirty || syntax.job.is_some() {
            return Ok(());
        }
        let id = self.send_job(syntax.language, Vec::new(), text, syntax.tree.clone())?;
        syntax.job = Some(id);
        syntax.since_job.clear();
        Ok(())
    }

    /// Sends a parse to the syntax thread, starting it if needed. Returns
    /// the job's id.
    fn send_job(
        &mut self,
        language: usize,
        ranges: Vec<TsRange>,
        text: &Rope,
        old: Option<Tree>,
    ) -> Result<u64, String> {
        self.ensure_loaded(language)?;
        let EntryState::Loaded { language, .. } = &self.list[language].state else {
            unreachable!("loaded above");
        };
        let language = language.clone();
        if self.worker.is_none() {
            let engine = self.engine.as_ref().expect("made when the grammar loaded");
            let inbox = self
                .background
                .clone()
                .ok_or("not parsing in the background")?;
            self.worker = Some(Worker::start(engine, inbox)?);
        }
        self.last_job += 1;
        let job = Job {
            id: self.last_job,
            language,
            ranges,
            text: text.clone(),
            old,
        };
        let worker = self.worker.as_ref().expect("started above");
        worker
            .jobs
            .send(job)
            .map_err(|_| "the syntax thread stopped".to_string())?;
        Ok(self.last_job)
    }

    /// Takes a parse the syntax thread finished for an injected layer.
    /// Returns whether its colors changed.
    pub fn take_layer_parse(&mut self, syntax: &mut BufferSyntax, done: Done, text: &Rope) -> bool {
        let Some(layer) = find_job(&mut syntax.injections, done.id) else {
            return false;
        };
        layer.job = None;
        let Some(mut tree) = done.tree else {
            return false;
        };
        // Its queries may still be compiling, as when its grammar loaded
        // for this parse. If they failed, the layer shows as text.
        if self.finish_loading(layer.language).is_err() {
            return false;
        }
        if !layer.since_job.is_empty() {
            // Parsed again when its host's tree is next taken in.
            for edit in layer.since_job.drain(..) {
                tree.edit(&edit);
            }
            layer.tree = Some(tree);
            layer.touched = true;
            return false;
        }
        let edited = layer.edited.take();
        let regions = layer
            .tree
            .as_ref()
            .map(|old| changed_regions(old, &tree, edited.clone()));
        match &regions {
            Some(regions) => self.repaint.extend(regions.iter().cloned()),
            None => self.repaint.extend(byte_ranges(&layer.ranges)),
        }
        let found = Found {
            tree: &tree,
            host: &layer.ranges,
            regions: regions.as_deref(),
            parent: layer.parent,
        };
        let (language, depth) = (layer.language, layer.depth);
        self.inject(
            language,
            found,
            text,
            &mut layer.injections,
            edited.as_ref(),
            depth + 1,
        );
        layer.tree = Some(tree);
        syntax.repaint(std::mem::take(&mut self.repaint));
        true
    }

    /// The parses the syntax thread has finished, after waiting for `job`
    /// if it is given.
    pub fn finished(&mut self, job: Option<u64>) -> Vec<Done> {
        let Some(worker) = &self.worker else {
            return Vec::new();
        };
        let mut done: Vec<Done> = worker.done.try_iter().collect();
        if let Some(job) = job {
            while !done.iter().any(|d| d.id == job) {
                match worker.done.recv() {
                    Ok(finished) => done.push(finished),
                    Err(_) => break,
                }
            }
        }
        done
    }

    /// Takes a parse the syntax thread finished for `syntax`. Returns
    /// whether the tree is now up to date: when the text changed during
    /// the parse, the changes are applied to the new tree, and it waits for
    /// the next one.
    pub fn take_parse(
        &mut self,
        syntax: &mut BufferSyntax,
        done: Done,
        text: &Rope,
    ) -> Result<bool, String> {
        syntax.job = None;
        self.finish_loading(syntax.language)?;
        let Some(mut tree) = done.tree else {
            return Err(format!(
                "{} could not be parsed",
                self.list[syntax.language].name
            ));
        };
        if syntax.since_job.is_empty() {
            self.take_tree(syntax, Some(tree), text);
            return Ok(true);
        }
        for edit in syntax.since_job.drain(..) {
            tree.edit(&edit);
        }
        syntax.tree = Some(tree);
        Ok(false)
    }

    /// Takes `tree`, parsed from the text as it is now, and brings the
    /// injections up to date with what changed since the last such tree.
    fn take_tree(&mut self, syntax: &mut BufferSyntax, tree: Option<Tree>, text: &Rope) {
        let old = syntax.base.take();
        syntax.tree = tree;
        syntax.base = syntax.tree.clone();
        syntax.dirty = false;
        let edited = syntax.edited.take();
        let regions = match (&old, &syntax.tree) {
            (Some(old), Some(tree)) => Some(changed_regions(old, tree, edited.clone())),
            _ => None,
        };
        // The colors kept were painted with the old tree, which differs
        // from the new one only there.
        match &regions {
            Some(regions) => syntax.repaint(regions.iter().cloned()),
            None => syntax.forget_colors(),
        }
        if syntax.injections_pending {
            return;
        }
        match &syntax.tree {
            Some(tree) => {
                let (language, injections) = (syntax.language, &mut syntax.injections);
                let found = Found {
                    tree,
                    host: &[],
                    regions: regions.as_deref(),
                    parent: None,
                };
                self.inject(language, found, text, injections, edited.as_ref(), 1);
                let changed = std::mem::take(&mut self.repaint);
                syntax.repaint(changed);
            }
            None => syntax.injections = Injections::default(),
        }
    }

    /// Finds the injections the first parse left for later, then parses
    /// the layers in `view`. Returns whether it parsed any.
    pub fn update_injections(
        &mut self,
        syntax: &mut BufferSyntax,
        text: &Rope,
        view: &[Range<usize>],
    ) -> bool {
        let Some(tree) = &syntax.tree else {
            return false;
        };
        if syntax.injections_pending {
            syntax.injections_pending = false;
            let found = Found {
                tree,
                host: &[],
                regions: None,
                parent: None,
            };
            let (language, injections) = (syntax.language, &mut syntax.injections);
            self.inject(language, found, text, injections, None, 1);
        }
        let parsed = self.fill(&mut syntax.injections, text, view, None, 1);
        syntax.repaint(std::mem::take(&mut self.repaint));
        parsed
    }

    /// Parses the layers `near` the screen ahead of time, and drops the
    /// trees of those outside `keep`.
    pub fn prefetch_injections(
        &mut self,
        syntax: &mut BufferSyntax,
        text: &Rope,
        near: &[Range<usize>],
        keep: &[Range<usize>],
    ) {
        self.fill(&mut syntax.injections, text, near, Some(keep), 1);
        syntax.repaint(std::mem::take(&mut self.repaint));
    }

    fn fill(
        &mut self,
        injections: &mut Injections,
        text: &Rope,
        wanted: &[Range<usize>],
        keep: Option<&[Range<usize>]>,
        depth: usize,
    ) -> bool {
        let in_any = |layer: &Layer, ranges: &[Range<usize>]| {
            ranges.iter().any(|r| overlaps(&layer.ranges, r))
        };
        let mut parsed = false;
        for layer in &mut injections.layers {
            if keep.is_some_and(|keep| !in_any(layer, keep)) {
                layer.job = None;
                if layer.tree.take().is_some() {
                    layer.injections = Injections::default();
                    self.repaint.extend(byte_ranges(&layer.ranges));
                }
                continue;
            }
            if !in_any(layer, wanted) {
                continue;
            }
            if layer.tree.is_none() && self.in_background() {
                if layer.job.is_none()
                    && let Ok(job) = self.send_job(layer.language, layer.ranges.clone(), text, None)
                {
                    layer.job = Some(job);
                }
                continue;
            }
            if layer.tree.is_none() {
                // A grammar that fails to load was reported when a file of
                // its own type was opened, or will be; here it just shows as
                // text.
                let Ok(Some(tree)) = self.parse_in(layer.language, text, &layer.ranges, None)
                else {
                    continue;
                };
                let found = Found {
                    tree: &tree,
                    host: &layer.ranges,
                    regions: None,
                    parent: layer.parent,
                };
                self.inject(
                    layer.language,
                    found,
                    text,
                    &mut layer.injections,
                    None,
                    depth + 1,
                );
                layer.tree = Some(tree);
                layer.edited = None;
                self.repaint.extend(byte_ranges(&layer.ranges));
                parsed = true;
            }
            parsed |= self.fill(&mut layer.injections, text, wanted, keep, depth + 1);
        }
        parsed
    }

    /// Parses only `ranges` of `text`, or all of it if there are none.
    fn parse_in(
        &mut self,
        id: usize,
        text: &Rope,
        ranges: &[TsRange],
        old: Option<&Tree>,
    ) -> Result<Option<Tree>, String> {
        self.ensure_loaded(id)?;
        let EntryState::Loaded { language, .. } = &self.list[id].state else {
            unreachable!("loaded above");
        };
        let tree = parse_text(&mut self.parser, language, ranges, text, old);
        self.finish_loading(id)?;
        Ok(tree)
    }

    /// Brings the injections of a tree of `language` up to date: looks for
    /// them again where the tree changed, keeps the layers whose text did
    /// not change, and parses the others again from their old trees if they
    /// have them. `fill` parses the rest once they come near the screen.
    fn inject(
        &mut self,
        language: usize,
        found: Found,
        text: &Rope,
        injections: &mut Injections,
        edited: Option<&Range<usize>>,
        depth: usize,
    ) {
        if depth > MAX_DEPTH {
            return;
        }
        let host = language;
        let changed = self.find_injections(language, &found, text, &mut injections.found);
        if changed.as_ref().is_some_and(HashSet::is_empty) {
            return;
        }
        // Only the layers that changed are taken apart; a Markdown file has
        // thousands.
        let mut old = HashMap::new();
        let mut i = 0;
        while i < injections.layers.len() {
            let layer = &mut injections.layers[i];
            // Where it starts moved along with the edits.
            if let LayerKey::At(language, _) = layer.key {
                layer.key = LayerKey::At(language, layer.ranges[0].start_byte);
            }
            if changed.as_ref().is_some_and(|c| !c.contains(&layer.key)) {
                i += 1;
                continue;
            }
            let layer = injections.layers.swap_remove(i);
            old.insert(layer.key, layer);
        }
        let mut grouped = group(&injections.found, changed.as_ref());
        // Switching between grammars costs, so each is parsed in a run.
        grouped.sort_by_key(|(_, language, _)| *language);
        for (key, language, ranges) in grouped {
            let old = old.remove(&key);
            let unchanged = old
                .as_ref()
                .is_some_and(|layer| !layer.touched && same_bytes(&layer.ranges, &ranges));
            if unchanged && let Some(mut layer) = old {
                layer.key = key;
                layer.ranges = ranges;
                injections.layers.push(layer);
                continue;
            }
            let mut layer = Layer {
                key,
                language,
                ranges,
                tree: None,
                touched: false,
                injections: Injections::default(),
                depth,
                parent: Some(host),
                job: None,
                since_job: Vec::new(),
                edited: None,
            };
            if let Some(Layer {
                tree: Some(mut old_tree),
                ranges: old_ranges,
                injections: mut inner,
                edited: old_edited,
                ..
            }) = old
            {
                mark_range_changes(&mut old_tree, &old_ranges, &layer.ranges);
                if self.in_background() {
                    // Shown with the old tree until the thread is done.
                    if let Ok(job) =
                        self.send_job(language, layer.ranges.clone(), text, Some(old_tree.clone()))
                    {
                        layer.job = Some(job);
                    }
                    layer.edited = old_edited;
                    layer.tree = Some(old_tree);
                    layer.injections = inner;
                } else if let Ok(Some(tree)) =
                    self.parse_in(language, text, &layer.ranges, Some(&old_tree))
                {
                    let regions = changed_regions(&old_tree, &tree, edited.cloned());
                    self.repaint.extend(regions.iter().cloned());
                    let found = Found {
                        tree: &tree,
                        host: &layer.ranges,
                        regions: Some(&regions),
                        parent: layer.parent,
                    };
                    self.inject(language, found, text, &mut inner, edited, depth + 1);
                    layer.tree = Some(tree);
                    layer.injections = inner;
                }
            } else {
                self.repaint.extend(byte_ranges(&layer.ranges));
            }
            injections.layers.push(layer);
        }
        // Layers no longer found took their colors with them.
        for gone in old.values() {
            self.repaint.extend(byte_ranges(&gone.ranges));
        }
    }

    /// Runs the `injections` query of `language` over the regions of
    /// `found`, and puts what it finds there in place of what `injections`
    /// had. Follows tree-sitter's conventions: the captures
    /// `@injection.content` and `@injection.language`, and the settings
    /// `injection.language`, `injection.combined`, and
    /// `injection.include-children`.
    /// Returns the keys of the layers that changed, or `None` if all may
    /// have.
    fn find_injections(
        &self,
        language: usize,
        found: &Found,
        text: &Rope,
        injections: &mut Vec<Injection>,
    ) -> Option<HashSet<LayerKey>> {
        let EntryState::Loaded {
            injections: Some(query),
            ..
        } = &self.list[language].state
        else {
            injections.clear();
            return None;
        };
        let Some(content) = query.capture_index_for_name("injection.content") else {
            injections.clear();
            return None;
        };
        let named = query.capture_index_for_name("injection.language");
        let everywhere = 0..usize::MAX;
        let mut new = Vec::new();
        let mut cursor = QueryCursor::new();
        for region in found.regions.unwrap_or(std::slice::from_ref(&everywhere)) {
            cursor.set_byte_range(region.clone());
            let mut matches = cursor.matches(query, found.tree.root_node(), node_text(text));
            while let Some(m) = matches.next() {
                let settings = query.property_settings(m.pattern_index);
                let setting = |key: &str| settings.iter().find(|p| &*p.key == key);
                let name = match m.captures().iter().find(|c| Some(c.index) == named) {
                    Some(c) => text.byte_slice(c.node.byte_range()).to_string(),
                    // The language this tree is injected into, as for code
                    // blocks with no language in Rust's doc comments.
                    None if setting("injection.parent").is_some() => match found.parent {
                        Some(parent) => self.list[parent].name.clone(),
                        None => continue,
                    },
                    None => match setting("injection.language").and_then(|p| p.value.as_deref()) {
                        Some(name) => name.to_string(),
                        None => continue,
                    },
                };
                // "rust,ignore" and "python title=x" name their language
                // first.
                let name = name
                    .trim()
                    .split(|c: char| c.is_whitespace() || c == ',' || c == '{')
                    .next()
                    .unwrap_or_default();
                let Some(injected) = self.for_injection(name) else {
                    continue;
                };
                let include_children = setting("injection.include-children").is_some();
                let ranges: Vec<TsRange> = m
                    .captures()
                    .iter()
                    .filter(|c| c.index == content)
                    .flat_map(|c| content_ranges(c.node, include_children, found.host))
                    .filter(|r| r.start_byte < r.end_byte)
                    .collect();
                if !ranges.is_empty() {
                    new.push(Injection {
                        language: injected,
                        combined: setting("injection.combined").map(|_| m.pattern_index),
                        ranges,
                    });
                }
            }
        }
        let Some(regions) = found.regions else {
            *injections = new;
            return None;
        };
        // A match can reach into two regions.
        new.sort_by_key(|i| i.ranges[0].start_byte);
        new.dedup_by(|a, b| a.language == b.language && same_bytes(&a.ranges, &b.ranges));
        let mut changed: HashSet<LayerKey> = new.iter().map(Injection::key).collect();
        injections.retain(|old| {
            let (start, end) = old.span();
            let stays = !regions.iter().any(|r| r.start <= end && start <= r.end)
                && !new.iter().any(|n| {
                    let (s, e) = n.span();
                    s < end && start < e
                });
            if !stays {
                changed.insert(old.key());
            }
            stays
        });
        injections.append(&mut new);
        injections.sort_by_key(|i| i.ranges[0].start_byte);
        Some(changed)
    }

    /// The highlight style of each byte in `range`, or `None` for plain text.
    /// Injected languages paint over the language around them.
    pub fn highlight(
        &self,
        theme: &Theme,
        syntax: &BufferSyntax,
        text: &Rope,
        range: Range<usize>,
    ) -> Vec<Option<Style>> {
        let mut painted = syntax.painted.borrow_mut();
        if let Some(kept) = painted
            .as_mut()
            .filter(|p| p.range.start <= range.start && range.end <= p.range.end)
        {
            for part in merged(std::mem::take(&mut kept.stale), &kept.range) {
                let fresh = self.paint_range(theme, syntax, text, part.clone());
                let at = part.start - kept.range.start;
                kept.styles[at..at + part.len()].copy_from_slice(&fresh);
            }
            let at = range.start - kept.range.start;
            return kept.styles[at..at + range.len()].to_vec();
        }
        let styles = self.paint_range(theme, syntax, text, range.clone());
        *painted = Some(Painted {
            range,
            styles: styles.clone(),
            stale: Vec::new(),
        });
        styles
    }

    fn paint_range(
        &self,
        theme: &Theme,
        syntax: &BufferSyntax,
        text: &Rope,
        range: Range<usize>,
    ) -> Vec<Option<Style>> {
        let mut styles = vec![None; range.len()];
        if let Some(tree) = &syntax.tree {
            self.paint_layer(theme, syntax.language, tree, &[], text, &range, &mut styles);
            self.paint_layers(theme, &syntax.injections.layers, text, &range, &mut styles);
        }
        styles
    }

    fn paint_layers(
        &self,
        theme: &Theme,
        layers: &[Layer],
        text: &Rope,
        range: &Range<usize>,
        styles: &mut [Option<Style>],
    ) {
        for layer in layers {
            // Injected layers lie inside their host's ranges, so ones out of
            // sight hide their own injections too.
            let Some(tree) = &layer.tree else {
                continue;
            };
            if !overlaps(&layer.ranges, range) {
                continue;
            }
            let ranges = &layer.ranges;
            self.paint_layer(theme, layer.language, tree, ranges, text, range, styles);
            self.paint_layers(theme, &layer.injections.layers, text, range, styles);
        }
    }

    /// Paints the highlights of one tree over `styles`, which covers
    /// `range`, but only within `ranges` if there are any.
    #[allow(clippy::too_many_arguments)]
    fn paint_layer(
        &self,
        theme: &Theme,
        language: usize,
        tree: &Tree,
        ranges: &[TsRange],
        text: &Rope,
        range: &Range<usize>,
        styles: &mut [Option<Style>],
    ) {
        let EntryState::Loaded {
            highlights: Some(highlights),
            ..
        } = &self.list[language].state
        else {
            return;
        };
        // Looked up per frame, so theme changes show at once; queries have
        // a few dozen captures.
        let capture_styles: Vec<Option<Style>> = highlights
            .capture_names()
            .iter()
            .map(|name| theme.style(name))
            .collect();
        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(range.clone());
        let mut spans = Vec::new();
        let mut captures = cursor.captures(highlights, tree.root_node(), node_text(text));
        while let Some((found, index)) = captures.next() {
            let capture = found.captures()[*index];
            if let Some(style) = capture_styles[capture.index as usize] {
                spans.push((capture.node.byte_range(), found.pattern_index, style));
            }
        }
        let mask = (!ranges.is_empty()).then(|| clip(ranges, range));
        paint(styles, range.start, spans, mask);
    }

    /// Where `capture` of the query `name` matched in `range`, one range per
    /// match, sorted and without repeats. Compiles the query the first time;
    /// the error comes back once, and later runs find nothing.
    pub fn captures(
        &mut self,
        language: usize,
        name: &str,
        capture: &str,
        tree: &Tree,
        text: &Rope,
        range: Range<usize>,
    ) -> Result<Vec<Range<usize>>, String> {
        let entry = &mut self.list[language];
        let EntryState::Loaded {
            language, queries, ..
        } = &mut entry.state
        else {
            return Ok(Vec::new());
        };
        let Some(state) = queries.get_mut(name) else {
            return Ok(Vec::new());
        };
        if let QueryState::Source(source) = state {
            match Query::new(language, source) {
                Ok(query) => *state = QueryState::Ready(query),
                Err(err) => {
                    *state = QueryState::Failed;
                    return Err(format!("{} {name}: {err}", entry.name));
                }
            }
        }
        let QueryState::Ready(query) = state else {
            return Ok(Vec::new());
        };
        let Some(index) = query.capture_index_for_name(capture) else {
            return Ok(Vec::new());
        };
        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(range);
        let mut found = Vec::new();
        let mut matches = cursor.matches(query, tree.root_node(), node_text(text));
        while let Some(m) = matches.next() {
            let span = m
                .captures()
                .iter()
                .filter(|c| c.index == index)
                .map(|c| c.node.byte_range())
                .reduce(|a, b| a.start.min(b.start)..a.end.max(b.end));
            found.extend(span);
        }
        found.sort_by_key(|r| (r.start, r.end));
        found.dedup();
        Ok(found)
    }
}

fn compile_queries(
    language: &Language,
    highlights: Option<String>,
    injections: Option<String>,
) -> thread::JoinHandle<CompiledQueries> {
    let language = language.clone();
    thread::spawn(move || {
        let compile = |name: &str, source: Option<String>| {
            source
                .map(|source| {
                    Query::new(&language, &source).map_err(|err| format!("{name}: {err}"))
                })
                .transpose()
        };
        Ok((
            compile("highlights", highlights)?,
            compile("injections", injections)?,
        ))
    })
}

fn node_text<'a>(text: &'a Rope) -> impl TextProvider<&'a [u8]> + 'a {
    |node: Node| {
        text.byte_slice(node.byte_range())
            .chunks()
            .map(str::as_bytes)
            .collect::<Vec<_>>()
            .into_iter()
    }
}

fn info(node: Node) -> NodeInfo {
    NodeInfo {
        id: node.id() as u64,
        kind: node.kind().to_string(),
        named: node.is_named(),
        range: node.byte_range(),
    }
}

/// The smallest node covering `range`, or the smallest named one.
pub(crate) fn node_at(tree: &Tree, range: Range<usize>, named: bool) -> Option<NodeInfo> {
    let root = tree.root_node();
    let node = if named {
        root.named_descendant_for_byte_range(range.start, range.end)
    } else {
        root.descendant_for_byte_range(range.start, range.end)
    };
    node.map(info)
}

pub(crate) fn parent(tree: &Tree, of: &NodeInfo) -> Option<NodeInfo> {
    find(tree, of)?.parent().map(info)
}

pub(crate) fn children(tree: &Tree, of: &NodeInfo) -> Vec<NodeInfo> {
    let Some(node) = find(tree, of) else {
        return Vec::new();
    };
    let mut cursor = node.walk();
    node.children(&mut cursor).map(info).collect()
}

/// Finds a node handed out earlier. Nodes with its range cover the smallest
/// node there, so it is among that node's ancestors if it still exists.
fn find<'t>(tree: &'t Tree, of: &NodeInfo) -> Option<Node<'t>> {
    let mut node = tree
        .root_node()
        .descendant_for_byte_range(of.range.start, of.range.end)?;
    loop {
        if node.id() as u64 == of.id {
            return (node.byte_range() == of.range).then_some(node);
        }
        node = node.parent()?;
    }
}

/// Paints `spans` into `styles`, which covers bytes from `offset`, but
/// only within `mask` if there is one. Inner spans win over the ones
/// around them; for the same range, the pattern written first in the query
/// wins, as in tree-sitter's highlighting.
fn paint(
    styles: &mut [Option<Style>],
    offset: usize,
    mut spans: Vec<(Range<usize>, usize, Style)>,
    mask: Option<Vec<Range<usize>>>,
) {
    spans.sort_by_key(|(range, pattern, _)| (range.start, std::cmp::Reverse(range.end), *pattern));
    let whole = offset..offset + styles.len();
    let mask = mask.as_deref().unwrap_or(std::slice::from_ref(&whole));
    let mut last: Option<Range<usize>> = None;
    for (range, _, style) in spans {
        if last.as_ref() == Some(&range) {
            continue;
        }
        let first = mask.partition_point(|m| m.end <= range.start);
        for part in &mask[first..] {
            if part.start >= range.end {
                break;
            }
            let start = range
                .start
                .max(part.start)
                .saturating_sub(offset)
                .min(styles.len());
            let end = range
                .end
                .min(part.end)
                .saturating_sub(offset)
                .min(styles.len());
            for slot in &mut styles[start..end] {
                *slot = Some(style);
            }
        }
        last = Some(range);
    }
}

fn overlaps(ranges: &[TsRange], range: &Range<usize>) -> bool {
    let first = ranges.partition_point(|r| r.end_byte <= range.start);
    ranges.get(first).is_some_and(|r| r.start_byte < range.end)
}

/// The byte ranges of `ranges` within `range`.
fn clip(ranges: &[TsRange], range: &Range<usize>) -> Vec<Range<usize>> {
    let first = ranges.partition_point(|r| r.end_byte <= range.start);
    ranges[first..]
        .iter()
        .take_while(|r| r.start_byte < range.end)
        .map(|r| r.start_byte.max(range.start)..r.end_byte.min(range.end))
        .filter(|r| !r.is_empty())
        .collect()
}

/// Tells `tree` about the text that joined or left its ranges without an
/// edit to it, such as a comment made a doc comment: tree-sitter reuses
/// what no edit reached, so it would not read that text otherwise. Each
/// such range becomes an edit that replaces it with itself, from the end
/// of the range before it: text after the end of the tree reaches no node
/// otherwise.
fn mark_range_changes(tree: &mut Tree, old: &[TsRange], new: &[TsRange]) {
    let key = |r: &TsRange| (r.start_byte, r.end_byte);
    let (mut i, mut j) = (0, 0);
    let mut before: Option<(usize, Point)> = None;
    while i < old.len() || j < new.len() {
        let changed = match (old.get(i), new.get(j)) {
            (Some(a), Some(b)) if key(a) == key(b) => {
                i += 1;
                j += 1;
                before = Some((a.end_byte, a.end_point));
                continue;
            }
            (Some(a), Some(b)) if a.start_byte <= b.start_byte => {
                i += 1;
                a
            }
            (Some(a), None) => {
                i += 1;
                a
            }
            (_, Some(b)) => {
                j += 1;
                b
            }
            (None, None) => unreachable!("checked by the loop"),
        };
        let (start_byte, start_position) = before
            .filter(|(byte, _)| *byte <= changed.start_byte)
            .unwrap_or((changed.start_byte, changed.start_point));
        tree.edit(&InputEdit {
            start_byte,
            old_end_byte: changed.end_byte,
            new_end_byte: changed.end_byte,
            start_position,
            old_end_position: changed.end_point,
            new_end_position: changed.end_point,
        });
    }
}

fn byte_ranges(ranges: &[TsRange]) -> impl Iterator<Item = Range<usize>> + '_ {
    ranges.iter().map(|r| r.start_byte..r.end_byte)
}

/// `ranges` cut to `within`, sorted, and joined where they are close, so a
/// layer of many short ranges is painted in a few runs, not one each.
fn merged(mut ranges: Vec<Range<usize>>, within: &Range<usize>) -> Vec<Range<usize>> {
    const CLOSE: usize = 256;
    ranges.retain_mut(|r| {
        *r = r.start.max(within.start)..r.end.min(within.end);
        r.start < r.end
    });
    ranges.sort_by_key(|r| r.start);
    let mut joined: Vec<Range<usize>> = Vec::new();
    for range in ranges {
        match joined.last_mut() {
            Some(last) if range.start <= last.end + CLOSE => last.end = last.end.max(range.end),
            _ => joined.push(range),
        }
    }
    joined
}

fn same_bytes(a: &[TsRange], b: &[TsRange]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(a, b)| (a.start_byte, a.end_byte) == (b.start_byte, b.end_byte))
}

/// The text of an injection's `node`: without its named children unless
/// `include_children`, and within `host` if it is not empty. Unnamed
/// children, such as the `(` in a Markdown code block, are text like the
/// rest.
fn content_ranges(node: Node, include_children: bool, host: &[TsRange]) -> Vec<TsRange> {
    let mut ranges = Vec::new();
    if include_children {
        ranges.push(node.range());
    } else {
        let mut start = (node.start_byte(), node.start_position());
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.start_byte() > start.0 {
                ranges.push(ts_range(
                    start,
                    (child.start_byte(), child.start_position()),
                ));
            }
            if child.end_byte() > start.0 {
                start = (child.end_byte(), child.end_position());
            }
        }
        if node.end_byte() > start.0 {
            ranges.push(ts_range(start, (node.end_byte(), node.end_position())));
        }
    }
    if host.is_empty() {
        return ranges;
    }
    let mut within = Vec::new();
    for range in ranges {
        let first = host.partition_point(|h| h.end_byte <= range.start_byte);
        for h in host[first..]
            .iter()
            .take_while(|h| h.start_byte < range.end_byte)
        {
            let start = if h.start_byte > range.start_byte {
                (h.start_byte, h.start_point)
            } else {
                (range.start_byte, range.start_point)
            };
            let end = if h.end_byte < range.end_byte {
                (h.end_byte, h.end_point)
            } else {
                (range.end_byte, range.end_point)
            };
            if start.0 < end.0 {
                within.push(ts_range(start, end));
            }
        }
    }
    within
}

fn ts_range(start: (usize, Point), end: (usize, Point)) -> TsRange {
    TsRange {
        start_byte: start.0,
        start_point: start.1,
        end_byte: end.0,
        end_point: end.1,
    }
}

impl BufferSyntax {
    pub fn new(language: usize) -> Self {
        Self {
            language,
            tree: None,
            dirty: true,
            edited: None,
            base: None,
            job: None,
            since_job: Vec::new(),
            injections: Injections::default(),
            painted: RefCell::new(None),
            injections_pending: true,
        }
    }

    /// Tells the tree that `old` became `new` between `start` and the ends,
    /// so the next parse only redoes what changed.
    pub fn edit(&mut self, old: &Rope, new: &Rope, start: usize, old_end: usize, new_end: usize) {
        let edit = InputEdit {
            start_byte: start,
            old_end_byte: old_end,
            new_end_byte: new_end,
            start_position: point(old, start),
            old_end_position: point(old, old_end),
            new_end_position: point(new, new_end),
        };
        for tree in [&mut self.tree, &mut self.base].into_iter().flatten() {
            tree.edit(&edit);
        }
        self.move_colors(new, &edit);
        if self.job.is_some() {
            self.since_job.push(edit);
        }
        edit_injections(&mut self.injections, &edit);
        add_edit(&mut self.edited, &edit);
        self.dirty = true;
    }

    /// The parse of this buffer under way on the syntax thread.
    pub fn job(&self) -> Option<u64> {
        self.job
    }

    /// Moves the kept colors along with an edit. Only the edited lines are
    /// painted again: with the old tree moved the same way, the others would
    /// come out as they were.
    fn move_colors(&mut self, text: &Rope, edit: &InputEdit) {
        let painted = self.painted.get_mut();
        let Some(kept) = painted else {
            return;
        };
        let (start, old_end, new_end) = (edit.start_byte, edit.old_end_byte, edit.new_end_byte);
        let range = kept.range.clone();
        if old_end <= range.start {
            kept.range = shift(range.start, edit)..shift(range.end, edit);
        } else if start >= range.end {
            return;
        } else if range.start <= start && old_end <= range.end {
            let at = start - range.start;
            kept.styles.splice(
                at..at + (old_end - start),
                std::iter::repeat_n(None, new_end - start),
            );
            kept.range = range.start..range.end - (old_end - start) + (new_end - start);
        } else {
            *painted = None;
            return;
        }
        for stale in &mut kept.stale {
            *stale = shift(stale.start, edit)..shift(stale.end, edit).max(shift(stale.start, edit));
        }
        let line = text.byte_to_line(start.min(text.len_bytes()));
        let from = text.line_to_byte(line);
        let end_line = text.byte_to_line(new_end.min(text.len_bytes()));
        let to = text
            .try_line_to_byte(end_line + 1)
            .unwrap_or(text.len_bytes());
        kept.stale.push(from..to.max(new_end));
    }

    /// Marks where the kept colors are no longer right.
    fn repaint(&mut self, ranges: impl IntoIterator<Item = Range<usize>>) {
        if let Some(kept) = self.painted.get_mut() {
            kept.stale.extend(ranges);
        }
    }

    pub fn forget_colors(&mut self) {
        *self.painted.get_mut() = None;
    }

    /// Stops waiting for the parse under way, as when the thread is gone.
    /// The text still counts as changed, so it is parsed again.
    pub fn forget_job(&mut self) {
        self.job = None;
        self.since_job.clear();
        forget_layer_jobs(&mut self.injections);
    }

    /// Every parse of this buffer under way: its own and its layers'.
    pub fn jobs(&self) -> Vec<u64> {
        let mut jobs: Vec<u64> = self.job.into_iter().collect();
        layer_jobs(&self.injections, &mut jobs);
        jobs
    }

    pub fn injections_pending(&self) -> bool {
        self.injections_pending
    }

    /// Drops the trees when an edit cannot be described, e.g. after undo.
    pub fn invalidate(&mut self) {
        self.forget_colors();
        self.tree = None;
        self.base = None;
        self.job = None;
        self.since_job.clear();
        self.edited = None;
        self.injections = Injections::default();
        self.dirty = true;
    }
}

/// Moves what was found and the layers' trees along with an edit.
fn edit_injections(injections: &mut Injections, edit: &InputEdit) {
    for injection in &mut injections.found {
        injection
            .ranges
            .iter_mut()
            .for_each(|r| shift_range(r, edit));
    }
    for layer in &mut injections.layers {
        if let Some(tree) = &mut layer.tree {
            tree.edit(edit);
        }
        if layer.job.is_some() {
            layer.since_job.push(*edit);
        }
        add_edit(&mut layer.edited, edit);
        // Touching counts: typing at the end of a range may extend it.
        layer.touched |= layer
            .ranges
            .iter()
            .any(|r| edit.start_byte <= r.end_byte && r.start_byte <= edit.old_end_byte);
        layer.ranges.iter_mut().for_each(|r| shift_range(r, edit));
        edit_injections(&mut layer.injections, edit);
    }
}

/// Adds `edit` to `edited`, the span of the text changed so far, moved
/// along with it.
fn add_edit(edited: &mut Option<Range<usize>>, edit: &InputEdit) {
    let (start, new_end) = (edit.start_byte, edit.new_end_byte);
    *edited = Some(match edited.take() {
        Some(span) => shift(span.start, edit).min(start)..shift(span.end, edit).max(new_end),
        None => start..new_end,
    });
}

/// Where `byte` is after `edit`. Bytes inside the edited text move to its
/// start.
fn shift(byte: usize, edit: &InputEdit) -> usize {
    if byte >= edit.old_end_byte {
        byte - edit.old_end_byte + edit.new_end_byte
    } else {
        byte.min(edit.start_byte)
    }
}

fn shift_range(range: &mut TsRange, edit: &InputEdit) {
    for (byte, point) in [
        (&mut range.start_byte, &mut range.start_point),
        (&mut range.end_byte, &mut range.end_point),
    ] {
        if *byte >= edit.old_end_byte {
            if point.row == edit.old_end_position.row {
                point.column =
                    point.column - edit.old_end_position.column + edit.new_end_position.column;
            }
            point.row = point.row - edit.old_end_position.row + edit.new_end_position.row;
        } else if *byte > edit.start_byte {
            *point = edit.start_position;
        }
        *byte = shift(*byte, edit);
    }
}

fn point(text: &Rope, byte: usize) -> Point {
    let row = text.byte_to_line(byte);
    Point {
        row,
        column: byte - text.line_to_byte(row),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Color;

    fn style(n: u8) -> Style {
        Style {
            fg: Color::Indexed(n),
            ..Style::default()
        }
    }

    #[test]
    fn inner_spans_win_and_the_first_pattern_wins_ties() {
        let mut styles = vec![None; 10];
        paint(
            &mut styles,
            100,
            vec![
                (102..108, 0, style(1)),
                (104..105, 3, style(2)),
                (100..101, 2, style(3)),
                (100..101, 1, style(4)),
            ],
            None,
        );
        let expected = [4, 0, 1, 1, 2, 1, 1, 1, 0, 0];
        for (i, n) in expected.into_iter().enumerate() {
            assert_eq!(styles[i], (n > 0).then(|| style(n)), "byte {i}");
        }
    }
}
