//! Plugins from elsewhere (docs/plugin-install.md): packing them into
//! `.nib.tar.gz` archives, and adding, updating, and removing them. No
//! plugin code runs here.

use std::fs::{self, File};
use std::io::{self, BufRead, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use nib_core::{API_VERSION, PluginManifest, read_manifest};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const SUFFIX: &str = ".nib.tar.gz";
/// The list of plugins to find by name: a TOML file in a git repository,
/// taking additions by pull request.
pub const INDEX: &str = "https://raw.githubusercontent.com/nib-editor/plugins/main/plugins.toml";
/// The most an archive may unpack to.
const MAX_SIZE: u64 = 100 << 20;

/// Where installed plugins live: `<data>/installed/<name>/`, and the
/// record of them, `<data>/installed.toml`.
#[derive(Clone)]
pub struct Store {
    pub data: PathBuf,
}

/// An installed plugin, as `installed.toml` keeps it.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Record {
    pub name: String,
    /// As it was given to `add`.
    pub source: String,
    /// The archive fetched.
    pub url: String,
    /// The release's tag, for GitHub.
    pub tag: Option<String>,
    pub version: String,
    pub sha256: String,
    /// The capabilities agreed to.
    pub capabilities: Vec<String>,
}

#[derive(Default, Deserialize, Serialize)]
struct Records {
    #[serde(default, rename = "plugin")]
    plugins: Vec<Record>,
}

impl Store {
    pub fn dir(&self, name: &str) -> PathBuf {
        self.data.join("installed").join(name)
    }

    fn record_file(&self) -> PathBuf {
        self.data.join("installed.toml")
    }

    pub fn records(&self) -> Result<Vec<Record>, String> {
        let file = self.record_file();
        match fs::read_to_string(&file) {
            Ok(text) => toml::from_str::<Records>(&text)
                .map(|r| r.plugins)
                .map_err(|err| format!("{}: {err}", file.display())),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(err) => Err(format!("{}: {err}", file.display())),
        }
    }

    fn save(&self, plugins: Vec<Record>) -> Result<(), String> {
        let text = toml::to_string(&Records { plugins }).map_err(|err| err.to_string())?;
        let file = self.record_file();
        fs::create_dir_all(&self.data).map_err(|err| format!("{}: {err}", self.data.display()))?;
        fs::write(&file, text).map_err(|err| format!("{}: {err}", file.display()))
    }
}

/// `nib plugin pack <dir>`: writes `<name>-<version>.nib.tar.gz` into `out`.
/// Packs what nib reads of the plugin in `dir`: its manifest, its code, the
/// files of its languages, the example of its settings, and its licenses.
/// Not sources or build output, which a plugin's directory often holds too.
pub fn pack(dir: &Path, out: &Path) -> Result<PathBuf, String> {
    let manifest = read_manifest(dir).map_err(|err| err.to_string())?;
    let file = out.join(format!("{}-{}{SUFFIX}", manifest.name, manifest.version));
    let fail = |err: io::Error| format!("{}: {err}", file.display());
    let mut names = vec!["plugin.toml".to_string()];
    if manifest.has_code {
        names.push("plugin.wasm".into());
    }
    if dir.join(nib_core::SETTINGS_EXAMPLE).is_file() {
        names.push(nib_core::SETTINGS_EXAMPLE.into());
    }
    names.extend(manifest.language_files);
    let licenses = fs::read_dir(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    names.extend(licenses.filter_map(|entry| {
        let name = entry.ok()?.file_name().into_string().ok()?;
        name.starts_with("LICENSE").then_some(name)
    }));
    names.sort();
    names.dedup();
    let encoder = GzEncoder::new(File::create(&file).map_err(fail)?, Compression::default());
    let mut archive = tar::Builder::new(encoder);
    for name in names {
        let path = dir.join(&name);
        archive
            .append_path_with_name(&path, &name)
            .map_err(|err| format!("{}: {err}", path.display()))?;
    }
    archive
        .into_inner()
        .and_then(|encoder| encoder.finish())
        .map_err(fail)?;
    Ok(file)
}

/// Unpacks `archive` into `dest`. Only plain files and directories inside
/// `dest` are allowed, and no more than `MAX_SIZE` in all.
pub fn unpack(archive: &Path, dest: &Path) -> Result<(), String> {
    let fail = |err: io::Error| format!("{}: {err}", archive.display());
    let file = File::open(archive).map_err(fail)?;
    let mut tar = tar::Archive::new(GzDecoder::new(file));
    let mut total = 0;
    for entry in tar.entries().map_err(fail)? {
        let mut entry = entry.map_err(fail)?;
        let path = entry.path().map_err(fail)?.into_owned();
        let inside = path
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
        if !inside {
            return Err(format!("{} is outside the plugin", path.display()));
        }
        match entry.header().entry_type() {
            tar::EntryType::Regular | tar::EntryType::Directory => {}
            _ => return Err(format!("{}: only files and directories", path.display())),
        }
        total += entry.header().size().map_err(fail)?;
        if total > MAX_SIZE {
            return Err(format!("it unpacks to more than {} MiB", MAX_SIZE >> 20));
        }
        entry.unpack_in(dest).map_err(fail)?;
    }
    Ok(())
}

/// Where a plugin comes from, as given to `add`.
#[derive(Debug, PartialEq, Eq)]
pub enum Source {
    GitHub {
        owner: String,
        repo: String,
        /// A release's tag; otherwise the latest release.
        tag: Option<String>,
    },
    Url(String),
    /// An archive on this machine, as when trying one's own build.
    File(PathBuf),
}

impl Source {
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.ends_with(SUFFIX) {
            if text.starts_with("https://") {
                return Ok(Source::Url(text.into()));
            }
            if text.contains("://") {
                return Err(format!("{text}: only https URLs"));
            }
            return Ok(Source::File(text.into()));
        }
        let rest = text.strip_prefix("https://").unwrap_or(text);
        let rest = rest.strip_prefix("github.com/").unwrap_or(rest);
        let (path, tag) = match rest.split_once('@') {
            Some((path, tag)) => (path, Some(tag.to_string())),
            None => (rest, None),
        };
        let usage = || {
            format!("{text}: expected owner/repo, github.com/owner/repo, or a URL of a *{SUFFIX}")
        };
        let (owner, repo) = path
            .trim_end_matches('/')
            .split_once('/')
            .ok_or_else(usage)?;
        if owner.is_empty() || repo.is_empty() || repo.contains('/') {
            return Err(usage());
        }
        Ok(Source::GitHub {
            owner: owner.into(),
            repo: repo.into(),
            tag,
        })
    }

    /// Whether `update` follows it: a GitHub repository with no tag given.
    fn follows_releases(&self) -> bool {
        matches!(self, Source::GitHub { tag: None, .. })
    }
}

