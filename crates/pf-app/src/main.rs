mod app;
mod autostart;
mod cinnamon_keys;
mod tray;
mod ui;

use anyhow::{Context, Result, bail};
use pf_core::backend::{ClipboardBackend, ClipboardEvent};
use pf_core::config::{AppPaths, Config};
use pf_core::model::{ClipFilter, ClipKind};
use pf_store::{InsertOutcome, Store};
use pf_x11::X11Engine;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{info, warn};

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_signal(_sig: libc::c_int) {
    SHUTDOWN.store(true, Ordering::SeqCst);
}

fn install_signal_handlers() {
    // SAFETY: `handle_signal` only sets an `AtomicBool`, which is async-signal-safe.
    unsafe {
        libc::signal(libc::SIGINT, handle_signal as *const () as libc::sighandler_t);
        libc::signal(
            libc::SIGTERM,
            handle_signal as *const () as libc::sighandler_t,
        );
    }
}

fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}

fn main() -> Result<()> {
    init_logging();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        return app::run_gui(&args);
    }

    match args[0].as_str() {
        "--help" | "-h" | "help" => {
            print_usage();
            Ok(())
        }
        "--version" | "-V" => {
            println!("pasteflow {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "--background" | "--toggle" | "--paste-next" | "--quit" | "--exit-after-ready" => {
            app::run_gui(&args)
        }
        "--headless" => run_headless(),
        "list" => cmd_list(&args[1..]),
        "search" => cmd_search(&args[1..]),
        "show" => cmd_show(&args[1..]),
        "copy" => cmd_copy(&args[1..]),
        "delete" => cmd_delete(&args[1..]),
        "pin" => cmd_pin(&args[1..], true),
        "unpin" => cmd_pin(&args[1..], false),
        "clear" => cmd_clear(&args[1..]),
        other => bail!("unknown command or flag: {other} (run `pasteflow --help`)"),
    }
}

fn print_usage() {
    println!(
        "PasteFlow {} — Native Linux clipboard manager\n\
         \n\
         GUI & LIFECYCLE:\n\
         \tpasteflow                            Launch or focus PasteFlow window\n\
         \tpasteflow --background               Start resident in tray with no window\n\
         \tpasteflow --toggle                   Toggle PasteFlow window (single-instance)\n\
         \tpasteflow --paste-next               Paste next queued item (single-instance)\n\
         \tpasteflow --quit                     Gracefully quit running PasteFlow instance\n\
         \n\
         HEADLESS & CLI:\n\
         \tpasteflow --headless                 Run headless X11 clipboard daemon\n\
         \tpasteflow list [--limit N] [--kind K] [--pinned]\n\
         \tpasteflow search <query> [--limit N] [--kind K]\n\
         \tpasteflow show <id>                  Print clip metadata and text preview\n\
         \tpasteflow copy <id>                  Place clip back on X11 CLIPBOARD\n\
         \tpasteflow delete <id>                Delete a clip from history\n\
         \tpasteflow pin <id> | unpin <id>      Pin or unpin a clip\n\
         \tpasteflow clear [--all]              Clear unpinned (or --all) history\n\
         \tpasteflow --version | --help",
        env!("CARGO_PKG_VERSION")
    );
}

fn run_headless() -> Result<()> {
    install_signal_handlers();

    let paths = AppPaths::resolve()?;
    let config = Config::load_or_default(&paths)?;
    let mut store = Store::open(&paths)?;
    let _ = store.sweep_orphan_blobs()?;

    let mut engine = X11Engine::connect(None, config.clone())?;
    info!(
        db = %paths.db_file().display(),
        max_history = config.max_history,
        "PasteFlow headless daemon started"
    );

    while !SHUTDOWN.load(Ordering::Relaxed) {
        let _ = engine.wait_readable(250)?;
        for event in engine.poll_events()? {
            match event {
                ClipboardEvent::Captured(raw) => match store.insert_raw_clip(&raw, &config) {
                    Ok(InsertOutcome::Inserted(rec)) => {
                        info!(
                            id = rec.id,
                            kind = rec.kind.as_str(),
                            bytes = rec.byte_size,
                            hash = %rec.short_hash(),
                            source_app = rec.source_app.as_deref().unwrap_or("unknown"),
                            "captured clip"
                        );
                    }
                    Ok(InsertOutcome::Deduplicated(rec)) => {
                        info!(
                            id = rec.id,
                            copy_count = rec.copy_count,
                            hash = %rec.short_hash(),
                            "deduplicated clip"
                        );
                    }
                    Ok(InsertOutcome::Skipped(reason)) => {
                        info!(reason, "skipped captured clip");
                    }
                    Err(e) => warn!(error = %e, "failed to persist clip"),
                },
                ClipboardEvent::OwnerVanished { .. } => {}
                ClipboardEvent::SelectionServed { target } => {
                    info!(target = %target, "served clipboard target");
                }
                ClipboardEvent::HotkeyPressed { .. } => {}
            }
        }
    }

    info!("shutting down headless daemon");
    let _ = engine.handoff_on_exit();
    let _ = store.wal_checkpoint_truncate();
    Ok(())
}

