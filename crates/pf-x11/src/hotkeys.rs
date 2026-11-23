use anyhow::Result;
use pf_core::backend::HotkeyAction;
use pf_core::config::ShortcutSpec;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{self, AtomEnum, ConnectionExt as XProtoExt, PropMode};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as WrapperExt;

fn lock_masks() -> [xproto::ModMask; 4] {
    [
        xproto::ModMask::from(0u16),
        xproto::ModMask::LOCK,
        xproto::ModMask::M2,
        xproto::ModMask::LOCK | xproto::ModMask::M2,
    ]
}

/// Status of registering a single global hotkey.
#[derive(Debug, Clone)]
pub struct HotkeyStatus {
    pub action: HotkeyAction,
    pub accel: String,
    pub grabbed: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct GrabbedHotkey {
    pub action: HotkeyAction,
    pub keycode: xproto::Keycode,
    pub modifiers: xproto::ModMask,
}

/// Convert a `ShortcutSpec`'s modifiers to an X11 `ModMask`.
pub fn spec_to_modmask(spec: &ShortcutSpec) -> xproto::ModMask {
    let mut m = xproto::ModMask::from(0u16);
    if spec.ctrl {
        m |= xproto::ModMask::CONTROL;
    }
    if spec.alt {
        m |= xproto::ModMask::M1;
    }
    if spec.shift {
        m |= xproto::ModMask::SHIFT;
    }
    if spec.super_key {
        m |= xproto::ModMask::M4;
    }
    m
}

/// Strip Lock (CapsLock) and Mod2 (NumLock) bits from an X11 key event state.
pub fn clean_state(state: xproto::KeyButMask) -> u16 {
    let mask = u16::from(state);
    let relevant = u16::from(
        xproto::ModMask::SHIFT
            | xproto::ModMask::CONTROL
            | xproto::ModMask::M1
            | xproto::ModMask::M4,
    );
    mask & relevant
}

/// Manages global `XGrabKey` registrations on the root window.
#[derive(Debug, Default)]
pub struct HotkeyManager {
    grabbed: Vec<GrabbedHotkey>,
}

impl HotkeyManager {
    pub fn ungrab_all(&mut self, conn: &RustConnection, root: xproto::Window) {
        for g in self.grabbed.drain(..) {
            for extra in lock_masks() {
                let _ = conn.ungrab_key(g.keycode, root, g.modifiers | extra);
            }
        }
        let _ = conn.flush();
    }

    /// Ungrab previous bindings and grab the new set of `(HotkeyAction, accel_string)` pairs.
    pub fn register(
        &mut self,
        conn: &RustConnection,
        root: xproto::Window,
        bindings: &[(HotkeyAction, String)],
    ) -> Vec<HotkeyStatus> {
        self.ungrab_all(conn, root);
        let mut statuses = Vec::with_capacity(bindings.len());

        for (action, accel) in bindings {
            let Some(spec) = ShortcutSpec::parse(accel) else {
                statuses.push(HotkeyStatus {
                    action: *action,
                    accel: accel.clone(),
                    grabbed: false,
                    error: Some("invalid shortcut syntax".to_string()),
                });
                continue;
            };
            let Some(keysym) = spec.x11_keysym() else {
                statuses.push(HotkeyStatus {
                    action: *action,
                    accel: accel.clone(),
                    grabbed: false,
                    error: Some(format!("unsupported key '{}'", spec.key)),
                });
                continue;
            };
            let keycode = match crate::keycode_for_keysym(conn, keysym) {
                Ok(Some(kc)) => kc,
                _ => {
                    statuses.push(HotkeyStatus {
                        action: *action,
                        accel: accel.clone(),
                        grabbed: false,
                        error: Some(format!("key '{}' not in current keyboard layout", spec.key)),
                    });
                    continue;
                }
            };

            let mods = spec_to_modmask(&spec);
            let mut acquired = Vec::new();
            let mut conflict = false;

            for extra in lock_masks() {
                let mask = mods | extra;
                match conn
                    .grab_key(
                        false,
                        root,
                        mask,
                        keycode,
                        xproto::GrabMode::ASYNC,
                        xproto::GrabMode::ASYNC,
                    )
                    .and_then(|c| Ok(c.check()))
                {
                    Ok(Ok(())) => acquired.push(mask),
                    _ => {
                        conflict = true;
                        break;
                    }
                }
            }

            if conflict {
                for mask in acquired {
                    let _ = conn.ungrab_key(keycode, root, mask);
                }
                warn!(accel = %accel, "global hotkey conflict (BadAccess)");
                statuses.push(HotkeyStatus {
                    action: *action,
                    accel: accel.clone(),
                    grabbed: false,
                    error: Some("shortcut already grabbed by another application".to_string()),
                });
            } else {
                self.grabbed.push(GrabbedHotkey {
                    action: *action,
                    keycode,
                    modifiers: mods,
                });
                info!(accel = %accel, keycode, "registered global hotkey");
                statuses.push(HotkeyStatus {
                    action: *action,
                    accel: accel.clone(),
                    grabbed: true,
                    error: None,
                });
            }
        }

        let _ = conn.flush();
        statuses
    }

