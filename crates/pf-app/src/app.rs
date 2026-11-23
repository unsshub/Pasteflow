use crate::autostart;
use crate::cinnamon_keys;
use crate::queue::QueueController;
use crate::tray::{self, PasteFlowTray, TrayCommand};
use crate::ui;
use crate::ui::detail;
use crate::ui::main_window::{MainWindow, UiAction};
use crate::ui::overlay::{OVERLAY_HEIGHT, OVERLAY_WIDTH, OverlayAction, OverlayWindow};
use crate::ui::queue_view::QueueUiAction;
use crate::ui::shortcut_dialog::{ShortcutSettingsUpdate, show_shortcut_dialog};
use anyhow::Result;
use gtk4::gio::{self, ApplicationFlags};
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::Application;
use pf_core::backend::{ClipboardBackend, ClipboardEvent, HotkeyAction};
use pf_core::config::{AppPaths, Config};
use pf_store::{InsertOutcome, Store};
use pf_x11::{HotkeyStatus, X11Engine};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;
use tracing::{info, warn};

pub const APP_ID: &str = "app.pasteflow.PasteFlow";

struct AppState {
    app: Application,
    paths: AppPaths,
    config: RefCell<Config>,
    store: Rc<RefCell<Store>>,
    engine: RefCell<Option<X11Engine>>,
    queue: RefCell<QueueController>,
    x11_source_id: Cell<Option<u32>>,
    hotkey_statuses: RefCell<Vec<HotkeyStatus>>,
    tray: RefCell<Option<ksni::blocking::Handle<PasteFlowTray>>>,
    main_window: RefCell<Option<MainWindow>>,
    overlay_window: RefCell<Option<OverlayWindow>>,
    hold_guard: RefCell<Option<gio::ApplicationHoldGuard>>,
    guard_timer_armed: Cell<bool>,
}

/// Attach a non-owning Unix file descriptor watch to the default GLib MainLoop
/// using `g_unix_fd_add_full` (already linked via `libglib-2.0`).
fn attach_unix_fd_local<F: FnMut() -> bool + 'static>(fd: std::os::fd::RawFd, callback: F) -> u32 {
    use std::ffi::{c_int, c_uint, c_void};

    unsafe extern "C" {
        fn g_unix_fd_add_full(
            priority: c_int,
            fd: c_int,
            condition: c_uint,
            function: extern "C" fn(c_int, c_uint, *mut c_void) -> c_int,
            user_data: *mut c_void,
            notify: extern "C" fn(*mut c_void),
        ) -> c_uint;
    }

    extern "C" fn trampoline<F: FnMut() -> bool>(
        _fd: c_int,
        _cond: c_uint,
        data: *mut c_void,
    ) -> c_int {
        // SAFETY: `data` was created via `Box::into_raw` below and lives until `destroy`.
        let cb = unsafe { &mut *(data as *mut F) };
        if cb() { 1 } else { 0 }
    }

    extern "C" fn destroy<F>(data: *mut c_void) {
        // SAFETY: Called once by GLib when the source is removed.
        unsafe {
            drop(Box::from_raw(data as *mut F));
        }
    }

    const G_PRIORITY_DEFAULT: c_int = 0;
    const G_IO_IN: c_uint = 1;
    let boxed = Box::into_raw(Box::new(callback)) as *mut c_void;
    // SAFETY: `fd` stays open until `remove_glib_source` is called in `connect_shutdown`.
    unsafe {
        g_unix_fd_add_full(
            G_PRIORITY_DEFAULT,
            fd,
            G_IO_IN,
            trampoline::<F>,
            boxed,
            destroy::<F>,
        )
    }
}

fn remove_glib_source(source_id: u32) {
    use std::ffi::{c_int, c_uint};
    unsafe extern "C" {
        fn g_source_remove(tag: c_uint) -> c_int;
    }
    // SAFETY: Valid GSource tag returned by `g_unix_fd_add_full`.
    unsafe {
        let _ = g_source_remove(source_id);
    }
}