fn parse_filter_flags(args: &[String], query: Option<String>) -> Result<ClipFilter> {
    let mut filter = ClipFilter {
        query,
        limit: 50,
        ..Default::default()
    };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--limit" => {
                i += 1;
                let v = args.get(i).context("--limit requires a number")?;
                filter.limit = v.parse().context("invalid --limit")?;
            }
            "--kind" => {
                i += 1;
                let v = args.get(i).context("--kind requires a kind name")?;
                filter.kind = Some(
                    ClipKind::from_db(v)
                        .with_context(|| format!("unknown clip kind: {v}"))?,
                );
            }
            "--pinned" => {
                filter.pinned_only = true;
            }
            other => bail!("unknown option: {other}"),
        }
        i += 1;
    }
    Ok(filter)
}

fn cmd_list(args: &[String]) -> Result<()> {
    let paths = AppPaths::resolve()?;
    let store = Store::open(&paths)?;
    let filter = parse_filter_flags(args, None)?;
    let clips = store.list_clips(&filter)?;
    for c in clips {
        let pin = if c.pinned { "*" } else { " " };
        let one_line = c.preview.replace('\n', " ↵ ");
        println!(
            "{pin}{:>5}  [{:<5}]  {:>8} B  {}  {}",
            c.id,
            c.kind.as_str(),
            c.byte_size,
            c.short_hash(),
            one_line
        );
    }
    Ok(())
}

fn cmd_search(args: &[String]) -> Result<()> {
    let query = args
        .first()
        .context("usage: pasteflow search <query> [--limit N] [--kind K]")?
        .clone();
    let paths = AppPaths::resolve()?;
    let store = Store::open(&paths)?;
    let filter = parse_filter_flags(&args[1..], Some(query))?;
    let clips = store.list_clips(&filter)?;
    for c in clips {
        let pin = if c.pinned { "*" } else { " " };
        let one_line = c.preview.replace('\n', " ↵ ");
        println!(
            "{pin}{:>5}  [{:<5}]  {:>8} B  {}  {}",
            c.id,
            c.kind.as_str(),
            c.byte_size,
            c.short_hash(),
            one_line
        );
    }
    Ok(())
}

fn cmd_show(args: &[String]) -> Result<()> {
    let id: i64 = args
        .first()
        .context("usage: pasteflow show <id>")?
        .parse()
        .context("invalid clip id")?;
    let paths = AppPaths::resolve()?;
    let store = Store::open(&paths)?;
    let Some(c) = store.get_clip(id)? else {
        bail!("clip {id} not found");
    };
    println!("id:          {}", c.id);
    println!("kind:        {}", c.kind.as_str());
    println!("hash:        {}", pf_core::model::hex_full(&c.content_hash));
    println!("size:        {} B", c.byte_size);
    println!("pinned:      {}", c.pinned);
    println!("copies:      {}", c.copy_count);
    println!("source_app:  {}", c.source_app.as_deref().unwrap_or("-"));
    if let Some(ref im) = c.image {
        println!(
            "image:       {}x{} ({})",
            im.width, im.height, im.original_mime
        );
    }
    println!("preview:\n{}", c.preview);
    Ok(())
}

fn cmd_copy(args: &[String]) -> Result<()> {
    let id: i64 = args
        .first()
        .context("usage: pasteflow copy <id>")?
        .parse()
        .context("invalid clip id")?;
    let paths = AppPaths::resolve()?;
    let config = Config::load_or_default(&paths)?;
    let store = Store::open(&paths)?;
    let formats = store.get_clip_formats(id)?;
    if formats.is_empty() {
        bail!("clip {id} not found or has no formats");
    }
    let mut engine = X11Engine::connect(None, config)?;
    engine.own_clipboard(formats)?;
    engine.handoff_on_exit()?;
    println!("Copied clip {id} to CLIPBOARD");
    Ok(())
}

fn cmd_delete(args: &[String]) -> Result<()> {
    let id: i64 = args
        .first()
        .context("usage: pasteflow delete <id>")?
        .parse()
        .context("invalid clip id")?;
    let paths = AppPaths::resolve()?;
    let mut store = Store::open(&paths)?;
    if store.delete_clip(id)? {
        println!("Deleted clip {id}");
    } else {
        bail!("clip {id} not found");
    }
    Ok(())
}

fn cmd_pin(args: &[String], pinned: bool) -> Result<()> {
    let id: i64 = args
        .first()
        .context("usage: pasteflow pin|unpin <id>")?
        .parse()
        .context("invalid clip id")?;
    let paths = AppPaths::resolve()?;
    let mut store = Store::open(&paths)?;
    if store.set_pinned(id, pinned)? {
        println!("{} clip {id}", if pinned { "Pinned" } else { "Unpinned" });
    } else {
        bail!("clip {id} not found");
    }
    Ok(())
}

fn cmd_clear(args: &[String]) -> Result<()> {
    let include_pinned = args.iter().any(|a| a == "--all");
    let paths = AppPaths::resolve()?;
    let mut store = Store::open(&paths)?;
    let count = store.clear_history(include_pinned)?;
    println!("Cleared {count} clips");
    Ok(())
}
