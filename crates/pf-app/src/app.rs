use crate::autostart;
use crate::tray::{self, PasteFlowTray, TrayCommand};
use crate::ui;
use crate::ui::main_window::{MainWindow, UiAction};
use anyhow::Result;
use gtk4::gio::{self, ApplicationFlags};
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::Application;
use pf_core::backend::{ClipboardBackend, ClipboardEvent};
use pf_core::config::{AppPaths, Config};
use pf_store::{InsertOutcome, Store};
use pf_x11::X11Engine;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;
use tracing::{info, warn};

pub const APP_ID: &str = "app.pasteflow.PasteFlow";

struct AppState {
    paths: AppPaths,
    config: RefCell<Config>,
    store: Rc<RefCell<Store>>,
    engine: RefCell<Option<X11Engine>>,
    x11_source_id: Cell<Option<u32>>,
    tray: RefCell<Option<ksni::blocking::Handle<PasteFlowTray>>>,
    main_window: RefCell<Option<MainWindow>>,
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

    // Register D-Bus actions for remote CLI/hotkey control
    register_actions(&app, Rc::clone(&state));

    // Startup runs ONLY in the primary instance
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

    // Command-line handler runs in the primary instance for both initial launch and remote invocations
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
                info!("--paste-next invoked (Paste Queue lands in M5)");
                send_notification(
                    app,
                    "paste-next",
                    "PasteFlow",
                    "Paste Queue will be enabled in M5.",
                );
                return 0.into();
            }

            if let Some(st) = state_cmd.borrow().as_ref() {
                if toggle {
                    toggle_main_window(app, st);
                } else if !background {
                    show_main_window(app, st);
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

    // Graceful shutdown: tear down tray, hand off clipboard via SAVE_TARGETS, truncate WAL
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
        let app_weak = app.downgrade();
        toggle_action.connect_activate(move |_, _| {
            if let (Some(a), Some(st)) = (app_weak.upgrade(), st_c.borrow().as_ref()) {
                toggle_main_window(&a, st);
            }
        });
    }
    app.add_action(&toggle_action);

    let show_action = gio::SimpleAction::new("show", None);
    {
        let st_c = Rc::clone(&state);
        let app_weak = app.downgrade();
        show_action.connect_activate(move |_, _| {
            if let (Some(a), Some(st)) = (app_weak.upgrade(), st_c.borrow().as_ref()) {
                show_main_window(&a, st);
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

    // Keep application alive with zero open windows (tray resident)
    let hold_guard = app.hold();

    let (tray_tx, tray_rx) = async_channel::unbounded::<TrayCommand>();
    let autostart_on = autostart::is_autostart_enabled();
    let tray_handle = match tray::spawn_tray(tray_tx, config.paused, autostart_on) {
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
        paths,
        config: RefCell::new(config),
        store: Rc::new(RefCell::new(store)),
        engine: RefCell::new(engine),
        x11_source_id: Cell::new(None),
        tray: RefCell::new(tray_handle),
        main_window: RefCell::new(None),
        hold_guard: RefCell::new(Some(hold_guard)),
        guard_timer_armed: Cell::new(false),
    });

    // Attach X11 socket fd to GLib MainLoop (event-driven, 0 idle wakeups!)
    if let Some(fd) = engine_fd {
        let st_fd = Rc::clone(&state);
        let tag = attach_unix_fd_local(fd, move || {
            pump_x11_events(&st_fd);
            true
        });
        state.x11_source_id.set(Some(tag));
    }

    // Listen for TrayCommands on the GLib MainLoop
    {
        let st_tray = Rc::clone(&state);
        let app_weak = app.downgrade();
        glib::MainContext::default().spawn_local(async move {
            while let Ok(cmd) = tray_rx.recv().await {
                let Some(a) = app_weak.upgrade() else {
                    break;
                };
                handle_tray_command(&a, &st_tray, cmd);
            }
        });
    }

    info!("PasteFlow primary instance ready");
    Ok(state)
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
                        store_changed = true;
                    }
                    Ok(InsertOutcome::Deduplicated(rec)) => {
                        info!(id = rec.id, copy_count = rec.copy_count, "deduplicated clip");
                        store_changed = true;
                    }
                    Ok(InsertOutcome::Skipped(_)) => {}
                    Err(e) => warn!(error = %e, "failed to store clip"),
                }
            }
            ClipboardEvent::OwnerVanished { .. } => {}
            ClipboardEvent::SelectionServed { target } => {
                info!(target = %target, "served clipboard target");
            }
        }
    }

    if store_changed
        && let Some(ref win) = *state.main_window.borrow()
        && win.window().is_visible()
    {
        win.refresh();
    }

    // Arm a single one-shot timer only when the empty-clipboard guard needs to fire
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

