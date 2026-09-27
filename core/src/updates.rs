//! Updating installed plugins from the core menu. Where a plugin came from
//! and how to fetch it are the frontend's business, as the core knows no
//! network or where plugins are kept; the frontend hands the core these.

use std::fmt;

/// How the core menu updates plugins.
pub trait PluginUpdates: Send + Sync {
    /// Whether `name` follows releases, so `check` can find newer ones.
    fn can_update(&self, name: &str) -> bool;

    /// Looks for a newer release of `name` and gets it ready: `None` when
    /// it is up to date. Downloads, so the core calls it on a thread of its
    /// own.
    fn check(&self, name: &str) -> Result<Option<Box<dyn PendingUpdate>>, String>;
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

/// What a check found, as it comes back from its thread.
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
