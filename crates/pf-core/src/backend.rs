use crate::model::{RawClip, RawFormat, SelectionKind};
use anyhow::Result;

/// Events emitted by a clipboard backend (X11 or future Wayland).
#[derive(Debug, Clone)]
pub enum ClipboardEvent {
    /// A new external clipboard payload was captured.
    Captured(RawClip),
    /// The previous selection owner vanished (`SelectionClientClose` / `WindowDestroy`).
    OwnerVanished { selection: SelectionKind },
    /// PasteFlow served a data target to an external requestor.
    SelectionServed { target: String },
}

/// Abstraction over platform clipboard engines (X11 in v1, Wayland in M10).
pub trait ClipboardBackend {
    /// Drain any pending events from the display server without blocking.
    fn poll_events(&mut self) -> Result<Vec<ClipboardEvent>>;

    /// Claim ownership of `CLIPBOARD` and serve the given formats.
    fn own_clipboard(&mut self, formats: Vec<RawFormat>) -> Result<()>;

    /// Hand off clipboard ownership to `CLIPBOARD_MANAGER` (`SAVE_TARGETS`) if currently owned.
    fn handoff_on_exit(&mut self) -> Result<()>;
}
