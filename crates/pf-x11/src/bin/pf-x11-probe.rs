//! M0 Spike S1 diagnostic binary: logs XFixes selection owner changes and TARGETS (content-free).

use anyhow::{Context, Result};
use pf_x11::{atom_name, identify_owner, probe_capabilities, subscribe_selection_events};
use std::time::Duration;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{self, AtomEnum, ConnectionExt as XProtoExt, WindowClass};
use x11rb::rust_connection::RustConnection;

fn main() -> Result<()> {
    let caps = probe_capabilities()?;
    println!(
        "X11 capabilities: XFixes={}.{}, XTEST={}.{}, CLIPBOARD_MANAGER=0x{:x}, max_req={} B",
        caps.xfixes_version.0,
        caps.xfixes_version.1,
        caps.xtest_version.0,
        caps.xtest_version.1,
        caps.clipboard_manager_owner,
        caps.max_request_bytes
    );

    if std::env::args().any(|a| a == "--once") {
        return Ok(());
    }

    let (conn, screen_num) = RustConnection::connect(None).context("connect X11")?;
    let screen = &conn.setup().roots[screen_num];
    let win = conn.generate_id()?;
    conn.create_window(
        0,
        win,
        screen.root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        0,
        &xproto::CreateWindowAux::new().event_mask(xproto::EventMask::PROPERTY_CHANGE),
    )?;

    let clipboard = conn.intern_atom(false, b"CLIPBOARD")?.reply()?.atom;
    let targets = conn.intern_atom(false, b"TARGETS")?.reply()?.atom;
    let prop = conn
        .intern_atom(false, b"_PASTEFLOW_PROBE_TARGETS")?
        .reply()?
        .atom;

    subscribe_selection_events(&conn, win, clipboard)?;
    conn.flush()?;

    println!("Listening for CLIPBOARD owner changes (Ctrl+C to stop)...");

    loop {
        let event = conn.wait_for_event()?;
        match event {
            Event::XfixesSelectionNotify(ev) => {
                let (pid, wm_class) = identify_owner(&conn, ev.owner);
                let comm = pid.and_then(|p| {
                    std::fs::read_to_string(format!("/proc/{p}/comm"))
                        .ok()
                        .map(|s| s.trim().to_string())
                });
                println!(
                    "owner=0x{:x} subtype={:?} pid={:?} comm={:?} wm_class={:?}",
                    ev.owner, ev.subtype, pid, comm, wm_class
                );
                if ev.owner != x11rb::NONE && ev.owner != win {
                    conn.convert_selection(win, clipboard, targets, prop, ev.timestamp)?;
                    conn.flush()?;
                }
            }
            Event::SelectionNotify(ev) => {
                if ev.property == x11rb::NONE {
                    println!("  TARGETS: <none>");
                } else {
                    let reply = conn
                        .get_property(true, win, ev.property, AtomEnum::ATOM, 0, 1024)?
                        .reply()?;
                    let names: Vec<String> = reply
                        .value32()
                        .map(|iter| iter.map(|a| atom_name(&conn, a)).collect())
                        .unwrap_or_default();
                    println!("  TARGETS ({}): {:?}", names.len(), names);
                }
                let _ = Duration::from_millis(0);
            }
            _ => {}
        }
    }
}