fn ensure_main_window(app: &Application, state: &Rc<AppState>) {
    if state.main_window.borrow().is_some() {
        return;
    }
    let st_act = Rc::clone(state);
    let app_weak = app.downgrade();
    let on_action: Rc<dyn Fn(UiAction)> = Rc::new(move |action| {
        if let Some(a) = app_weak.upgrade() {
            handle_ui_action(&a, &st_act, action);
        }
    });

    let paused = state.config.borrow().paused;
    let win = MainWindow::new(
        app,
        Rc::clone(&state.store),
        state.paths.thumbs_dir(),
        paused,
        on_action,
    );
    *state.main_window.borrow_mut() = Some(win);
}

fn show_main_window(app: &Application, state: &Rc<AppState>) {
    ensure_main_window(app, state);
    if let Some(ref win) = *state.main_window.borrow() {
        win.present();
    }
}

fn toggle_main_window(app: &Application, state: &Rc<AppState>) {
    ensure_main_window(app, state);
    if let Some(ref win) = *state.main_window.borrow() {
        win.toggle();
    }
}

fn handle_tray_command(app: &Application, state: &Rc<AppState>, cmd: TrayCommand) {
    match cmd {
        TrayCommand::ShowMain => show_main_window(app, state),
        TrayCommand::ToggleMain => toggle_main_window(app, state),
        TrayCommand::TogglePause => toggle_pause(app, state),
        TrayCommand::ToggleAutostart => {
            let next = !autostart::is_autostart_enabled();
            match autostart::set_autostart_enabled(next) {
                Ok(()) => {
                    if let Some(ref handle) = *state.tray.borrow() {
                        handle.update(|t| t.autostart = next);
                    }
                    send_notification(
                        app,
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
        TrayCommand::ClearUnpinned => handle_ui_action(app, state, UiAction::ClearUnpinned),
        TrayCommand::Quit => app.quit(),
    }
}

fn handle_ui_action(app: &Application, state: &Rc<AppState>, action: UiAction) {
    match action {
        UiAction::CopyClip(id) => {
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
        UiAction::DeleteClip(id) => {
            if let Err(e) = state.store.borrow_mut().delete_clip(id) {
                warn!(error = %e, id, "failed to delete clip");
                return;
            }
            if let Some(ref win) = *state.main_window.borrow() {
                win.refresh();
            }
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
                if let Some(ref win) = *state.main_window.borrow() {
                    win.refresh();
                }
            }
        }
        UiAction::TogglePause => toggle_pause(app, state),
        UiAction::ClearUnpinned => {
            match state.store.borrow_mut().clear_history(false) {
                Ok(n) => {
                    if let Some(ref win) = *state.main_window.borrow() {
                        win.refresh();
                    }
                    send_notification(
                        app,
                        "clear",
                        "PasteFlow",
                        &format!("Cleared {n} unpinned clip{}.", if n == 1 { "" } else { "s" }),
                    );
                }
                Err(e) => warn!(error = %e, "failed to clear history"),
            }
        }
    }
}

fn toggle_pause(app: &Application, state: &Rc<AppState>) {
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
        app,
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
