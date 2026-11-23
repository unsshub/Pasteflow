//! X11 clipboard engine (XFixes, ICCCM/INCR, SAVE_TARGETS, XGrabKey, XTEST).

use anyhow::{Context, Result, bail};
use std::time::{Duration, Instant};
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::Event;
use x11rb::protocol::xfixes::{self, ConnectionExt as XFixesExt};
use x11rb::protocol::xproto::{self, AtomEnum, ConnectionExt as XProtoExt, WindowClass};
use x11rb::protocol::xtest::ConnectionExt as XTestExt;
use x11rb::rust_connection::RustConnection;

/// Basic X11 environment capabilities probed during M0 spikes.
#[derive(Debug, Clone)]
pub struct X11Capabilities {
    pub xfixes_version: (u32, u32),
    pub xtest_version: (u32, u32),
    pub clipboard_manager_owner: u32,
    pub max_request_bytes: usize,
}

/// Probe X11 extensions (`XFixes`, `XTEST`), `CLIPBOARD_MANAGER` owner, and max request size.
pub fn probe_capabilities() -> Result<X11Capabilities> {
    let (conn, _screen_num) = RustConnection::connect(None).context("connect to X11 display")?;

    let xfixes_reply = conn
        .xfixes_query_version(5, 0)?
        .reply()
        .context("query XFixes version")?;
    let xtest_reply = conn
        .xtest_get_version(2, 2)?
        .reply()
        .context("query XTEST version")?;

    let cm_atom = conn
        .intern_atom(false, b"CLIPBOARD_MANAGER")?
        .reply()?
        .atom;
    let cm_owner = conn.get_selection_owner(cm_atom)?.reply()?.owner;

    Ok(X11Capabilities {
        xfixes_version: (xfixes_reply.major_version, xfixes_reply.minor_version),
        xtest_version: (
            u32::from(xtest_reply.major_version),
            u32::from(xtest_reply.minor_version),
        ),
        clipboard_manager_owner: cm_owner,
        max_request_bytes: conn.maximum_request_bytes(),
    })
}

/// Resolve an atom name for diagnostics (never returns clipboard content).
pub fn atom_name(conn: &RustConnection, atom: xproto::Atom) -> String {
    if atom == x11rb::NONE {
        return "NONE".to_string();
    }
    match conn.get_atom_name(atom).ok().and_then(|c| c.reply().ok()) {
        Some(reply) => String::from_utf8_lossy(&reply.name).into_owned(),
        None => format!("ATOM({atom})"),
    }
}

/// Best-effort owner window PID and WM_CLASS lookup for diagnostics/exclusions.
pub fn identify_owner(conn: &RustConnection, owner: xproto::Window) -> (Option<u32>, Option<String>) {
    if owner == x11rb::NONE {
        return (None, None);
    }
    let pid_atom = conn
        .intern_atom(false, b"_NET_WM_PID")
        .ok()
        .and_then(|c| c.reply().ok())
        .map(|r| r.atom);

    let pid = pid_atom.and_then(|atom| {
        conn.get_property(false, owner, atom, AtomEnum::CARDINAL, 0, 1)
            .ok()?
            .reply()
            .ok()?
            .value32()?
            .next()
    });

    let wm_class = conn
        .get_property(false, owner, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 256)
        .ok()
        .and_then(|c| c.reply().ok())
        .and_then(|r| {
            if r.value.is_empty() {
                None
            } else {
                let parts: Vec<String> = r
                    .value
                    .split(|&b| b == 0)
                    .filter(|s| !s.is_empty())
                    .map(|s| String::from_utf8_lossy(s).into_owned())
                    .collect();
                if parts.is_empty() {
                    None
                } else {
                    Some(parts.join("/"))
                }
            }
        });

    (pid, wm_class)
}

/// Check if grabbing a key+modifiers combination on the root window succeeds without `BadAccess`.
pub fn test_global_grab(
    conn: &RustConnection,
    root: xproto::Window,
    keycode: xproto::Keycode,
    modifiers: xproto::ModMask,
) -> Result<bool> {
    let lock_masks = [
        xproto::ModMask::from(0u16),
        xproto::ModMask::LOCK,
        xproto::ModMask::M2,
        xproto::ModMask::LOCK | xproto::ModMask::M2,
    ];

    let mut grabbed = Vec::new();
    let mut conflict = false;

    for extra in lock_masks {
        let mask = modifiers | extra;
        let cookie = conn.grab_key(
            false,
            root,
            mask,
            keycode,
            xproto::GrabMode::ASYNC,
            xproto::GrabMode::ASYNC,
        )?;
        match cookie.check() {
            Ok(()) => grabbed.push(mask),
            Err(_) => {
                conflict = true;
                break;
            }
        }
    }

    for mask in grabbed {
        let _ = conn.ungrab_key(keycode, root, mask)?.check();
    }

    Ok(!conflict)
}