/// An archive ready to install, and where it came from.
struct Fetched {
    archive: PathBuf,
    url: String,
    tag: Option<String>,
}

/// Downloads what `source` points at into `work`.
fn fetch(source: &Source, work: &Path) -> Result<Fetched, String> {
    let (url, tag) = match source {
        Source::File(path) => {
            return Ok(Fetched {
                archive: path.clone(),
                url: path.display().to_string(),
                tag: None,
            });
        }
        Source::Url(url) => (url.clone(), None),
        Source::GitHub { owner, repo, tag } => {
            let release = match tag {
                Some(tag) => format!("tags/{tag}"),
                None => "latest".into(),
            };
            let api = format!("https://api.github.com/repos/{owner}/{repo}/releases/{release}");
            let response = work.join("release.json");
            download(&api, &response)?;
            let text = fs::read_to_string(&response).map_err(|err| err.to_string())?;
            let release: Value =
                serde_json::from_str(&text).map_err(|err| format!("{api}: {err}"))?;
            let (url, tag) =
                release_asset(&release).map_err(|err| format!("{owner}/{repo}: {err}"))?;
            (url, Some(tag))
        }
    };
    let archive = work.join(format!("plugin{SUFFIX}"));
    download(&url, &archive)?;
    Ok(Fetched { archive, url, tag })
}

