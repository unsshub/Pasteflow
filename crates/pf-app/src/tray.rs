use anyhow::Result;
use ksni::blocking::TrayMethods;
use ksni::menu::{CheckmarkItem, MenuItem, StandardItem};

/// Commands sent from the `ksni` tray thread to the GTK main loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    ShowMain,
    ToggleMain,
    ToggleOverlay,
    OpenShortcuts,
    TogglePause,
    ToggleUniversalPaste,
    PasteNext,
    ClearQueue,
    ToggleAutostart,
    ClearUnpinned,
    Quit,
}

pub struct PasteFlowTray {
    tx: async_channel::Sender<TrayCommand>,
    pub paused: bool,
    pub autostart: bool,
    pub universal_paste: bool,
    pub queue_count: usize,
}

impl ksni::Tray for PasteFlowTray {
    fn id(&self) -> String {
        "app.pasteflow.PasteFlow".to_string()
    }

    fn title(&self) -> String {
        if self.paused {
            "PasteFlow (Paused)".to_string()
        } else if self.universal_paste {
            format!("PasteFlow (Universal Paste · {} queued)", self.queue_count)
        } else if self.queue_count > 0 {
            format!("PasteFlow ({} queued)", self.queue_count)
        } else {
            "PasteFlow".to_string()
        }
    }

    fn icon_name(&self) -> String {
        if self.paused {
            "media-playback-pause-symbolic".to_string()
        } else {
            "edit-paste-symbolic".to_string()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.send_blocking(TrayCommand::ToggleMain);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem {
                label: "Quick Overlay".to_string(),
                icon_name: "edit-find-symbolic".to_string(),
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.tx.send_blocking(TrayCommand::ToggleOverlay);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Open PasteFlow".to_string(),
                icon_name: "window-new-symbolic".to_string(),
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.tx.send_blocking(TrayCommand::ShowMain);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Shortcuts…".to_string(),
                icon_name: "preferences-desktop-keyboard-shortcuts-symbolic".to_string(),
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.tx.send_blocking(TrayCommand::OpenShortcuts);
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            CheckmarkItem {
                label: "Pause Capture".to_string(),
                checked: self.paused,
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.tx.send_blocking(TrayCommand::TogglePause);
                }),
                ..Default::default()
            }
            .into(),
            CheckmarkItem {
                label: format!("Universal Paste ({} queued)", self.queue_count),
                checked: self.universal_paste,
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.tx.send_blocking(TrayCommand::ToggleUniversalPaste);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Paste Next".to_string(),
                icon_name: "edit-paste-symbolic".to_string(),
                enabled: self.queue_count > 0,
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.tx.send_blocking(TrayCommand::PasteNext);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Clear Queue".to_string(),
                icon_name: "edit-clear-symbolic".to_string(),
                enabled: self.queue_count > 0,
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.tx.send_blocking(TrayCommand::ClearQueue);
                }),
                ..Default::default()
            }
            .into(),
            CheckmarkItem {
                label: "Launch at Startup".to_string(),
                checked: self.autostart,
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.tx.send_blocking(TrayCommand::ToggleAutostart);
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Clear Unpinned History".to_string(),
                icon_name: "edit-clear-all-symbolic".to_string(),
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.tx.send_blocking(TrayCommand::ClearUnpinned);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Quit PasteFlow".to_string(),
                icon_name: "application-exit-symbolic".to_string(),
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.tx.send_blocking(TrayCommand::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

pub fn spawn_tray(
    tx: async_channel::Sender<TrayCommand>,
    paused: bool,
    autostart: bool,
    queue_count: usize,
) -> Result<ksni::blocking::Handle<PasteFlowTray>> {
    let tray = PasteFlowTray {
        tx,
        paused,
        autostart,
        universal_paste: false,
        queue_count,
    };
    let handle = tray.spawn()?;
    Ok(handle)
}