/// Launch the single-instance GTK4 PasteFlow application.
pub fn run_gui(raw_args: &[String]) -> Result<()> {
    // M0 S6 decision: default to `cairo` renderer on llvmpipe unless overridden by user.
    if std::env::var_os("GSK_RENDERER").is_none() {
        // SAFETY: Called before `gtk4::init` and before spawning any threads.
        unsafe {
            std::env::set_var("GSK_RENDERER", "cairo");
        }
    }

    let app = Application::builder()
        .application_id(APP_ID)
        .flags(ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();

    let state: Rc<RefCell<Option<Rc<AppState>>>> = Rc::new(RefCell::new(None));

    register_actions(&app, Rc::clone(&state));

    {
        let state_startup = Rc::clone(&state);
        app.connect_startup(move |app| {
            ui::install_css();
            match init_primary_instance(app) {
                Ok(st) => {
                    *state_startup.borrow_mut() = Some(st);
                }
                Err(e) => {
                    warn!(error = %e, "failed to initialize PasteFlow primary instance");
                    app.quit();
                }
            }
        });
    }

    {
        let state_cmd = Rc::clone(&state);
        app.connect_command_line(move |app, cmdline| {
            let args: Vec<String> = cmdline
                .arguments()
                .into_iter()
                .skip(1)
                .map(|a| a.to_string_lossy().into_owned())
                .collect();

            let background = args.iter().any(|a| a == "--background");
            let toggle = args.iter().any(|a| a == "--toggle");
            let paste_next = args.iter().any(|a| a == "--paste-next");
            let quit = args.iter().any(|a| a == "--quit");
            let exit_after_ready = args.iter().any(|a| a == "--exit-after-ready");

            if quit {
                app.quit();
                return 0.into();
            }

            if paste_next {
                if let Some(st) = state_cmd.borrow().as_ref() {
                    let prev = st.engine.borrow().as_ref().and_then(|e| e.active_window());
                    trigger_paste_next(st, prev);
                }
                return 0.into();
            }

            if let Some(st) = state_cmd.borrow().as_ref() {
                if toggle {
                    let prev = st
                        .engine
                        .borrow()
                        .as_ref()
                        .and_then(|e| e.active_window());
                    toggle_overlay(st, None, prev);
                } else if !background {
                    show_main_window(st);
                }
            }

            if exit_after_ready {
                let app_weak = app.downgrade();
                glib::idle_add_local_once(move || {
                    if let Some(a) = app_weak.upgrade() {
                        a.quit();
                    }
                });
            }

            0.into()
        });
    }

    {
        let state_shutdown = Rc::clone(&state);
        app.connect_shutdown(move |_| {
            if let Some(st) = state_shutdown.borrow().as_ref() {
                info!("PasteFlow shutting down gracefully");
                if let Some(tag) = st.x11_source_id.take() {
                    remove_glib_source(tag);
                }
                if let Some(handle) = st.tray.borrow_mut().take() {
                    handle.shutdown();
                }
                if let Some(ref mut eng) = *st.engine.borrow_mut() {
                    eng.ungrab_hotkeys();
                    if let Err(e) = eng.handoff_on_exit() {
                        warn!(error = %e, "SAVE_TARGETS handoff failed");
                    }
                }
                if let Err(e) = st.store.borrow().wal_checkpoint_truncate() {
                    warn!(error = %e, "WAL checkpoint failed on shutdown");
                }
                let _ = st.hold_guard.borrow_mut().take();
            }
        });
    }

    let mut argv = vec!["pasteflow".to_string()];
    argv.extend_from_slice(raw_args);
    let _exit = app.run_with_args(&argv);
    Ok(())
}

fn register_actions(app: &Application, state: Rc<RefCell<Option<Rc<AppState>>>>) {
    let toggle_action = gio::SimpleAction::new("toggle", None);
    {
        let st_c = Rc::clone(&state);
        toggle_action.connect_activate(move |_, _| {
            if let Some(st) = st_c.borrow().as_ref() {
                let prev = st.engine.borrow().as_ref().and_then(|e| e.active_window());
                toggle_overlay(st, None, prev);
            }
        });
    }
    app.add_action(&toggle_action);

    let show_action = gio::SimpleAction::new("show", None);
    {
        let st_c = Rc::clone(&state);
        show_action.connect_activate(move |_, _| {
            if let Some(st) = st_c.borrow().as_ref() {
                show_main_window(st);
            }
        });
    }
    app.add_action(&show_action);

    let quit_action = gio::SimpleAction::new("quit", None);
    {
        let app_weak = app.downgrade();
        quit_action.connect_activate(move |_, _| {
            if let Some(a) = app_weak.upgrade() {
                a.quit();
            }
        });
    }
    app.add_action(&quit_action);
}

fn init_primary_instance(app: &Application) -> Result<Rc<AppState>> {
    let paths = AppPaths::resolve()?;
    let config = Config::load_or_default(&paths)?;
    let store = Store::open(&paths)?;
    let _ = store.sweep_orphan_blobs()?;

    let hold_guard = app.hold();

    let (tray_tx, tray_rx) = async_channel::unbounded::<TrayCommand>();
    let autostart_on = autostart::is_autostart_enabled();
    let initial_queue_count = store.queue_count().unwrap_or(0);
    let tray_handle = match tray::spawn_tray(tray_tx, config.paused, autostart_on, initial_queue_count) {
        Ok(h) => Some(h),
        Err(e) => {
            warn!(error = %e, "tray icon unavailable; continuing without SNI tray");
            None
        }
    };

    let engine = match X11Engine::connect(None, config.clone()) {
        Ok(eng) => Some(eng),
        Err(e) => {
            warn!(error = %e, "X11 clipboard engine unavailable");
            None
        }
    };

    let engine_fd = engine.as_ref().map(|e| e.as_raw_fd());

    let state = Rc::new(AppState {
        app: app.clone(),
        paths,
        config: RefCell::new(config),
        store: Rc::new(RefCell::new(store)),
        engine: RefCell::new(engine),
        queue: RefCell::new(QueueController::default()),
        x11_source_id: Cell::new(None),
        hotkey_statuses: RefCell::new(Vec::new()),
        tray: RefCell::new(tray_handle),
        main_window: RefCell::new(None),
        overlay_window: RefCell::new(None),
        hold_guard: RefCell::new(Some(hold_guard)),
        guard_timer_armed: Cell::new(false),
    });

    // Apply global shortcut provider (X11 grab or Cinnamon custom keybindings)
    let statuses = apply_hotkey_provider(&state);
    *state.hotkey_statuses.borrow_mut() = statuses;

    if let Some(fd) = engine_fd {
        let st_fd = Rc::clone(&state);
        let tag = attach_unix_fd_local(fd, move || {
            pump_x11_events(&st_fd);
            true
        });
        state.x11_source_id.set(Some(tag));
    }

    {
        let st_tray = Rc::clone(&state);
        glib::MainContext::default().spawn_local(async move {
            while let Ok(cmd) = tray_rx.recv().await {
                handle_tray_command(&st_tray, cmd);
            }
        });
    }

    info!("PasteFlow primary instance ready");
    Ok(state)
}

fn apply_hotkey_provider(state: &Rc<AppState>) -> Vec<HotkeyStatus> {
    let cfg = state.config.borrow().clone();
    if cfg.use_cinnamon_keybindings {
        if let Some(ref mut eng) = *state.engine.borrow_mut() {
            eng.ungrab_hotkeys();
        }
        if let Err(e) = cinnamon_keys::sync_cinnamon_keybindings(
            true,
            &cfg.overlay_shortcut,
            &cfg.paste_next_shortcut,
        ) {
            warn!(error = %e, "failed to sync Cinnamon custom keybindings");
        }
        return vec![
            HotkeyStatus {
                action: HotkeyAction::ToggleOverlay,
                accel: cfg.overlay_shortcut,
                grabbed: true,
                error: None,
            },
            HotkeyStatus {
                action: HotkeyAction::PasteNext,
                accel: cfg.paste_next_shortcut,
                grabbed: true,
                error: None,
            },
        ];
    }

    // X11 in-process grab provider
    let bindings = [
        (HotkeyAction::ToggleOverlay, cfg.overlay_shortcut.clone()),
        (HotkeyAction::PasteNext, cfg.paste_next_shortcut.clone()),
    ];
    if let Some(ref mut eng) = *state.engine.borrow_mut() {
        eng.register_hotkeys(&bindings)
    } else {
        Vec::new()
    }
}

fn pump_x11_events(state: &Rc<AppState>) {
    let events = {
        let mut eng_ref = state.engine.borrow_mut();
        let Some(ref mut eng) = *eng_ref else {
            return;
        };
        match eng.poll_events() {
            Ok(ev) => ev,
            Err(e) => {
                warn!(error = %e, "X11 poll_events error");
                return;
            }
        }
    };

    let mut store_changed = false;
    let cfg = state.config.borrow().clone();

    for ev in events {
        match ev {
            ClipboardEvent::Captured(raw) => {
                let res = state.store.borrow_mut().insert_raw_clip(&raw, &cfg);
                match res {
                    Ok(InsertOutcome::Inserted(rec)) => {
                        info!(
                            id = rec.id,
                            kind = rec.kind.as_str(),
                            bytes = rec.byte_size,
                            hash = %rec.short_hash(),
                            "captured clip"
                        );
                        if state.queue.borrow().universal_paste {
                            let _ = state.store.borrow_mut().queue_enqueue(rec.id);
                            sync_queue_indicators(state);
                        }
                        store_changed = true;
                    }
                    Ok(InsertOutcome::Deduplicated(rec)) => {
                        info!(id = rec.id, copy_count = rec.copy_count, "deduplicated clip");
                        if state.queue.borrow().universal_paste {
                            let _ = state.store.borrow_mut().queue_enqueue(rec.id);
                            sync_queue_indicators(state);
                        }
                        store_changed = true;
                    }
                    Ok(InsertOutcome::Skipped(_)) => {}
                    Err(e) => warn!(error = %e, "failed to store clip"),
                }
            }
            ClipboardEvent::OwnerVanished { .. } => {}
            ClipboardEvent::SelectionServed {
                target,
                is_clipboard_manager,
            } => {
                info!(target = %target, is_clipboard_manager, "served clipboard target");
                if !is_clipboard_manager {
                    confirm_in_flight_paste(state);
                }
            }
            ClipboardEvent::HotkeyPressed {
                action,
                timestamp,
                previous_window,
            } => match action {
                HotkeyAction::ToggleOverlay => {
                    toggle_overlay(state, Some(timestamp), previous_window);
                }
                HotkeyAction::PasteNext => {
                    trigger_paste_next(state, previous_window);
                }
            },
        }
    }

    if store_changed {
        refresh_open_views(state);
    }

    let next_ms = state
        .engine
        .borrow()
        .as_ref()
        .map(|e| e.next_timeout_ms())
        .unwrap_or(-1);
    if next_ms > 0 && !state.guard_timer_armed.replace(true) {
        let st_timer = Rc::clone(state);
        glib::timeout_add_local_once(Duration::from_millis(next_ms as u64 + 5), move || {
            st_timer.guard_timer_armed.set(false);
            pump_x11_events(&st_timer);
        });
    }
}

fn refresh_open_views(state: &Rc<AppState>) {
    if let Some(ref win) = *state.main_window.borrow()
        && win.window().is_visible()
    {
        win.refresh();
    }
    if let Some(ref ov) = *state.overlay_window.borrow()
        && ov.is_visible()
    {
        ov.refresh();
    }
}

fn ensure_main_window(state: &Rc<AppState>) {
    if state.main_window.borrow().is_some() {
        return;
    }
    let st_act = Rc::clone(state);
    let on_action: Rc<dyn Fn(UiAction)> = Rc::new(move |action| {
        handle_ui_action(&st_act, action);
    });

    let paused = state.config.borrow().paused;
    let win = MainWindow::new(
        &state.app,
        Rc::clone(&state.store),
        state.paths.thumbs_dir(),
        paused,
        on_action,
    );
    *state.main_window.borrow_mut() = Some(win);
}

fn ensure_overlay_window(state: &Rc<AppState>) {
    if state.overlay_window.borrow().is_some() {
        return;
    }
    let st_act = Rc::clone(state);
    let on_action: Rc<dyn Fn(OverlayAction)> = Rc::new(move |action| {
        handle_overlay_action(&st_act, action);
    });
    let ov = OverlayWindow::new(
        &state.app,
        Rc::clone(&state.store),
        state.paths.thumbs_dir(),
        on_action,
    );
    *state.overlay_window.borrow_mut() = Some(ov);
}

fn show_main_window(state: &Rc<AppState>) {
    ensure_main_window(state);
    if let Some(ref win) = *state.main_window.borrow() {
        win.present();
    }
}

fn toggle_main_window(state: &Rc<AppState>) {
    ensure_main_window(state);
    if let Some(ref win) = *state.main_window.borrow() {
        win.toggle();
    }
}

fn toggle_overlay(
    state: &Rc<AppState>,
    user_time: Option<u32>,
    previous_window: Option<u32>,
) {
    ensure_overlay_window(state);
    let overlay_at_pointer = state.config.borrow().overlay_at_pointer;
    if let Some(ref ov) = *state.overlay_window.borrow() {
        if ov.is_visible() {
            ov.hide();
            if let Some(prev) = previous_window
                && let Some(ref eng) = *state.engine.borrow()
            {
                let _ = eng.focus_window(prev);
            }
        } else {
            ov.show_and_focus(previous_window, |xid| {
                if overlay_at_pointer
                    && let Some(ref eng) = *state.engine.borrow()
                    && let Some((px, py)) = eng.pointer_position()
                {
                    let _ = eng.position_overlay_window(
                        xid,
                        px - OVERLAY_WIDTH / 2,
                        py - 24,
                        OVERLAY_WIDTH,
                        OVERLAY_HEIGHT,
                        user_time,
                    );
                }
            });
        }
    }
}

fn open_shortcuts_dialog(state: &Rc<AppState>) {
    let cfg = state.config.borrow().clone();
    let statuses = state.hotkey_statuses.borrow().clone();
    let st_c = Rc::clone(state);
    let on_apply: Rc<dyn Fn(ShortcutSettingsUpdate) -> Vec<HotkeyStatus>> = Rc::new(move |upd| {
        let was_cinnamon = st_c.config.borrow().use_cinnamon_keybindings;
        {
            let mut c = st_c.config.borrow_mut();
            c.overlay_shortcut = upd.overlay_shortcut;
            c.paste_next_shortcut = upd.paste_next_shortcut;
            c.use_cinnamon_keybindings = upd.use_cinnamon_keybindings;
            c.overlay_at_pointer = upd.overlay_at_pointer;
            let _ = c.save(&st_c.paths);
        }
        if was_cinnamon && !upd.use_cinnamon_keybindings {
            let _ = cinnamon_keys::sync_cinnamon_keybindings(false, "", "");
        }
        let new_statuses = apply_hotkey_provider(&st_c);
        *st_c.hotkey_statuses.borrow_mut() = new_statuses.clone();
        new_statuses
    });
    show_shortcut_dialog(&state.app, &cfg, &statuses, on_apply);
}

fn handle_overlay_action(state: &Rc<AppState>, action: OverlayAction) {
    match action {
        OverlayAction::CopyAndClose {
            clip_id,
            previous_window,
        } => {
            copy_clip_to_clipboard(state, clip_id);
            if let Some(prev) = previous_window
                && let Some(ref eng) = *state.engine.borrow()
            {
                let _ = eng.focus_window(prev);
            }
        }
        OverlayAction::PasteAndClose {
            clip_id,
            previous_window,
        } => {
            copy_clip_to_clipboard(state, clip_id);
            if let Some(prev) = previous_window
                && let Some(ref eng) = *state.engine.borrow()
            {
                let _ = eng.focus_window(prev);
            }
            // Brief delay so Muffin finishes restoring focus before XTEST injects Ctrl+V
            let st_c = Rc::clone(state);
            glib::timeout_add_local_once(Duration::from_millis(60), move || {
                if let Some(ref eng) = *st_c.engine.borrow() {
                    if let Err(e) = eng.send_paste_chord(previous_window) {
                        warn!(error = %e, "synthetic paste failed");
                    }
                }
            });
        }
        OverlayAction::TogglePin(id) => handle_ui_action(state, UiAction::TogglePin(id)),
        OverlayAction::AddToQueue(id) => handle_ui_action(state, UiAction::AddToQueue(id)),
        OverlayAction::DeleteClip(id) => handle_ui_action(state, UiAction::DeleteClip(id)),
        OverlayAction::Closed { previous_window } => {
            if let Some(prev) = previous_window
                && let Some(ref eng) = *state.engine.borrow()
            {
                let _ = eng.focus_window(prev);
            }
        }
    }
}

fn handle_tray_command(state: &Rc<AppState>, cmd: TrayCommand) {
    match cmd {
        TrayCommand::ShowMain => show_main_window(state),
        TrayCommand::ToggleMain => toggle_main_window(state),
        TrayCommand::ToggleOverlay => {
            let prev = state.engine.borrow().as_ref().and_then(|e| e.active_window());
            toggle_overlay(state, None, prev);
        }
        TrayCommand::OpenShortcuts => open_shortcuts_dialog(state),
        TrayCommand::TogglePause => toggle_pause(state),
        TrayCommand::ToggleUniversalPaste => {
            let next = !state.queue.borrow().universal_paste;
            set_universal_paste(state, next);
        }
        TrayCommand::PasteNext => {
            let prev = state.engine.borrow().as_ref().and_then(|e| e.active_window());
            trigger_paste_next(state, prev);
        }
        TrayCommand::ClearQueue => handle_queue_ui_action(state, QueueUiAction::ClearQueue),
        TrayCommand::ToggleAutostart => {
            let next = !autostart::is_autostart_enabled();
            match autostart::set_autostart_enabled(next) {
                Ok(()) => {
                    if let Some(ref handle) = *state.tray.borrow() {
                        handle.update(|t| t.autostart = next);
                    }
                    send_notification(
                        &state.app,
                        "autostart",
                        "PasteFlow",
                        if next {
                            "Launch at startup enabled."
                        } else {
                            "Launch at startup disabled."
                        },
                    );
                }
                Err(e) => warn!(error = %e, "failed to toggle autostart"),
            }
        }
        TrayCommand::ClearUnpinned => handle_ui_action(state, UiAction::ClearUnpinned),
        TrayCommand::Quit => state.app.quit(),
    }
}

fn copy_clip_to_clipboard(state: &Rc<AppState>, id: i64) {
    let formats = match state.store.borrow().get_clip_formats(id) {
        Ok(f) if !f.is_empty() => f,
        Ok(_) => return,
        Err(e) => {
            warn!(error = %e, id, "failed to load clip formats");
            return;
        }
    };
    if let Some(ref mut eng) = *state.engine.borrow_mut() {
        if let Err(e) = eng.own_clipboard(formats) {
            warn!(error = %e, id, "failed to own clipboard");
            return;
        }
        info!(id, "copied clip back to CLIPBOARD");
    }
}

fn handle_ui_action(state: &Rc<AppState>, action: UiAction) {
    match action {
        UiAction::CopyClip(id) => copy_clip_to_clipboard(state, id),
        UiAction::DeleteClip(id) => {
            if let Err(e) = state.store.borrow_mut().delete_clip(id) {
                warn!(error = %e, id, "failed to delete clip");
                return;
            }
            refresh_open_views(state);
        }
        UiAction::TogglePin(id) => {
            let current = state
                .store
                .borrow()
                .get_clip(id)
                .ok()
                .flatten()
                .map(|c| c.pinned);
            if let Some(pinned) = current {
                let _ = state.store.borrow_mut().set_pinned(id, !pinned);
                refresh_open_views(state);
            }
        }
        UiAction::TogglePause => toggle_pause(state),
        UiAction::ClearUnpinned => match state.store.borrow_mut().clear_history(false) {
            Ok(n) => {
                refresh_open_views(state);
                send_notification(
                    &state.app,
                    "clear",
                    "PasteFlow",
                    &format!("Cleared {n} unpinned clip{}.", if n == 1 { "" } else { "s" }),
                );
            }
            Err(e) => warn!(error = %e, "failed to clear history"),
        },
        UiAction::OpenShortcuts => open_shortcuts_dialog(state),
        UiAction::CopyText(text) => {
            if let Some(ref mut eng) = *state.engine.borrow_mut() {
                let _ = eng.own_clipboard(vec![pf_core::model::RawFormat {
                    mime: "text/plain;charset=utf-8".to_string(),
                    data: text.into_bytes(),
                }]);
            }
        }
        UiAction::SaveImageToDownloads(id) => save_image_to_downloads(state, id),
        UiAction::OpenImageInViewer(id) => open_image_in_viewer(state, id),
        UiAction::OpenUrl(url) => {
            if let Err(e) = detail::launch_default_uri(&url) {
                warn!(error = %e, "failed to open URL");
            }
        }
        UiAction::SetLanguage { clip_id, lang } => {
            let meta = if lang.is_empty() {
                None
            } else {
                Some(serde_json::json!({ "lang": lang }).to_string())
            };
            let _ = state
                .store
                .borrow_mut()
                .update_clip_meta(clip_id, meta.as_deref());
            refresh_open_views(state);
        }
        UiAction::AddToQueue(clip_id) => {
            if state.store.borrow_mut().queue_enqueue(clip_id).is_ok() {
                sync_queue_indicators(state);
            }
        }
        UiAction::Queue(qa) => handle_queue_ui_action(state, qa),
        UiAction::ToggleUniversalPasteShortcut => {
            let next = !state.queue.borrow().universal_paste;
            set_universal_paste(state, next);
        }
    }
}

fn sync_queue_indicators(state: &Rc<AppState>) {
    let count = state.store.borrow().queue_count().unwrap_or(0);
    let (universal, manual) = {
        let q = state.queue.borrow();
        (q.universal_paste, q.manual_mode)
    };
    if let Some(ref handle) = *state.tray.borrow() {
        handle.update(|t| {
            t.universal_paste = universal;
            t.queue_count = count;
        });
    }
    if let Some(ref win) = *state.main_window.borrow() {
        win.sync_queue_state(universal, manual, count);
    }
}

fn set_universal_paste(state: &Rc<AppState>, enabled: bool) {
    state.queue.borrow_mut().universal_paste = enabled;
    sync_queue_indicators(state);
    send_notification(
        &state.app,
        "universal-paste",
        "PasteFlow",
        if enabled {
            "Universal Paste ON — every copy joins the queue."
        } else {
            "Universal Paste OFF."
        },
    );
}

fn handle_queue_ui_action(state: &Rc<AppState>, action: QueueUiAction) {
    match action {
        QueueUiAction::ToggleUniversalPaste(enabled) => set_universal_paste(state, enabled),
        QueueUiAction::ToggleManualMode(manual) => {
            state.queue.borrow_mut().manual_mode = manual;
            sync_queue_indicators(state);
        }
        QueueUiAction::PasteNext => {
            let prev = state.engine.borrow().as_ref().and_then(|e| e.active_window());
            trigger_paste_next(state, prev);
        }
        QueueUiAction::SkipHead => {
            if let Ok(Some(head)) = state.store.borrow().queue_peek_head() {
                let _ = state.store.borrow_mut().queue_remove(head.id);
                sync_queue_indicators(state);
            }
        }
        QueueUiAction::ClearQueue => {
            let _ = state.store.borrow_mut().queue_clear();
            {
                let mut q = state.queue.borrow_mut();
                q.in_flight = None;
                q.buffered_presses = 0;
            }
            sync_queue_indicators(state);
        }
        QueueUiAction::MoveItem {
            queue_item_id,
            delta,
        } => {
            let _ = state.store.borrow_mut().queue_move(queue_item_id, delta);
            sync_queue_indicators(state);
        }
        QueueUiAction::RemoveItem(qid) => {
            let _ = state.store.borrow_mut().queue_remove(qid);
            sync_queue_indicators(state);
        }
    }
}

fn trigger_paste_next(state: &Rc<AppState>, target_window: Option<u32>) {
    // Buffer rapid presses while an auto-paste is in flight (§16 #1)
    {
        let mut q = state.queue.borrow_mut();
        if q.in_flight.is_some() && !q.manual_mode {
            q.buffered_presses += 1;
            return;
        }
    }

    let head = match state.store.borrow().queue_peek_head() {
        Ok(Some(h)) => h,
        Ok(None) => {
            send_notification(&state.app, "queue", "Paste Queue", "Paste Queue is empty.");
            return;
        }
        Err(e) => {
            warn!(error = %e, "failed to peek paste queue");
            return;
        }
    };

    let formats = match state.store.borrow().get_clip_formats(head.clip_id) {
        Ok(f) if !f.is_empty() => f,
        _ => {
            let _ = state.store.borrow_mut().queue_remove(head.id);
            sync_queue_indicators(state);
            return;
        }
    };

    if let Some(ref mut eng) = *state.engine.borrow_mut() {
        if let Err(e) = eng.own_clipboard(formats) {
            warn!(error = %e, "failed to own clipboard for queue head");
            return;
        }
    }

    let manual = state.queue.borrow().manual_mode;
    let remaining = state.store.borrow().queue_count().unwrap_or(1);

    if manual {
        let _ = state.queue.borrow_mut().begin_in_flight(head.id);
        let short_preview: String = head.clip.preview.chars().take(40).collect();
        send_notification(
            &state.app,
            "queue-manual",
            "Paste Queue (Manual)",
            &format!("Next: {short_preview} — {remaining} left. Press Ctrl+V."),
        );
        return;
    }

    // Auto X11 mode: inject Ctrl+V (or Ctrl+Shift+V for terminals) and wait up to 2 s for SelectionRequest
    let generation = state.queue.borrow_mut().begin_in_flight(head.id);
    if let Some(ref eng) = *state.engine.borrow() {
        if let Err(e) = eng.send_paste_chord(target_window) {
            warn!(error = %e, "XTEST paste failed; falling back to manual");
            state.queue.borrow_mut().in_flight = None;
            send_notification(
                &state.app,
                "queue-fallback",
                "Paste Queue",
                "Paste not detected — press Ctrl+V manually.",
            );
            return;
        }
    }

    let st_c = Rc::clone(state);
    glib::timeout_add_local_once(Duration::from_secs(2), move || {
        if st_c.queue.borrow_mut().handle_timeout(generation) {
            send_notification(
                &st_c.app,
                "queue-timeout",
                "Paste Queue",
                "Paste not detected — press Ctrl+V manually.",
            );
        }
    });
}

fn confirm_in_flight_paste(state: &Rc<AppState>) {
    let confirmed_qid = state.queue.borrow_mut().confirm_served();
    let Some(qid) = confirmed_qid else {
        return;
    };

    let _ = state.store.borrow_mut().queue_remove(qid);
    let left = state.store.borrow().queue_count().unwrap_or(0);
    if left == 0 && state.queue.borrow().universal_paste {
        state.queue.borrow_mut().universal_paste = false;
        send_notification(&state.app, "queue-done", "Paste Queue", "All queued items pasted.");
    }
    sync_queue_indicators(state);

    if left > 0 && state.queue.borrow_mut().take_buffered_press() {
        let st_c = Rc::clone(state);
        glib::timeout_add_local_once(Duration::from_millis(50), move || {
            let prev = st_c.engine.borrow().as_ref().and_then(|e| e.active_window());
            trigger_paste_next(&st_c, prev);
        });
    }
}

fn save_image_to_downloads(state: &Rc<AppState>, id: i64) {
    let Ok(formats) = state.store.borrow().get_clip_formats(id) else {
        return;
    };
    let Some(img) = formats.iter().find(|f| f.mime.starts_with("image/")) else {
        return;
    };
    let ext = pf_store::blobs::mime_to_ext(&img.mime);
    let downloads = directories::UserDirs::new()
        .and_then(|u| u.download_dir().map(std::path::Path::to_path_buf))
        .unwrap_or_else(|| state.paths.data_dir.clone());
    let _ = std::fs::create_dir_all(&downloads);
    let filename = format!("pasteflow-{id}-{}.{ext}", pf_core::model::now_unix_ms() / 1000);
    let dest = downloads.join(&filename);
    match std::fs::write(&dest, &img.data) {
        Ok(()) => {
            info!(path = %dest.display(), "saved image to Downloads");
            send_notification(
                &state.app,
                "save-image",
                "Image Saved",
                &format!("Saved to {}", dest.display()),
            );
        }
        Err(e) => warn!(error = %e, "failed to save image to Downloads"),
    }
}

fn open_image_in_viewer(state: &Rc<AppState>, id: i64) {
    let Ok(formats) = state.store.borrow().get_clip_formats(id) else {
        return;
    };
    let Some(img) = formats.iter().find(|f| f.mime.starts_with("image/")) else {
        return;
    };
    let ext = pf_store::blobs::mime_to_ext(&img.mime);
    let tmp = state
        .paths
        .cache_dir
        .join(format!("preview-{id}.{ext}"));
    if std::fs::write(&tmp, &img.data).is_ok() {
        let uri = format!("file://{}", tmp.display());
        if let Err(e) = detail::launch_default_uri(&uri) {
            warn!(error = %e, "failed to open image in default viewer");
        }
    }
}

fn toggle_pause(state: &Rc<AppState>) {
    let next = {
        let mut cfg = state.config.borrow_mut();
        cfg.paused = !cfg.paused;
        let _ = cfg.save(&state.paths);
        cfg.paused
    };
    if let Some(ref mut eng) = *state.engine.borrow_mut() {
        eng.set_paused(next);
    }
    if let Some(ref handle) = *state.tray.borrow() {
        handle.update(|t| t.paused = next);
    }
    if let Some(ref win) = *state.main_window.borrow() {
        win.set_paused_ui(next);
    }
    send_notification(
        &state.app,
        "pause",
        "PasteFlow",
        if next {
            "Clipboard capture paused."
        } else {
            "Clipboard capture resumed."
        },
    );
}

pub fn send_notification(app: &Application, id: &str, title: &str, body: &str) {
    let notif = gio::Notification::new(title);
    notif.set_body(Some(body));
    app.send_notification(Some(id), &notif);
}