    pub fn match_key_press(&self, ev: &xproto::KeyPressEvent) -> Option<HotkeyAction> {
        let ev_mods = clean_state(ev.state);
        self.grabbed
            .iter()
            .find(|g| g.keycode == ev.detail && u16::from(g.modifiers) == ev_mods)
            .map(|g| g.action)
    }
}

/// Query the current pointer `(root_x, root_y)` on the root window.
pub fn query_pointer(conn: &RustConnection, root: xproto::Window) -> Option<(i32, i32)> {
    let reply = conn.query_pointer(root).ok()?.reply().ok()?;
    Some((i32::from(reply.root_x), i32::from(reply.root_y)))
}

/// Position an undecorated GTK4 overlay window via its X11 XID (`USPosition` +
/// `_NET_WM_WINDOW_TYPE_UTILITY` + optional `_NET_WM_USER_TIME` + `ConfigureWindow`),
/// clamping `(x, y)` inside the root screen bounds.
pub fn position_overlay_xid(
    conn: &RustConnection,
    screen_num: usize,
    xid: u32,
    requested_x: i32,
    requested_y: i32,
    width: i32,
    height: i32,
    user_time: Option<u32>,
) -> Result<(i32, i32)> {
    let screen = &conn.setup().roots[screen_num];
    let sw = i32::from(screen.width_in_pixels);
    let sh = i32::from(screen.height_in_pixels);

    let clamped_x = requested_x.clamp(8, (sw - width - 8).max(8));
    let clamped_y = requested_y.clamp(8, (sh - height - 8).max(8));

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

    if let Some(ts) = user_time
        && ts != 0
        && let Some(ut) = conn
            .intern_atom(false, b"_NET_WM_USER_TIME")
            .ok()
            .and_then(|c| c.reply().ok())
    {
        let _ = conn.change_property32(
            PropMode::REPLACE,
            xid,
            ut.atom,
            AtomEnum::CARDINAL,
            &[ts],
        );
    }

    let mut hints = [0u32; 18];
    hints[0] = 1 | 4; // USPosition | PPosition
    hints[1] = clamped_x as u32;
    hints[2] = clamped_y as u32;
    let _ = conn.change_property32(
        PropMode::REPLACE,
        xid,
        AtomEnum::WM_NORMAL_HINTS,
        AtomEnum::WM_SIZE_HINTS,
        &hints,
    );
    let _ = conn.configure_window(
        xid,
        &xproto::ConfigureWindowAux::new().x(clamped_x).y(clamped_y),
    );
    conn.flush()?;
    Ok((clamped_x, clamped_y))
}

/// Request the window manager (Muffin/EWMH) to restore focus to `target_window`.
pub fn activate_window(conn: &RustConnection, root: xproto::Window, target_window: u32) -> Result<()> {
    if target_window == x11rb::NONE {
        return Ok(());
    }
    let atom = conn
        .intern_atom(false, b"_NET_ACTIVE_WINDOW")?
        .reply()?
        .atom;
    let ev = xproto::ClientMessageEvent::new(
        32,
        target_window,
        atom,
        [2, x11rb::CURRENT_TIME, 0, 0, 0],
    );
    conn.send_event(
        false,
        root,
        xproto::EventMask::SUBSTRUCTURE_REDIRECT | xproto::EventMask::SUBSTRUCTURE_NOTIFY,
        ev,
    )?;
    conn.flush()?;
    Ok(())
}

/// Wait up to `max_wait` for physical modifier keys (Ctrl/Shift/Alt/Super) to be released
/// before injecting an `XTEST` paste chord.
pub fn wait_for_modifiers_released(conn: &RustConnection, max_wait: Duration) {
    let mod_keysyms: &[u32] = &[
        0xffe1, // Shift_L
        0xffe2, // Shift_R
        0xffe3, // Control_L
        0xffe4, // Control_R
        0xffe9, // Alt_L
        0xffea, // Alt_R
        0xffeb, // Super_L
        0xffec, // Super_R
    ];
    let mod_keycodes: Vec<u8> = mod_keysyms
        .iter()
        .filter_map(|&ks| crate::keycode_for_keysym(conn, ks).ok().flatten())
        .collect();

    let deadline = Instant::now() + max_wait;
    while Instant::now() < deadline {
        let Some(km) = conn.query_keymap().ok().and_then(|c| c.reply().ok()) else {
            break;
        };
        let any_pressed = mod_keycodes.iter().any(|&kc| {
            let byte = (kc / 8) as usize;
            let bit = 1u8 << (kc % 8);
            km.keys.get(byte).map(|&b| (b & bit) != 0).unwrap_or(false)
        });
        if !any_pressed {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Inject `Ctrl+V` (or `Ctrl+Shift+V` when `use_shift` is true) using keycodes from the
/// active X11 keyboard layout.
pub fn inject_paste_chord(conn: &RustConnection, use_shift: bool) -> Result<()> {
    wait_for_modifiers_released(conn, Duration::from_millis(500));

    let ctrl = crate::keycode_for_keysym(conn, 0xffe3 /* Control_L */)?
        .ok_or_else(|| anyhow::anyhow!("Control_L keycode not found"))?;
    let v = crate::keycode_for_keysym(conn, 0x0076 /* v */)?
        .ok_or_else(|| anyhow::anyhow!("'v' keycode not found"))?;

    let mut mods = vec![ctrl];
    if use_shift {
        if let Some(shift) = crate::keycode_for_keysym(conn, 0xffe1 /* Shift_L */)? {
            mods.push(shift);
        }
    }
    debug!(use_shift, ctrl, v, "injecting XTEST paste chord");
    crate::inject_xtest_chord(conn, &mods, v)
}
