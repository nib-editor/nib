//! Core of the nib editor: buffers, selections, rendering, and the plugin host.

mod background;
pub mod buffer;
pub mod change;
mod clipboard;
pub mod config;
pub mod editor;
mod error;
mod events;
mod files;
pub mod grapheme;
pub mod grid;
mod history;
pub mod input;
mod layout;
pub mod marks;
mod plugin;
mod process;
mod render;
pub mod search;
pub mod selection;
mod syntax;
pub mod ui;
pub mod updates;
pub mod view;
mod windows;

pub use background::Waker;
pub use buffer::{Buffer, Change, LineEnding};
pub use change::{Assoc, ChangeSet, Edit};
pub use clipboard::Clipboard;
pub use config::{
    CONFIG_TEMPLATE, Config, Indent, Load, PluginConfig, Settings, Timeout, plugin_template,
};
pub use editor::{Editor, Menu, ScrollAmount};
pub use error::Error;
pub use events::TextChange;
pub use grid::{Cell, Color, Cursor, CursorShape, Grid, Style, Symbol};
pub use history::UndoMode;
pub use input::{KeyCode, KeyEvent, Modifiers, parse_keys};
pub use plugin::{
    API_VERSION, Interrupter, PluginInfo, PluginManifest, PluginOptions, PluginSource, plugin_name,
    read_manifest,
};
pub use process::Stream;
pub use selection::{Range, Selection};
pub use ui::{Side, Span, StyledLine, Theme};
pub use updates::{PendingInstall, PendingUpdate, PluginStore};
pub use view::View;
pub use windows::Rect;
