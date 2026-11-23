//! Pure domain types, configuration, detectors, and backend traits for PasteFlow.
//! Contains no GTK or X11 dependencies.

pub mod backend;
pub mod config;
pub mod model;
pub mod redact;

pub use backend::{ClipboardBackend, ClipboardEvent};
pub use config::{AppPaths, Config};
pub use model::{
    ClipFilter, ClipKind, ClipRecord, ImageMeta, RawClip, RawFormat, SelectionKind, SpaceRecord,
};
pub use redact::Redacted;
