//! Screen 5 — Settings window (`GtkWindow` + `GtkStackSidebar`) with General, Clipboard,
//! Privacy, Appearance, and Advanced/Diagnostics tabs.

use crate::autostart;
use crate::ui::format_bytes;
use gtk4::prelude::*;
use gtk4::{
    Align, Application, Box as GtkBox, Button, CheckButton, DropDown, Entry, Label, Orientation,
    ScrolledWindow, Separator, SpinButton, Stack, StackSidebar, StringList, Window,
};
use pf_core::config::{AppPaths, Config};
use pf_store::DbStats;
use std::rc::Rc;

#[derive(Debug, Clone)]
pub enum SettingsAction {
    SaveConfig(Config),
    SetAutostart(bool),
    OpenShortcuts,
    ClearUnpinned,
    ClearOlderThan7Days,
    ClearAll,
    VacuumNow,
    BackupNow,
    ExportJson,
    ImportJson,
    OpenDataFolder,
}

/// Apply theme (`system` | `light` | `dark`) for this process only (Mint-Y friendly).
pub fn apply_theme(theme: &str) {
    let Some(settings) = gtk4::Settings::default() else {
        return;
    };
    match theme {
        "light" => {
            settings.set_gtk_application_prefer_dark_theme(false);
            if let Some(name) = settings.gtk_theme_name()
                && name.contains("-Dark-")
            {
                let light = name.replace("-Dark-", "-");
                settings.set_gtk_theme_name(Some(&light));
            }
        }
        "dark" => {
            settings.set_gtk_application_prefer_dark_theme(true);
            if let Some(name) = settings.gtk_theme_name()
                && name.starts_with("Mint-Y-")
                && !name.contains("-Dark-")
            {
                let dark = name.replacen("Mint-Y-", "Mint-Y-Dark-", 1);
                settings.set_gtk_theme_name(Some(&dark));
            }
        }
        _ => {
            settings.reset_property("gtk-application-prefer-dark-theme");
            settings.reset_property("gtk-theme-name");
        }
    }
}

