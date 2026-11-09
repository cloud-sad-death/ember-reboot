//! Ember core: capture, slicing, storage, search and export. No UI.
pub mod ansi;
pub mod export;
pub mod host;
pub mod index;
pub mod loopdetect;
pub mod osc;
pub mod paths;
pub mod pty;
pub mod recorder;
pub mod search;
pub mod settings;
pub mod shell;
pub mod slicer;
pub mod snippets;
pub mod store;
pub mod stream;
pub mod themes;

pub use recorder::{Mode, Recorder};
pub use store::{Row, RowKind, SessionMeta, SessionReader, SessionWriter};