/// Resolve the first keycode matching `keysym` from the current X11 keyboard mapping.
pub fn keycode_for_keysym(conn: &RustConnection, keysym: u32) -> Result<Option<xproto::Keycode>> {
    let setup = conn.setup();
    let min = setup.min_keycode;
    let count = setup.max_keycode - min + 1;
    let mapping = conn.get_keyboard_mapping(min, count)?.reply()?;
    let per = usize::from(mapping.keysyms_per_keycode);
    if per == 0 {
        return Ok(None);
    }
    for (idx, chunk) in mapping.keysyms.chunks(per).enumerate() {
        if chunk.contains(&keysym) {
            return Ok(Some(min + u8::try_from(idx).unwrap_or(0)));
        }
    }
    Ok(None)
}

/// Read a selection target synchronously, supporting both direct and ICCCM `INCR` transfers
/// up to `max_bytes` within `timeout`.
pub fn read_selection_target(
    conn: &RustConnection,
    screen_num: usize,
    selection_name: &str,
    target_name: &str,
    max_bytes: usize,
    timeout: Duration,
) -> Result<Vec<u8>> {
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

    let sel_atom = conn.intern_atom(false, selection_name.as_bytes())?.reply()?.atom;
    let target_atom = conn.intern_atom(false, target_name.as_bytes())?.reply()?.atom;
    let prop_atom = conn.intern_atom(false, b"_PASTEFLOW_READ")?.reply()?.atom;
    let incr_atom = conn.intern_atom(false, b"INCR")?.reply()?.atom;

    conn.convert_selection(win, sel_atom, target_atom, prop_atom, x11rb::CURRENT_TIME)?;
    conn.flush()?;

    let deadline = Instant::now() + timeout;
    let mut in_incr = false;
    let mut buf = Vec::new();

    let result = loop {
        if Instant::now() > deadline {
            break Err(anyhow::anyhow!("selection read timed out"));
        }
        if let Some(ev) = conn.poll_for_event()? {
            match ev {
                Event::SelectionNotify(sn) if sn.requestor == win => {
                    if sn.property == x11rb::NONE {
                        break Err(anyhow::anyhow!("selection conversion refused"));
                    }
                    let reply = conn
                        .get_property(true, win, sn.property, AtomEnum::ANY, 0, u32::MAX / 4)?
                        .reply()?;
                    if reply.type_ == incr_atom {
                        in_incr = true;
                        conn.flush()?;
                    } else {
                        if reply.value.len() > max_bytes {
                            break Err(anyhow::anyhow!("payload exceeds max_bytes"));
                        }
                        break Ok(reply.value);
                    }
                }
                Event::PropertyNotify(pn)
                    if in_incr
                        && pn.window == win
                        && pn.atom == prop_atom
                        && pn.state == xproto::Property::NEW_VALUE =>
                {
                    let reply = conn
                        .get_property(true, win, prop_atom, AtomEnum::ANY, 0, u32::MAX / 4)?
                        .reply()?;
                    if reply.value.is_empty() {
                        break Ok(buf);
                    }
                    if buf.len() + reply.value.len() > max_bytes {
                        break Err(anyhow::anyhow!("INCR payload exceeds max_bytes"));
                    }
                    buf.extend_from_slice(&reply.value);
                    conn.flush()?;
                }
                _ => {}
            }
        } else {
            std::thread::sleep(Duration::from_millis(1));
        }
    };

    let _ = conn.destroy_window(win);
    let _ = conn.flush();
    result
}

/// Inject a synthetic key chord via `XTEST` (press modifiers, press+release key, release modifiers).
pub fn inject_xtest_chord(
    conn: &RustConnection,
    modifier_keycodes: &[xproto::Keycode],
    key: xproto::Keycode,
) -> Result<()> {
    for &m in modifier_keycodes {
        conn.xtest_fake_input(xproto::KEY_PRESS_EVENT, m, 0, x11rb::NONE, 0, 0, 0)?;
    }
    conn.xtest_fake_input(xproto::KEY_PRESS_EVENT, key, 0, x11rb::NONE, 0, 0, 0)?;
    conn.xtest_fake_input(xproto::KEY_RELEASE_EVENT, key, 0, x11rb::NONE, 0, 0, 0)?;
    for &m in modifier_keycodes.iter().rev() {
        conn.xtest_fake_input(xproto::KEY_RELEASE_EVENT, m, 0, x11rb::NONE, 0, 0, 0)?;
    }
    conn.flush()?;
    Ok(())
}

/// Subscribe a hidden window to XFixes selection-owner events.
pub fn subscribe_selection_events(
    conn: &RustConnection,
    window: xproto::Window,
    selection: xproto::Atom,
) -> Result<()> {
    let mask = xfixes::SelectionEventMask::SET_SELECTION_OWNER
        | xfixes::SelectionEventMask::SELECTION_WINDOW_DESTROY
        | xfixes::SelectionEventMask::SELECTION_CLIENT_CLOSE;
    conn.xfixes_select_selection_input(window, selection, mask)?
        .check()
        .context("XFixesSelectSelectionInput")?;
    Ok(())
}

/// Ensure a valid non-empty server connection exists; helper for diagnostics.
pub fn verify_x11_ready() -> Result<()> {
    let caps = probe_capabilities()?;
    if caps.xfixes_version.0 < 1 {
        bail!("XFixes unavailable");
    }
    Ok(())
}
