//! Test-only X11 selection client for automated PasteFlow integration tests.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    self, AtomEnum, ConnectionExt as XProtoExt, PropMode, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as WrapperExt;

const INCR_THRESHOLD: usize = 256 * 1024;
const INCR_CHUNK: usize = 256 * 1024;

struct PendingIncr {
    requestor: xproto::Window,
    property: xproto::Atom,
    target: xproto::Atom,
    data: Vec<u8>,
    offset: usize,
}

/// Optional metadata attached to a `TestSelectionOwner` window for integration testing.
#[derive(Debug, Clone, Default)]
pub struct TestOwnerOptions {
    /// Sets `WM_CLASS` (e.g. `("bitwarden-app", "Bitwarden")`).
    pub wm_class: Option<(String, String)>,
    /// Sets `_NET_WM_PID`.
    pub pid: Option<u32>,
}

/// Spawns a background X11 client that owns `selection_name` and serves the given MIME payloads
/// (automatically using ICCCM `INCR` when payload length exceeds 256 KiB).
pub struct TestSelectionOwner {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<Result<()>>>,
    pub window: xproto::Window,
}

impl TestSelectionOwner {
    pub fn spawn(
        display: Option<&str>,
        selection_name: &str,
        payloads: HashMap<String, Vec<u8>>,
    ) -> Result<Self> {
        Self::spawn_with_options(display, selection_name, payloads, TestOwnerOptions::default())
    }

    pub fn spawn_with_options(
        display: Option<&str>,
        selection_name: &str,
        payloads: HashMap<String, Vec<u8>>,
        options: TestOwnerOptions,
    ) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_bg = Arc::clone(&stop);
        let display_owned = display.map(str::to_string);
        let sel_name = selection_name.to_string();

        let (tx, rx) = std::sync::mpsc::channel::<Result<xproto::Window>>();

        let handle = thread::spawn(move || {
            let (conn, screen_num) = match RustConnection::connect(display_owned.as_deref()) {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(Err(anyhow::anyhow!("connect X11: {e}")));
                    return Ok(());
                }
            };
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

            if let Some((instance, class)) = options.wm_class {
                let mut val = Vec::new();
                val.extend_from_slice(instance.as_bytes());
                val.push(0);
                val.extend_from_slice(class.as_bytes());
                val.push(0);
                conn.change_property8(
                    PropMode::REPLACE,
                    win,
                    AtomEnum::WM_CLASS,
                    AtomEnum::STRING,
                    &val,
                )?;
            }

            if let Some(pid) = options.pid {
                let pid_atom = conn.intern_atom(false, b"_NET_WM_PID")?.reply()?.atom;
                conn.change_property32(
                    PropMode::REPLACE,
                    win,
                    pid_atom,
                    AtomEnum::CARDINAL,
                    &[pid],
                )?;
            }

            let sel_atom = conn.intern_atom(false, sel_name.as_bytes())?.reply()?.atom;
            let targets_atom = conn.intern_atom(false, b"TARGETS")?.reply()?.atom;
            let incr_atom = conn.intern_atom(false, b"INCR")?.reply()?.atom;

            let mut target_map: HashMap<xproto::Atom, Vec<u8>> = HashMap::new();
            let mut target_atoms = vec![targets_atom];
            for (mime, data) in payloads {
                let atom = conn.intern_atom(false, mime.as_bytes())?.reply()?.atom;
                target_atoms.push(atom);
                target_map.insert(atom, data);
            }

            conn.set_selection_owner(win, sel_atom, x11rb::CURRENT_TIME)?;
            conn.flush()?;
            let _ = tx.send(Ok(win));

            let mut pending_incrs: HashMap<(xproto::Window, xproto::Atom), PendingIncr> =
                HashMap::new();

            while !stop_bg.load(Ordering::Relaxed) {
                while let Some(event) = conn.poll_for_event()? {
                    match event {
                        Event::SelectionRequest(req) => {
                            let prop = if req.property == x11rb::NONE {
                                req.target
                            } else {
                                req.property
                            };
                            let mut notify_prop = prop;

                            if req.target == targets_atom {
                                conn.change_property32(
                                    PropMode::REPLACE,
                                    req.requestor,
                                    prop,
                                    AtomEnum::ATOM,
                                    &target_atoms,
                                )?;
                            } else if let Some(data) = target_map.get(&req.target) {
                                if data.len() > INCR_THRESHOLD {
                                    // Watch requestor property deletion for INCR handshaking
                                    let _ = conn.change_window_attributes(
                                        req.requestor,
                                        &xproto::ChangeWindowAttributesAux::new()
                                            .event_mask(xproto::EventMask::PROPERTY_CHANGE),
                                    );
                                    let size_u32 = u32::try_from(data.len()).unwrap_or(u32::MAX);
                                    conn.change_property32(
                                        PropMode::REPLACE,
                                        req.requestor,
                                        prop,
                                        incr_atom,
                                        &[size_u32],
                                    )?;
                                    pending_incrs.insert(
                                        (req.requestor, prop),
                                        PendingIncr {
                                            requestor: req.requestor,
                                            property: prop,
                                            target: req.target,
                                            data: data.clone(),
                                            offset: 0,
                                        },
                                    );
                                } else {
                                    conn.change_property8(
                                        PropMode::REPLACE,
                                        req.requestor,
                                        prop,
                                        req.target,
                                        data,
                                    )?;
                                }
                            } else {
                                notify_prop = x11rb::NONE;
                            }

                            let notify = xproto::SelectionNotifyEvent {
                                response_type: xproto::SELECTION_NOTIFY_EVENT,
                                sequence: 0,
                                time: req.time,
                                requestor: req.requestor,
                                selection: req.selection,
                                target: req.target,
                                property: notify_prop,
                            };
                            conn.send_event(
                                false,
                                req.requestor,
                                xproto::EventMask::NO_EVENT,
                                notify,
                            )?;
                            conn.flush()?;
                        }
                        Event::PropertyNotify(ev)
                            if ev.state == xproto::Property::DELETE =>
                        {
                            if let Some(incr) = pending_incrs.get_mut(&(ev.window, ev.atom)) {
                                if incr.offset < incr.data.len() {
                                    let end = (incr.offset + INCR_CHUNK).min(incr.data.len());
                                    conn.change_property8(
                                        PropMode::REPLACE,
                                        incr.requestor,
                                        incr.property,
                                        incr.target,
                                        &incr.data[incr.offset..end],
                                    )?;
                                    incr.offset = end;
                                } else {
                                    // Zero-length property signals end of INCR transfer
                                    conn.change_property8(
                                        PropMode::REPLACE,
                                        incr.requestor,
                                        incr.property,
                                        incr.target,
                                        &[],
                                    )?;
                                    pending_incrs.remove(&(ev.window, ev.atom));
                                }
                                conn.flush()?;
                            }
                        }
                        Event::SelectionClear(_) => break,
                        _ => {}
                    }
                }
                thread::sleep(std::time::Duration::from_millis(1));
            }
            Ok(())
        });

        let window = rx.recv().context("wait for TestSelectionOwner window")??;
        Ok(Self {
            stop,
            handle: Some(handle),
            window,
        })
    }
}

impl Drop for TestSelectionOwner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}