/// The URL of the release's one plugin archive, and the release's tag.
fn release_asset(release: &Value) -> Result<(String, String), String> {
    let tag = release["tag_name"].as_str().unwrap_or_default().to_string();
    let archives: Vec<&str> = release["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|asset| asset["name"].as_str().is_some_and(|n| n.ends_with(SUFFIX)))
        .filter_map(|asset| asset["browser_download_url"].as_str())
        .collect();
    match archives[..] {
        [url] => Ok((url.to_string(), tag)),
        [] => Err(format!("release {tag} has no *{SUFFIX} file")),
        _ => Err(format!("release {tag} has more than one *{SUFFIX} file")),
    }
}

fn curl(url: &str) -> Command {
    let mut command = Command::new("curl");
    command
        .args(["--fail", "--silent", "--show-error", "--location"])
        .args(["--proto", "=https"])
        .arg(url);
    command
}

fn download(url: &str, out: &Path) -> Result<(), String> {
    let status = curl(url)
        .arg("--output")
        .arg(out)
        .status()
        .map_err(|err| format!("running curl failed: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("downloading {url} failed"))
    }
}

fn download_text(url: &str) -> Result<String, String> {
    let output = curl(url)
        .stderr(Stdio::inherit())
        .output()
        .map_err(|err| format!("running curl failed: {err}"))?;
    if !output.status.success() {
        return Err(format!("downloading {url} failed"));
    }
    String::from_utf8(output.stdout).map_err(|err| format!("{url}: {err}"))
}

/// A plugin in the index.
#[derive(Debug, Deserialize, PartialEq, Eq)]
pub struct Listing {
    pub name: String,
    /// Where to get it, as `add` takes it.
    pub source: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Deserialize)]
struct Index {
    #[serde(default, rename = "plugin")]
    plugins: Vec<Listing>,
}

/// Downloads the index.
pub fn index() -> Result<Vec<Listing>, String> {
    parse_index(&download_text(INDEX)?).map_err(|err| format!("{INDEX}: {err}"))
}

fn parse_index(text: &str) -> Result<Vec<Listing>, String> {
    toml::from_str::<Index>(text)
        .map(|index| index.plugins)
        .map_err(|err| err.to_string())
}

/// The listings with `word` in their name or description, ignoring case.
pub fn search<'a>(listings: &'a [Listing], word: &str) -> Vec<&'a Listing> {
    let word = word.to_lowercase();
    listings
        .iter()
        .filter(|l| {
            l.name.to_lowercase().contains(&word) || l.description.to_lowercase().contains(&word)
        })
        .collect()
}

/// Whether `add` takes `text` as a name to look up in the index, rather
/// than as where to get the plugin.
pub fn is_name(text: &str) -> bool {
    !text.is_empty() && !text.contains(['/', '\\', ':', '@']) && !text.ends_with(SUFFIX)
}

/// A name in the index, and a release's tag if one follows `@`:
/// `wordcount@v0.2.0`.
pub fn name_and_tag(text: &str) -> Option<(&str, Option<&str>)> {
    let (name, tag) = match text.split_once('@') {
        Some((name, tag)) => (name, Some(tag)),
        None => (text, None),
    };
    let tag_ok = tag.is_none_or(|t| !t.is_empty() && !t.contains(['/', '\\', '@']));
    (is_name(name) && tag_ok).then_some((name, tag))
}

/// The index's `source` for a plugin, at release `tag`: only a GitHub
/// repository has tags to choose, and only one the index does not pin.
pub fn at_tag(source: &str, tag: &str) -> Result<String, String> {
    match Source::parse(source)? {
        Source::GitHub {
            owner,
            repo,
            tag: None,
        } => Ok(format!("{owner}/{repo}@{tag}")),
        Source::GitHub {
            tag: Some(pinned), ..
        } => Err(format!(
            "the index pins {source} to {pinned}, so no other tag can be chosen"
        )),
        Source::Url(_) | Source::File(_) => Err(format!(
            "{source} is an archive, with no releases to choose a tag from"
        )),
    }
}