/// Check if another clipboard manager (`copyq`) is running on the system (M0 S8 / Test Matrix #21).
pub fn detect_conflicting_manager() -> Option<String> {
    let proc_dir = std::fs::read_dir("/proc").ok()?;
    for entry in proc_dir.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let comm_path = entry.path().join("comm");
        if let Ok(comm) = std::fs::read_to_string(comm_path) {
            let trimmed = comm.trim();
            if trimmed.eq_ignore_ascii_case("copyq")
                || trimmed.eq_ignore_ascii_case("diodon")
                || trimmed.eq_ignore_ascii_case("parcellite")
                || trimmed.eq_ignore_ascii_case("clipit")
            {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

pub fn show_settings_window(
    app: &Application,
    config: &Config,
    paths: &AppPaths,
    stats: Option<DbStats>,
    on_action: Rc<dyn Fn(SettingsAction)>,
) {
    let win = Window::builder()
        .application(app)
        .title("PasteFlow Settings")
        .default_width(680)
        .default_height(480)
        .build();

    let hbox = GtkBox::new(Orientation::Horizontal, 0);
    let stack = Stack::new();
    stack.set_hexpand(true);
    stack.set_vexpand(true);

    let sidebar = StackSidebar::new();
    sidebar.set_stack(&stack);
    sidebar.set_width_request(160);

    // --- 1. General ---
    let gen_box = make_page();
    let autostart_chk = CheckButton::with_label("Launch PasteFlow at startup (--background)");
    autostart_chk.set_active(autostart::is_autostart_enabled());
    {
        let act = Rc::clone(&on_action);
        autostart_chk.connect_toggled(move |b| act(SettingsAction::SetAutostart(b.is_active())));
    }
    gen_box.append(&autostart_chk);

    let close_note = Label::new(Some(
        "Closing the PasteFlow window hides it to the system tray. Use the tray menu or `pasteflow --quit` to exit.",
    ));
    close_note.set_xalign(0.0);
    close_note.set_wrap(true);
    close_note.add_css_class("pf-clip-meta");
    gen_box.append(&close_note);
    stack.add_titled(&gen_box, Some("general"), "General");

    // --- 2. Clipboard ---
    let clip_box = make_page();
    let hist_row = GtkBox::new(Orientation::Horizontal, 8);
    let hist_lbl = Label::builder()
        .label("Maximum unpinned history in General:")
        .xalign(0.0)
        .hexpand(true)
        .build();
    let max_hist_spin = SpinButton::with_range(100.0, 10_000.0, 100.0);
    max_hist_spin.set_value(config.max_history as f64);
    hist_row.append(&hist_lbl);
    hist_row.append(&max_hist_spin);
    clip_box.append(&hist_row);

    let cap_img_chk = CheckButton::with_label("Capture images (stored in local content-addressed blobs)");
    cap_img_chk.set_active(config.capture_images);
    clip_box.append(&cap_img_chk);

    let cap_pri_chk = CheckButton::with_label("Capture PRIMARY selection (middle-click, debounced 750 ms)");
    cap_pri_chk.set_active(config.capture_primary);
    clip_box.append(&cap_pri_chk);

    let dedupe_chk = CheckButton::with_label("Collapse duplicate copies (bump existing item to top)");
    dedupe_chk.set_active(config.ignore_duplicates);
    clip_box.append(&dedupe_chk);

    let guard_chk = CheckButton::with_label("Empty-clipboard guard (keep clipboard alive when source app exits)");
    guard_chk.set_active(config.empty_clipboard_guard);
    clip_box.append(&guard_chk);

    clip_box.append(&Label::builder().label("Excluded applications (comma-separated WM_CLASS / process names):").xalign(0.0).build());
    let excl_entry = Entry::new();
    excl_entry.set_text(&config.excluded_apps.join(", "));
    clip_box.append(&excl_entry);
    stack.add_titled(&clip_box, Some("clipboard"), "Clipboard");

    // --- 3. Privacy ---
    let priv_box = make_page();
    let secret_chk = CheckButton::with_label("Detect secret-looking clips (API keys, JWTs, OTPs) and store in RAM only");
    secret_chk.set_active(config.detect_secrets);
    priv_box.append(&secret_chk);

    let sec_row = GtkBox::new(Orientation::Horizontal, 8);
    let sec_lbl = Label::builder()
        .label("Auto-expire sensitive clips after (seconds):")
        .xalign(0.0)
        .hexpand(true)
        .build();
    let sec_spin = SpinButton::with_range(10.0, 3600.0, 5.0);
    sec_spin.set_value(config.secret_expiry_secs as f64);
    sec_row.append(&sec_lbl);
    sec_row.append(&sec_spin);
    priv_box.append(&sec_row);

    priv_box.append(&Separator::new(Orientation::Horizontal));
    let priv_note = Label::new(Some(
        "Clearing history removes clips from PasteFlow's local database, FTS index, blobs, and backups. Sensitive clips and ≤ 1 h timers never touch disk.",
    ));
    priv_note.set_xalign(0.0);
    priv_note.set_wrap(true);
    priv_note.add_css_class("pf-clip-meta");
    priv_box.append(&priv_note);

    let clear_row = GtkBox::new(Orientation::Horizontal, 8);
    let clear_unpinned = Button::with_label("Clear Unpinned");
    {
        let act = Rc::clone(&on_action);
        clear_unpinned.connect_clicked(move |_| act(SettingsAction::ClearUnpinned));
    }
    let clear_7d = Button::with_label("Clear > 7 Days");
    {
        let act = Rc::clone(&on_action);
        clear_7d.connect_clicked(move |_| act(SettingsAction::ClearOlderThan7Days));
    }
    let clear_all = Button::with_label("Clear All");
    clear_all.add_css_class("destructive-action");
    {
        let act = Rc::clone(&on_action);
        clear_all.connect_clicked(move |_| act(SettingsAction::ClearAll));
    }
    clear_row.append(&clear_unpinned);
    clear_row.append(&clear_7d);
    clear_row.append(&clear_all);
    priv_box.append(&clear_row);
    stack.add_titled(&priv_box, Some("privacy"), "Privacy");

    // --- 4. Appearance & Shortcuts ---
    let app_box = make_page();
    let theme_row = GtkBox::new(Orientation::Horizontal, 8);
    theme_row.append(&Label::builder().label("Theme:").xalign(0.0).hexpand(true).build());
    let theme_dd = DropDown::new(
        Some(StringList::new(&["System", "Light", "Dark"])),
        gtk4::Expression::NONE,
    );
    let theme_idx = match config.theme.as_str() {
        "light" => 1,
        "dark" => 2,
        _ => 0,
    };
    theme_dd.set_selected(theme_idx);
    theme_row.append(&theme_dd);
    app_box.append(&theme_row);

    let compact_chk = CheckButton::with_label("Compact row density");
    compact_chk.set_active(config.compact_density);
    app_box.append(&compact_chk);

    let ptr_chk = CheckButton::with_label("Open floating overlay at mouse pointer");
    ptr_chk.set_active(config.overlay_at_pointer);
    app_box.append(&ptr_chk);

    app_box.append(&Separator::new(Orientation::Horizontal));
    let sc_btn = Button::with_label("Configure Global Shortcuts…");
    sc_btn.set_halign(Align::Start);
    {
        let act = Rc::clone(&on_action);
        sc_btn.connect_clicked(move |_| act(SettingsAction::OpenShortcuts));
    }
    app_box.append(&sc_btn);
    stack.add_titled(&app_box, Some("appearance"), "Appearance");

    // --- 5. Advanced & Diagnostics ---
    let adv_box = make_page();
    let path_lbl = Label::builder()
        .label(&format!("Data folder: {}", paths.data_dir.display()))
        .xalign(0.0)
        .selectable(true)
        .build();
    adv_box.append(&path_lbl);

    if let Some(st) = stats {
        let stats_lbl = Label::builder()
            .label(&format!(
                "Clips: {} · Spaces: {} · Queue: {} · DB: {} · WAL: {}",
                st.clip_count,
                st.space_count,
                st.queue_count,
                format_bytes(st.db_bytes as i64),
                format_bytes(st.wal_bytes as i64),
            ))
            .xalign(0.0)
            .build();
        stats_lbl.add_css_class("pf-clip-meta");
        adv_box.append(&stats_lbl);
    }

    let maint_row = GtkBox::new(Orientation::Horizontal, 8);
    for (label, action) in [
        ("Open Folder", SettingsAction::OpenDataFolder),
        ("Vacuum Now", SettingsAction::VacuumNow),
        ("Backup Now", SettingsAction::BackupNow),
        ("Export JSON", SettingsAction::ExportJson),
        ("Import JSON", SettingsAction::ImportJson),
    ] {
        let btn = Button::with_label(label);
        let act = Rc::clone(&on_action);
        btn.connect_clicked(move |_| act(action.clone()));
        maint_row.append(&btn);
    }
    adv_box.append(&maint_row);

    adv_box.append(&Separator::new(Orientation::Horizontal));
    let diag_text = build_diagnostics_summary();
    let diag_lbl = Label::builder()
        .label(&diag_text)
        .xalign(0.0)
        .wrap(true)
        .selectable(true)
        .build();
    diag_lbl.add_css_class("pf-clip-meta");
    let diag_scroll = ScrolledWindow::builder()
        .vexpand(true)
        .child(&diag_lbl)
        .build();
    adv_box.append(&diag_scroll);
    stack.add_titled(&adv_box, Some("advanced"), "Advanced");

    // Bottom Save / Close bar
    let right_vbox = GtkBox::new(Orientation::Vertical, 0);
    right_vbox.set_hexpand(true);
    right_vbox.append(&stack);
    right_vbox.append(&Separator::new(Orientation::Horizontal));

    let footer = GtkBox::new(Orientation::Horizontal, 8);
    footer.set_margin_top(10);
    footer.set_margin_bottom(10);
    footer.set_margin_start(12);
    footer.set_margin_end(12);
    footer.set_halign(Align::End);

    let close_btn = Button::with_label("Close");
    {
        let win_weak = win.downgrade();
        close_btn.connect_clicked(move |_| {
            if let Some(w) = win_weak.upgrade() {
                w.close();
            }
        });
    }
    let save_btn = Button::with_label("Save Changes");
    save_btn.add_css_class("suggested-action");
    {
        let base_cfg = config.clone();
        let act = Rc::clone(&on_action);
        let win_weak = win.downgrade();
        save_btn.connect_clicked(move |_| {
            let mut next = base_cfg.clone();
            next.max_history = max_hist_spin.value() as usize;
            next.capture_images = cap_img_chk.is_active();
            next.capture_primary = cap_pri_chk.is_active();
            next.ignore_duplicates = dedupe_chk.is_active();
            next.empty_clipboard_guard = guard_chk.is_active();
            next.excluded_apps = excl_entry
                .text()
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            next.detect_secrets = secret_chk.is_active();
            next.secret_expiry_secs = sec_spin.value() as u64;
            next.theme = match theme_dd.selected() {
                1 => "light",
                2 => "dark",
                _ => "system",
            }
            .to_string();
            next.compact_density = compact_chk.is_active();
            next.overlay_at_pointer = ptr_chk.is_active();

            act(SettingsAction::SaveConfig(next));
            if let Some(w) = win_weak.upgrade() {
                w.close();
            }
        });
    }
    footer.append(&close_btn);
    footer.append(&save_btn);
    right_vbox.append(&footer);

    hbox.append(&sidebar);
    hbox.append(&Separator::new(Orientation::Vertical));
    hbox.append(&right_vbox);
    win.set_child(Some(&hbox));
    win.present();
}

fn make_page() -> GtkBox {
    let b = GtkBox::new(Orientation::Vertical, 10);
    b.set_margin_top(16);
    b.set_margin_bottom(16);
    b.set_margin_start(16);
    b.set_margin_end(16);
    b
}

fn build_diagnostics_summary() -> String {
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".to_string());
    let renderer = std::env::var("GSK_RENDERER").unwrap_or_else(|_| "default".to_string());
    let x11_info = match pf_x11::probe_capabilities() {
        Ok(c) => format!(
            "XFixes {}.{}, XTEST {}.{}, CLIPBOARD_MANAGER={:#x}",
            c.xfixes_version.0,
            c.xfixes_version.1,
            c.xtest_version.0,
            c.xtest_version.1,
            c.clipboard_manager_owner
        ),
        Err(e) => format!("X11 probe error: {e}"),
    };
    let conflict = match detect_conflicting_manager() {
        Some(name) => format!("WARNING: Conflicting clipboard manager running ({name}) — quit it to avoid ownership conflicts."),
        None => "Conflicting managers: none detected".to_string(),
    };
    format!(
        "PasteFlow v{}\nSession: {session} · GTK Renderer: {renderer}\n{x11_info}\n{conflict}\nNetwork crates: none (offline-only)",
        env!("CARGO_PKG_VERSION")
    )
}
