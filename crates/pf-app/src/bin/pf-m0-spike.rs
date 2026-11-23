//! M0 Spikes S5 (GTK4 overlay XID positioning/focus), S6 (cairo vs ngl on llvmpipe), and S7 (ksni tray).

use anyhow::Result;
use gdk4_x11::X11Surface;
use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, Box as GtkBox, Label, Orientation};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{self, AtomEnum, ConnectionExt as XProtoExt, PropMode};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as WrapperExt;

fn read_rss_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            return rest.split_whitespace().next()?.parse().ok();
        }
    }
    None
}

struct SpikeTray;

impl ksni::Tray for SpikeTray {
    fn id(&self) -> String {
        "app.pasteflow.m0spike".to_string()
    }
    fn title(&self) -> String {
        "PasteFlow M0 Spike".to_string()
    }
    fn icon_name(&self) -> String {
        "edit-paste-symbolic".to_string()
    }
}

fn run_tray_spike() -> Result<()> {
    use ksni::blocking::TrayMethods;
    let handle = SpikeTray.spawn()?;
    std::thread::sleep(std::time::Duration::from_millis(500));
    handle.shutdown();
    println!("S7_TRAY_OK");
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--tray") {
        return run_tray_spike();
    }

    let renderer = std::env::var("GSK_RENDERER").unwrap_or_else(|_| "default".to_string());
    let target_x: i32 = 240;
    let target_y: i32 = 180;
    let start = Instant::now();

    let app = Application::builder()
        .application_id("app.pasteflow.M0Spike")
        .build();

    let painted = Rc::new(Cell::new(false));
    let painted_clone = Rc::clone(&painted);

    app.connect_activate(move |app| {
        let win = ApplicationWindow::builder()
            .application(app)
            .decorated(false)
            .default_width(520)
            .default_height(420)
            .title("PasteFlow M0 Overlay Spike")
            .build();

        let vbox = GtkBox::new(Orientation::Vertical, 8);
        for i in 0..40 {
            vbox.append(&Label::new(Some(&format!(
                "Clip row {i}: sample clipboard entry for renderer benchmark"
            ))));
        }
        win.set_child(Some(&vbox));

        win.connect_realize(move |w| {
            if let Some(surface) = w.surface()
                && let Some(x11_surface) = surface.downcast_ref::<X11Surface>()
            {
                let xid = x11_surface.xid() as u32;
                if let Ok((conn, _)) = RustConnection::connect(None) {
                    // Set _NET_WM_WINDOW_TYPE_UTILITY
                    if let (Some(wt), Some(util)) = (
                        conn.intern_atom(false, b"_NET_WM_WINDOW_TYPE")
                            .ok()
                            .and_then(|c| c.reply().ok()),
                        conn.intern_atom(false, b"_NET_WM_WINDOW_TYPE_UTILITY")
                            .ok()
                            .and_then(|c| c.reply().ok()),
                    ) {
                        let _ = conn.change_property32(
                            PropMode::REPLACE,
                            xid,
                            wt.atom,
                            AtomEnum::ATOM,
                            &[util.atom],
                        );
                    }
                    // Set WM_NORMAL_HINTS with USPosition (flag 1)
                    let mut hints = [0u32; 18];
                    hints[0] = 1 | 4; // USPosition | PPosition
                    hints[1] = target_x as u32;
                    hints[2] = target_y as u32;
                    let _ = conn.change_property32(
                        PropMode::REPLACE,
                        xid,
                        AtomEnum::WM_NORMAL_HINTS,
                        AtomEnum::WM_SIZE_HINTS,
                        &hints,
                    );
                    let _ = conn.configure_window(
                        xid,
                        &xproto::ConfigureWindowAux::new().x(target_x).y(target_y),
                    );
                    let _ = conn.flush();
                }
            }
        });

        let win_weak = win.downgrade();
        let app_weak = app.downgrade();
        let renderer_name = renderer.clone();
        let painted_inner = Rc::clone(&painted_clone);

        win.present();

        gtk4::glib::timeout_add_local_once(std::time::Duration::from_millis(350), move || {
            if !painted_inner.replace(true) {
                let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
                let rss_kib = read_rss_kib().unwrap_or(0);
                let mut actual_pos = None;
                if let Some(w) = win_weak.upgrade()
                    && let Some(surface) = w.surface()
                    && let Some(x11_surface) = surface.downcast_ref::<X11Surface>()
                {
                    let xid = x11_surface.xid() as u32;
                    if let Ok((conn, screen_num)) = RustConnection::connect(None) {
                        let root = conn.setup().roots[screen_num].root;
                        if let Ok(c) = conn.translate_coordinates(xid, root, 0, 0)
                            && let Ok(r) = c.reply()
                        {
                            actual_pos = Some((r.dst_x, r.dst_y));
                        }
                    }
                    w.close();
                }
                println!(
                    "S5_S6 renderer={} elapsed_ms={:.1} rss_mb={:.1} requested=({},{}) actual={:?}",
                    renderer_name,
                    elapsed_ms,
                    rss_kib as f64 / 1024.0,
                    target_x,
                    target_y,
                    actual_pos
                );
                if let Some(a) = app_weak.upgrade() {
                    a.quit();
                }
            }
        });
    });

    app.run_with_args::<&str>(&[]);
    Ok(())
}