fn sha256(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let bytes = fs::read(path).map_err(|err| format!("{}: {err}", path.display()))?;
    Ok(Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// Asks the user a yes-or-no question on the terminal. No answer, as when
/// input is not a terminal, is no.
pub fn ask(question: &str) -> bool {
    print!("{question} [y/N] ");
    let _ = io::stdout().flush();
    let mut answer = String::new();
    let _ = io::stdin().lock().read_line(&mut answer);
    answer.trim().eq_ignore_ascii_case("y")
}

/// What a plugin is and may do, for the user to agree to.
fn describe(manifest: &PluginManifest, source: &str) -> String {
    let mut text = format!("{} {} from {source}\n", manifest.name, manifest.version);
    let list = |items: &[String]| {
        if items.is_empty() {
            "none".to_string()
        } else {
            items.join(", ")
        }
    };
    text += &format!("  capabilities: {}\n", list(&manifest.capabilities));
    text += &format!("  events: {}\n", list(&manifest.events));
    if !manifest.languages.is_empty() {
        text += &format!("  languages: {}\n", manifest.languages.join(", "));
    }
    text
}

/// A directory to work in under the store, removed when dropped.
struct Work(PathBuf);

impl Work {
    fn new(store: &Store) -> Result<Self, String> {
        let dir = store.data.join("installed").join(format!(
            ".work-{}-{}",
            std::process::id(),
            next_work()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("plugin"))
            .map_err(|err| format!("{}: {err}", dir.display()))?;
        Ok(Self(dir))
    }

    fn plugin(&self) -> PathBuf {
        self.0.join("plugin")
    }
}

/// Tells apart the work of checks that run at once, as from the editor.
fn next_work() -> usize {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

impl Drop for Work {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Downloads, unpacks, and checks the plugin at `source`.
fn prepare(source: &Source, work: &Work) -> Result<(Fetched, PluginManifest), String> {
    let fetched = fetch(source, &work.0)?;
    unpack(&fetched.archive, &work.plugin())?;
    let manifest = read_manifest(&work.plugin()).map_err(|err| err.to_string())?;
    if manifest.api != API_VERSION {
        return Err(format!(
            "{} is for nib's plugin API {}, but this nib has {API_VERSION}",
            manifest.name, manifest.api
        ));
    }
    Ok((fetched, manifest))
}

/// Puts the unpacked plugin in its place, replacing an older one.
fn put_in_place(store: &Store, work: &Work, name: &str) -> Result<(), String> {
    let dir = store.dir(name);
    if dir.exists() {
        fs::remove_dir_all(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    }
    fs::rename(work.plugin(), &dir).map_err(|err| format!("{}: {err}", dir.display()))
}

/// `nib plugin add`. `builtin` names the plugins built into nib, which an
/// installed one may not share a name with. `listed` is the name the index
/// gave `text` under, which the plugin must have. Returns the plugin's
/// name, or `None` when the user said no.
pub fn add(
    store: &Store,
    text: &str,
    listed: Option<&str>,
    builtin: &[&str],
    confirm: &mut dyn FnMut(&str) -> bool,
) -> Result<Option<String>, String> {
    let pending = prepare_add(store, text, listed, builtin)?;
    if !confirm(&pending.question()) {
        return Ok(None);
    }
    pending.apply(store).map(Some)
}

/// What `add` takes, as a source: a name in the index, with `@tag` or not,
/// is looked up; anything else is a source already. Returns the source and
/// the name it was listed under.
pub fn resolve(text: &str) -> Result<(String, Option<String>), String> {
    let Some((name, tag)) = name_and_tag(text) else {
        return Ok((text.to_string(), None));
    };
    let listing = index()?
        .into_iter()
        .find(|l| l.name == name)
        .ok_or_else(|| format!("no plugin named {name} in {INDEX}"))?;
    let source = match tag {
        Some(tag) => at_tag(&listing.source, tag)?,
        None => listing.source,
    };
    Ok((source, Some(name.to_string())))
}

/// A plugin fetched and checked for installing, not yet in place.
pub struct PendingAdd {
    work: Work,
    source: String,
    fetched: Fetched,
    manifest: PluginManifest,
}

/// Fetches the plugin at `source` and checks it can go in: the name it was
/// listed under, if any, not a built-in one, not installed from elsewhere.
/// Downloads, so the editor runs it on a thread.
pub fn prepare_add(
    store: &Store,
    source: &str,
    listed: Option<&str>,
    builtin: &[&str],
) -> Result<PendingAdd, String> {
    let parsed = Source::parse(source)?;
    let work = Work::new(store)?;
    let (fetched, manifest) = prepare(&parsed, &work)?;
    let name = &manifest.name;
    if let Some(listed) = listed.filter(|listed| listed != name) {
        return Err(format!(
            "{source} is listed as {listed}, but the plugin there is {name}"
        ));
    }
    if builtin.contains(&name.as_str()) {
        return Err(format!("{name} is the name of a plugin built into nib"));
    }
    if let Some(other) = store
        .records()?
        .iter()
        .find(|r| &r.name == name && r.source != source)
    {
        return Err(format!(
            "{name} is installed already, from {}; remove it first",
            other.source
        ));
    }
    Ok(PendingAdd {
        work,
        source: source.to_string(),
        fetched,
        manifest,
    })
}

impl PendingAdd {
    pub fn question(&self) -> String {
        format!("{}Install it?", describe(&self.manifest, &self.source))
    }

    /// Puts it in the store and records it. Returns its name.
    pub fn apply(self, store: &Store) -> Result<String, String> {
        let name = self.manifest.name.clone();
        let mut records = store.records()?;
        put_in_place(store, &self.work, &name)?;
        records.retain(|r| r.name != name);
        records.push(Record {
            name: name.clone(),
            source: self.source,
            sha256: sha256(&self.fetched.archive)?,
            url: self.fetched.url,
            tag: self.fetched.tag,
            version: self.manifest.version,
            capabilities: self.manifest.capabilities,
        });
        records.sort_by(|a, b| a.name.cmp(&b.name));
        store.save(records)?;
        Ok(name)
    }
}

/// What `update` did to one plugin.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Updated {
        from: String,
        to: String,
    },
    UpToDate,
    /// Installed from a tag or an archive, so it stays as it is.
    Pinned,
    /// It asks for more capabilities, and the user said no.
    Declined,
}

/// `nib plugin update` for one installed plugin. The user is asked again
/// only when the new version wants capabilities not agreed to before.
pub fn update(
    store: &Store,
    name: &str,
    confirm: &mut dyn FnMut(&str) -> bool,
) -> Result<Outcome, String> {
    let (record, source) = installed(store, name)?;
    if !source.follows_releases() {
        return Ok(Outcome::Pinned);
    }
    update_from(store, record, &source, confirm)
}

fn update_from(
    store: &Store,
    record: Record,
    source: &Source,
    confirm: &mut dyn FnMut(&str) -> bool,
) -> Result<Outcome, String> {
    match check_from(store, record, source)? {
        Check::Pinned => Ok(Outcome::Pinned),
        Check::UpToDate => Ok(Outcome::UpToDate),
        Check::Ready(pending) => {
            if let Some(question) = pending.question()
                && !confirm(&question)
            {
                return Ok(Outcome::Declined);
            }
            pending.apply(store)
        }
    }
}

/// The record of `name` and where it came from.
fn installed(store: &Store, name: &str) -> Result<(Record, Source), String> {
    let record = store
        .records()?
        .into_iter()
        .find(|r| r.name == name)
        .ok_or_else(|| format!("{name} is not installed"))?;
    let source = Source::parse(&record.source)?;
    Ok((record, source))
}

/// What a check for a newer release found.
pub enum Check {
    UpToDate,
    /// Installed from a tag or an archive, so it stays as it is.
    Pinned,
    Ready(Box<Pending>),
}

/// A newer release of an installed plugin, fetched and unpacked, not yet
/// in place of the old one.
pub struct Pending {
    work: Work,
    record: Record,
    fetched: Fetched,
    manifest: PluginManifest,
    sha256: String,
    /// Capabilities it asks for that were not agreed to.
    pub added: Vec<String>,
}

/// Whether `name` follows releases, so `check` can find newer ones.
pub fn follows_releases(store: &Store, name: &str) -> bool {
    installed(store, name).is_ok_and(|(_, source)| source.follows_releases())
}

/// Looks for a newer release of `name` and gets it ready. Downloads, so
/// the editor runs it on a thread.
pub fn check(store: &Store, name: &str) -> Result<Check, String> {
    let (record, source) = installed(store, name)?;
    if !source.follows_releases() {
        return Ok(Check::Pinned);
    }
    check_from(store, record, &source)
}

fn check_from(store: &Store, record: Record, source: &Source) -> Result<Check, String> {
    let name = record.name.as_str();
    let work = Work::new(store)?;
    let (fetched, manifest) = prepare(source, &work)?;
    if manifest.name != name {
        return Err(format!(
            "{} now holds a plugin named {}",
            record.source, manifest.name
        ));
    }
    let sha256 = sha256(&fetched.archive)?;
    if sha256 == record.sha256 {
        return Ok(Check::UpToDate);
    }
    let added = manifest
        .capabilities
        .iter()
        .filter(|c| !record.capabilities.contains(c))
        .cloned()
        .collect();
    Ok(Check::Ready(Box::new(Pending {
        work,
        record,
        fetched,
        manifest,
        sha256,
        added,
    })))
}

impl Pending {
    pub fn version(&self) -> &str {
        &self.manifest.version
    }

    /// What to ask before it goes in, if it wants more capabilities.
    pub fn question(&self) -> Option<String> {
        (!self.added.is_empty()).then(|| {
            format!(
                "{}It now also wants: {}. Update it?",
                describe(&self.manifest, &self.record.source),
                self.added.join(", ")
            )
        })
    }

    /// Puts it in place of the old version and records it.
    pub fn apply(self, store: &Store) -> Result<Outcome, String> {
        let name = self.record.name.as_str();
        let mut records = store.records()?;
        put_in_place(store, &self.work, name)?;
        for r in &mut records {
            if r.name == name {
                r.url = self.fetched.url.clone();
                r.tag = self.fetched.tag.clone();
                r.version = self.manifest.version.clone();
                r.sha256 = self.sha256.clone();
                r.capabilities = self.manifest.capabilities.clone();
            }
        }
        store.save(records)?;
        Ok(Outcome::Updated {
            from: self.record.version,
            to: self.manifest.version,
        })
    }
}

/// `nib plugin remove`: the installed files and the record go; settings and
/// data the plugin kept stay.
/// The store as the core menu works with it: installing, and updating and
/// removing the plugins nib loaded from the store, since one loaded from a
/// `path` would not change when the store's copy does.
pub struct StorePlugins {
    pub store: Store,
    /// Installed plugins nib loaded from the store, including those
    /// installed from the menu since.
    pub loaded: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    /// Names no installed plugin may take.
    pub builtin: Vec<&'static str>,
}

impl StorePlugins {
    fn loaded(&self, name: &str) -> bool {
        self.loaded
            .lock()
            .expect("loaded lock")
            .iter()
            .any(|n| n == name)
    }
}

struct StorePending {
    store: Store,
    pending: Pending,
}

struct StoreInstall {
    store: Store,
    pending: PendingAdd,
    loaded: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl nib_core::PluginStore for StorePlugins {
    fn can_update(&self, name: &str) -> bool {
        self.loaded(name) && follows_releases(&self.store, name)
    }

    fn check(&self, name: &str) -> Result<Option<Box<dyn nib_core::PendingUpdate>>, String> {
        Ok(match check(&self.store, name)? {
            Check::Ready(pending) => Some(Box::new(StorePending {
                store: self.store.clone(),
                pending: *pending,
            })),
            Check::UpToDate | Check::Pinned => None,
        })
    }

    fn can_remove(&self, name: &str) -> bool {
        self.loaded(name) && installed(&self.store, name).is_ok()
    }

    fn remove(&self, name: &str) -> Result<(), String> {
        remove(&self.store, name)?;
        self.loaded
            .lock()
            .expect("loaded lock")
            .retain(|n| n != name);
        Ok(())
    }

    fn prepare_install(&self, text: &str) -> Result<Box<dyn nib_core::PendingInstall>, String> {
        let (source, listed) = resolve(text)?;
        let pending = prepare_add(&self.store, &source, listed.as_deref(), &self.builtin)?;
        if self.loaded(&pending.manifest.name) {
            return Err(format!("{} is loaded already", pending.manifest.name));
        }
        Ok(Box::new(StoreInstall {
            store: self.store.clone(),
            pending,
            loaded: self.loaded.clone(),
        }))
    }
}

impl nib_core::PendingUpdate for StorePending {
    fn version(&self) -> &str {
        self.pending.version()
    }

    fn added_capabilities(&self) -> &[String] {
        &self.pending.added
    }

    fn apply(self: Box<Self>) -> Result<(), String> {
        self.pending.apply(&self.store).map(|_| ())
    }
}

impl nib_core::PendingInstall for StoreInstall {
    fn name(&self) -> &str {
        &self.pending.manifest.name
    }

    fn version(&self) -> &str {
        &self.pending.manifest.version
    }

    fn source(&self) -> &str {
        &self.pending.source
    }

    fn capabilities(&self) -> &[String] {
        &self.pending.manifest.capabilities
    }

    fn apply(self: Box<Self>) -> Result<std::path::PathBuf, String> {
        let name = self.pending.apply(&self.store)?;
        self.loaded.lock().expect("loaded lock").push(name.clone());
        Ok(self.store.dir(&name))
    }
}

pub fn remove(store: &Store, name: &str) -> Result<(), String> {
    let mut records = store.records()?;
    if !records.iter().any(|r| r.name == name) {
        return Err(format!("{name} is not installed"));
    }
    let dir = store.dir(name);
    if dir.exists() {
        fs::remove_dir_all(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    }
    records.retain(|r| r.name != name);
    store.save(records)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Temp(PathBuf);

    impl Temp {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("nib-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A plugin directory named `name` asking for `capabilities`.
    fn plugin(root: &Path, name: &str, version: &str, capabilities: &str, api: &str) -> PathBuf {
        let dir = root.join(format!("src-{name}-{version}"));
        fs::create_dir_all(dir.join("queries")).unwrap();
        fs::write(
            dir.join("plugin.toml"),
            format!(
                "name = \"{name}\"\nversion = \"{version}\"\napi = \"{api}\"\ncapabilities = [{capabilities}]\n\
                 [[languages]]\nname = \"x\"\nfile-types = [\"x\"]\ngrammar = \"x.wasm\"\n\
                 [languages.queries]\nhighlights = \"queries/a.scm\"\n"
            ),
        )
        .unwrap();
        fs::write(dir.join("plugin.wasm"), b"\0asm").unwrap();
        fs::write(dir.join("x.wasm"), b"\0asm").unwrap();
        fs::write(dir.join("queries/a.scm"), "(x)").unwrap();
        fs::write(dir.join("LICENSE-MIT"), "MIT").unwrap();
        // Sources and build output stay out of the archive.
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/lib.rs"), "").unwrap();
        fs::write(dir.join("Cargo.toml"), "").unwrap();
        dir
    }

    fn packed(root: &Path, name: &str, version: &str, capabilities: &str) -> String {
        let dir = plugin(root, name, version, capabilities, API_VERSION);
        pack(&dir, root).unwrap().to_string_lossy().into_owned()
    }

    #[test]
    fn names_take_a_tag() {
        assert_eq!(name_and_tag("wordcount"), Some(("wordcount", None)));
        assert_eq!(
            name_and_tag("wordcount@v0.2.0"),
            Some(("wordcount", Some("v0.2.0")))
        );
        for not in ["owner/repo@v1", "wordcount@", "a@b@c", "x.nib.tar.gz", ""] {
            assert_eq!(name_and_tag(not), None, "{not}");
        }
        assert_eq!(
            at_tag("nib-editor/plugin-example", "v0.2.0").unwrap(),
            "nib-editor/plugin-example@v0.2.0"
        );
        assert!(at_tag("a/b@v1", "v2").unwrap_err().contains("pins"));
        assert!(
            at_tag("https://x.test/a.nib.tar.gz", "v2")
                .unwrap_err()
                .contains("archive")
        );
    }

    #[test]
    fn packs_and_unpacks() {
        let temp = Temp::new("pack");
        let archive = packed(&temp.0, "foo", "0.1.0", "");
        assert!(archive.ends_with("foo-0.1.0.nib.tar.gz"));
        let dest = temp.0.join("out");
        fs::create_dir_all(&dest).unwrap();
        unpack(Path::new(&archive), &dest).unwrap();
        assert_eq!(
            fs::read_to_string(dest.join("queries/a.scm")).unwrap(),
            "(x)"
        );
        assert!(dest.join("x.wasm").is_file() && dest.join("LICENSE-MIT").is_file());
        assert!(!dest.join("src").exists() && !dest.join("Cargo.toml").exists());
        assert_eq!(read_manifest(&dest).unwrap().name, "foo");
    }

    /// An archive with one entry, written as is.
    fn raw_archive(path: &Path, name: &[u8], kind: tar::EntryType) {
        let mut header = tar::Header::new_gnu();
        header.as_gnu_mut().unwrap().name[..name.len()].copy_from_slice(name);
        header.set_entry_type(kind);
        header.set_size(1);
        if kind == tar::EntryType::Symlink {
            header.set_size(0);
            header.set_link_name("/etc/passwd").unwrap();
        }
        header.set_cksum();
        let encoder = GzEncoder::new(File::create(path).unwrap(), Compression::default());
        let mut archive = tar::Builder::new(encoder);
        let data: &[u8] = if kind == tar::EntryType::Symlink {
            b""
        } else {
            b"x"
        };
        archive.append(&header, data).unwrap();
        archive.into_inner().unwrap().finish().unwrap();
    }

    #[test]
    fn unpacking_refuses_what_leaves_the_plugin() {
        let temp = Temp::new("unsafe");
        let dest = temp.0.join("out");
        fs::create_dir_all(&dest).unwrap();
        let archive = temp.0.join("bad.nib.tar.gz");
        raw_archive(&archive, b"../escape", tar::EntryType::Regular);
        assert!(
            unpack(&archive, &dest)
                .unwrap_err()
                .contains("outside the plugin")
        );
        raw_archive(&archive, b"link", tar::EntryType::Symlink);
        assert!(unpack(&archive, &dest).unwrap_err().contains("only files"));
        assert!(!temp.0.join("escape").exists());
    }

    #[test]
    fn sources_parse() {
        let github = |owner: &str, repo: &str, tag: Option<&str>| Source::GitHub {
            owner: owner.into(),
            repo: repo.into(),
            tag: tag.map(String::from),
        };
        assert_eq!(Source::parse("a/b"), Ok(github("a", "b", None)));
        assert_eq!(
            Source::parse("github.com/a/b@v1"),
            Ok(github("a", "b", Some("v1")))
        );
        assert_eq!(
            Source::parse("https://github.com/a/b/"),
            Ok(github("a", "b", None))
        );
        assert_eq!(
            Source::parse("https://x.org/f.nib.tar.gz"),
            Ok(Source::Url("https://x.org/f.nib.tar.gz".into()))
        );
        assert!(Source::parse("http://x.org/f.nib.tar.gz").is_err());
        assert!(Source::parse("nothing").is_err());
    }

    #[test]
    fn releases_have_one_archive() {
        let release = serde_json::json!({"tag_name": "v1", "assets": [
            {"name": "notes.txt", "browser_download_url": "https://x/notes.txt"},
            {"name": "foo-1.nib.tar.gz", "browser_download_url": "https://x/foo-1.nib.tar.gz"},
        ]});
        assert_eq!(
            release_asset(&release),
            Ok(("https://x/foo-1.nib.tar.gz".into(), "v1".into()))
        );
        let none = serde_json::json!({"tag_name": "v2", "assets": []});
        assert!(
            release_asset(&none)
                .unwrap_err()
                .contains("no *.nib.tar.gz")
        );
    }

    #[test]
    fn adds_after_asking_and_removes() {
        let temp = Temp::new("add");
        let store = Store {
            data: temp.0.join("data"),
        };
        let archive = packed(&temp.0, "foo", "0.1.0", "\"clipboard\"");
        let mut asked = String::new();
        let name = add(&store, &archive, None, &["helix"], &mut |q| {
            asked = q.to_string();
            true
        })
        .unwrap();
        assert_eq!(name.as_deref(), Some("foo"));
        assert!(asked.contains("capabilities: clipboard"), "{asked}");
        assert!(store.dir("foo").join("plugin.wasm").is_file());
        let records = store.records().unwrap();
        assert_eq!(records[0].version, "0.1.0");
        assert_eq!(records[0].capabilities, ["clipboard"]);
        assert_eq!(records[0].sha256, sha256(Path::new(&archive)).unwrap());
        // Pinned to a file, so update leaves it.
        assert_eq!(update(&store, "foo", &mut |_| true), Ok(Outcome::Pinned));

        remove(&store, "foo").unwrap();
        assert!(!store.dir("foo").exists());
        assert!(store.records().unwrap().is_empty());
    }

    #[test]
    fn updates_ask_again_only_for_new_capabilities() {
        let temp = Temp::new("update");
        let store = Store {
            data: temp.0.join("data"),
        };
        let first = packed(&temp.0, "foo", "0.1.0", "\"clipboard\"");
        add(&store, &first, None, &[], &mut |_| true).unwrap();
        let record = || store.records().unwrap()[0].clone();
        let from = |path: &str| Source::File(path.into());

        let same = from(&first);
        assert_eq!(
            update_from(&store, record(), &same, &mut |_| panic!()),
            Ok(Outcome::UpToDate)
        );
        // Fewer capabilities: no question.
        let second = packed(&temp.0, "foo", "0.2.0", "");
        let updated = update_from(&store, record(), &from(&second), &mut |_| panic!());
        assert_eq!(
            updated,
            Ok(Outcome::Updated {
                from: "0.1.0".into(),
                to: "0.2.0".into()
            })
        );
        assert!(record().capabilities.is_empty());
        // More: asked, and no keeps the old one.
        let third = packed(&temp.0, "foo", "0.3.0", "\"process\"");
        let mut asked = String::new();
        let declined = update_from(&store, record(), &from(&third), &mut |q| {
            asked = q.to_string();
            false
        });
        assert_eq!(declined, Ok(Outcome::Declined));
        assert!(asked.contains("now also wants: process"), "{asked}");
        assert_eq!(record().version, "0.2.0");
    }

    #[test]
    fn refuses_what_does_not_fit() {
        let temp = Temp::new("refuse");
        let store = Store {
            data: temp.0.join("data"),
        };
        let yes = &mut |_: &str| true;
        let helix = packed(&temp.0, "helix", "9.0.0", "");
        assert!(
            add(&store, &helix, None, &["helix"], yes)
                .unwrap_err()
                .contains("built into nib")
        );
        let dir = plugin(&temp.0, "old", "0.1.0", "", "0.1");
        let old = pack(&dir, &temp.0).unwrap();
        let err = add(&store, &old.to_string_lossy(), None, &[], yes).unwrap_err();
        assert!(err.contains("plugin API 0.1"), "{err}");
        // Saying no installs nothing.
        let foo = packed(&temp.0, "foo", "0.1.0", "");
        assert_eq!(add(&store, &foo, None, &[], &mut |_| false), Ok(None));
        assert!(!store.dir("foo").exists());
        // What the index calls it is what it has to be.
        let err = add(&store, &foo, Some("bar"), &[], yes).unwrap_err();
        assert!(err.contains("listed as bar"), "{err}");
    }

    #[test]
    fn the_index_is_searched_by_name_and_description() {
        let listings = parse_index(
            r#"
            [[plugin]]
            name = "wordcount"
            source = "someone/nib-wordcount"
            description = "Counts words in the status line"

            [[plugin]]
            name = "git"
            source = "someone/nib-git"
            "#,
        )
        .unwrap();
        let names = |word| -> Vec<&str> {
            search(&listings, word)
                .iter()
                .map(|l| l.name.as_str())
                .collect()
        };
        assert_eq!(names("STATUS"), ["wordcount"]);
        assert_eq!(names("git"), ["git"]);
        assert_eq!(names(""), ["wordcount", "git"]);
        assert!(parse_index("[[plugin]]\nname = \"x\"").is_err());
    }

    #[test]
    fn names_are_told_from_sources() {
        assert!(is_name("wordcount"));
        for source in [
            "someone/nib-foo",
            "foo.nib.tar.gz",
            "https://x/foo.nib.tar.gz",
            "foo@v1",
            "C:foo",
        ] {
            assert!(!is_name(source), "{source}");
        }
    }
}
