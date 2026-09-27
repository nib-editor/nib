//! Installing, updating, and removing plugins from the core menu. Where a
//! plugin comes from and where it is kept are the frontend's business, as
//! the core knows no network or where plugins are kept; the frontend hands
//! the core these.

use std::fmt;
use std::path::PathBuf;

/// How the core menu installs, updates, and removes plugins.
pub trait PluginStore: Send + Sync {
    /// Whether `name` follows releases, so `check` can find newer ones.
    fn can_update(&self, name: &str) -> bool;

    /// Looks for a newer release of `name` and gets it ready: `None` when
    /// it is up to date. Downloads, so the core calls it on a thread of its
    /// own.
    fn check(&self, name: &str) -> Result<Option<Box<dyn PendingUpdate>>, String>;

    /// Whether `name` was installed, so it can be removed.
    fn can_remove(&self, name: &str) -> bool;

    /// Removes `name`'s files, keeping its settings and data.
    fn remove(&self, name: &str) -> Result<(), String>;

    /// Fetches and checks the plugin `source` names, as `nib plugin add`
    /// takes it, for the user to agree to. Downloads, so the core calls it
    /// on a thread of its own.
    fn prepare_install(&self, source: &str) -> Result<Box<dyn PendingInstall>, String>;
}

/// A newer release, fetched, not yet in place of the one loaded.
pub trait PendingUpdate: Send {
    fn version(&self) -> &str;

    /// The capabilities it asks for that were not agreed to. The core asks
    /// before it goes in if there are any.
    fn added_capabilities(&self) -> &[String];

    /// Puts it in place of the old version, for the core to load again.
    fn apply(self: Box<Self>) -> Result<(), String>;
}

/// A plugin fetched for installing, not yet in place.
pub trait PendingInstall: Send {
    fn name(&self) -> &str;
    fn version(&self) -> &str;
    /// Where it comes from, as the user will know it.
    fn source(&self) -> &str;
    fn capabilities(&self) -> &[String];

    /// Puts it in place. Returns its directory, for the core to load.
    fn apply(self: Box<Self>) -> Result<PathBuf, String>;
}

/// What a check for a newer release found, as it comes back from its
/// thread.
pub(crate) struct Checked {
    pub plugin: usize,
    pub name: String,
    pub result: Result<Option<Box<dyn PendingUpdate>>, String>,
}

impl fmt::Debug for Checked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let result = match &self.result {
            Ok(Some(pending)) => format!("ready: {}", pending.version()),
            Ok(None) => "up to date".into(),
            Err(err) => err.clone(),
        };
        write!(f, "{} {}", self.name, result)
    }
}

/// A plugin fetched for installing, as it comes back from its thread.
pub(crate) struct Prepared {
    pub source: String,
    pub result: Result<Box<dyn PendingInstall>, String>,
}

impl fmt::Debug for Prepared {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.result {
            Ok(pending) => write!(
                f,
                "{}: {} {}",
                self.source,
                pending.name(),
                pending.version()
            ),
            Err(err) => write!(f, "{}: {err}", self.source),
        }
    }
}
