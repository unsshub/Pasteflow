use crate::hotkeys::{self, HotkeyManager, HotkeyStatus};
use anyhow::{Context, Result, bail};
use pf_core::backend::{ClipboardBackend, ClipboardEvent, HotkeyAction};
use pf_core::config::Config;
use pf_core::model::{
    RawClip, RawFormat, SelectionKind, is_url_or_filename_only, now_unix_ms,
};
use std::collections::HashMap;
use std::os::fd::{AsRawFd, RawFd};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xfixes::{self, ConnectionExt as XFixesExt};
use x11rb::protocol::xproto::{
    self, AtomEnum, ConnectionExt as XProtoExt, PropMode, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as WrapperExt;

const INCR_THRESHOLD: usize = 256 * 1024;
const INCR_CHUNK: usize = 256 * 1024;
const STEP_TIMEOUT: Duration = Duration::from_secs(1);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(5);
const EMPTY_GUARD_DELAY: Duration = Duration::from_millis(300);

struct Atoms {
    clipboard: xproto::Atom,
    primary: xproto::Atom,
    clipboard_manager: xproto::Atom,
    targets: xproto::Atom,
    timestamp: xproto::Atom,
    save_targets: xproto::Atom,
    incr: xproto::Atom,
    utf8_string: xproto::Atom,
    string: xproto::Atom,
    text: xproto::Atom,
    text_plain: xproto::Atom,
    text_plain_utf8: xproto::Atom,
    text_html: xproto::Atom,
    image_png: xproto::Atom,
    image_jpeg: xproto::Atom,
    image_webp: xproto::Atom,
    image_bmp: xproto::Atom,
    image_gif: xproto::Atom,
    gnome_copied_files: xproto::Atom,
    uri_list: xproto::Atom,
    kde_password_hint: xproto::Atom,
    pasteflow_origin: xproto::Atom,
    wm_client_leader: xproto::Atom,
    net_active_window: xproto::Atom,
    prop_read: xproto::Atom,
    prop_ts: xproto::Atom,
    prop_save: xproto::Atom,
}

impl Atoms {
    fn intern_all(conn: &RustConnection) -> Result<Self> {
        let intern = |name: &[u8]| -> Result<xproto::Atom> {
            Ok(conn.intern_atom(false, name)?.reply()?.atom)
        };
        Ok(Self {
            clipboard: intern(b"CLIPBOARD")?,
            primary: intern(b"PRIMARY")?,
            clipboard_manager: intern(b"CLIPBOARD_MANAGER")?,
            targets: intern(b"TARGETS")?,
            timestamp: intern(b"TIMESTAMP")?,
            save_targets: intern(b"SAVE_TARGETS")?,
            incr: intern(b"INCR")?,
            utf8_string: intern(b"UTF8_STRING")?,
            string: AtomEnum::STRING.into(),
            text: intern(b"TEXT")?,
            text_plain: intern(b"text/plain")?,
            text_plain_utf8: intern(b"text/plain;charset=utf-8")?,
            text_html: intern(b"text/html")?,
            image_png: intern(b"image/png")?,
            image_jpeg: intern(b"image/jpeg")?,
            image_webp: intern(b"image/webp")?,
            image_bmp: intern(b"image/bmp")?,
            image_gif: intern(b"image/gif")?,
            gnome_copied_files: intern(b"x-special/gnome-copied-files")?,
            uri_list: intern(b"text/uri-list")?,
            kde_password_hint: intern(b"x-kde-passwordManagerHint")?,
            pasteflow_origin: intern(b"application/x-pasteflow-origin")?,
            wm_client_leader: intern(b"WM_CLIENT_LEADER")?,
            net_active_window: intern(b"_NET_ACTIVE_WINDOW")?,
            prop_read: intern(b"_PASTEFLOW_READ")?,
            prop_ts: intern(b"_PASTEFLOW_TS")?,
            prop_save: intern(b"_PASTEFLOW_SAVE")?,
        })
    }
}

struct PendingIncr {
    requestor: xproto::Window,
    property: xproto::Atom,
    target: xproto::Atom,
    data: Vec<u8>,
    offset: usize,
}

/// Event-driven X11 clipboard engine (monitor, ICCCM/INCR reader, selection server,
/// owner identification, password-manager hint, SAVE_TARGETS handoff, empty-clipboard guard).
pub struct X11Engine {
    conn: RustConnection,
    screen_num: usize,
    root: xproto::Window,
    window: xproto::Window,
    atoms: Atoms,
    config: Config,
    paused: bool,
    owns_clipboard: bool,
    ownership_timestamp: xproto::Timestamp,
    served_formats: Vec<RawFormat>,
    last_captured_formats: Option<Vec<RawFormat>>,
    pending_incrs: HashMap<(xproto::Window, xproto::Atom), PendingIncr>,
    empty_guard_deadline: Option<Instant>,
    atom_name_cache: HashMap<xproto::Atom, String>,
    hotkeys: HotkeyManager,
}

impl X11Engine {
    /// Connect to X11 (`display = None` uses `$DISPLAY`), create the hidden window,
    /// and subscribe to XFixes selection notifications.
    pub fn connect(display: Option<&str>, config: Config) -> Result<Self> {
        let (conn, screen_num) =
            RustConnection::connect(display).context("connect to X11 display")?;

        let xfixes_ver = conn
            .xfixes_query_version(5, 0)?
            .reply()
            .context("query XFixes")?;
        if xfixes_ver.major_version < 1 {
            bail!("XFixes extension unavailable");
        }

        let root = conn.setup().roots[screen_num].root;
        let window = conn.generate_id()?;
        conn.create_window(
            0,
            window,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            0,
            &xproto::CreateWindowAux::new().event_mask(xproto::EventMask::PROPERTY_CHANGE),
        )?;

        let atoms = Atoms::intern_all(&conn)?;

        let mask = xfixes::SelectionEventMask::SET_SELECTION_OWNER
            | xfixes::SelectionEventMask::SELECTION_WINDOW_DESTROY
            | xfixes::SelectionEventMask::SELECTION_CLIENT_CLOSE;
        conn.xfixes_select_selection_input(window, atoms.clipboard, mask)?
            .check()
            .context("subscribe XFixes CLIPBOARD")?;

        if config.capture_primary {
            conn.xfixes_select_selection_input(window, atoms.primary, mask)?
                .check()
                .context("subscribe XFixes PRIMARY")?;
        }
        conn.flush()?;

        let paused = config.paused;
        info!(window = format_args!("{window:#x}"), "X11Engine initialized");

        Ok(Self {
            conn,
            screen_num,
            root,
            window,
            atoms,
            config,
            paused,
            owns_clipboard: false,
            ownership_timestamp: x11rb::CURRENT_TIME,
            served_formats: Vec::new(),
            last_captured_formats: None,
            pending_incrs: HashMap::new(),
            empty_guard_deadline: None,
            atom_name_cache: HashMap::new(),
            hotkeys: HotkeyManager::default(),
        })
    }

    /// Register global X11 key grabs for the configured shortcuts.
    pub fn register_hotkeys(&mut self, bindings: &[(HotkeyAction, String)]) -> Vec<HotkeyStatus> {
        self.hotkeys.register(&self.conn, self.root, bindings)
    }

    /// Release all global X11 key grabs.
    pub fn ungrab_hotkeys(&mut self) {
        self.hotkeys.ungrab_all(&self.conn, self.root);
    }

    /// Return the currently active X11 window (`_NET_ACTIVE_WINDOW`), if any.
    pub fn active_window(&self) -> Option<u32> {
        self.get_window_prop_u32(self.root, self.atoms.net_active_window)
            .filter(|&w| w != x11rb::NONE)
    }

    /// Return the current pointer `(x, y)` on the root screen.
    pub fn pointer_position(&self) -> Option<(i32, i32)> {
        hotkeys::query_pointer(&self.conn, self.root)
    }

    /// Position an overlay window via its XID, clamped to screen bounds.
    pub fn position_overlay_window(
        &self,
        xid: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        user_time: Option<u32>,
    ) -> Result<(i32, i32)> {
        hotkeys::position_overlay_xid(
            &self.conn,
            self.screen_num,
            xid,
            x,
            y,
            width,
            height,
            user_time,
        )
    }

    /// Request the window manager to restore focus to `target_window`.
    pub fn focus_window(&self, target_window: u32) -> Result<()> {
        hotkeys::activate_window(&self.conn, self.root, target_window)
    }

    /// Inject a synthetic paste chord (`Ctrl+V` or `Ctrl+Shift+V` for terminals)
    /// into the focused window (optionally checking `target_window`'s `WM_CLASS`).
    pub fn send_paste_chord(&self, target_window: Option<u32>) -> Result<()> {
        let win = target_window.or_else(|| self.active_window());
        let wm_class = win.and_then(|w| self.identify_owner_full(w).1);
        let use_shift = self.config.uses_shift_paste(wm_class.as_deref());
        hotkeys::inject_paste_chord(&self.conn, use_shift)
    }

    pub fn window(&self) -> xproto::Window {
        self.window
    }

    pub fn as_raw_fd(&self) -> RawFd {
        self.conn.stream().as_raw_fd()
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        info!(paused, "capture pause state updated");
    }

    pub fn update_config(&mut self, config: Config) {
        self.paused = config.paused;
        self.config = config;
    }

    pub fn owns_clipboard(&self) -> bool {
        self.owns_clipboard
    }

    /// Milliseconds until the next internal timer (e.g. empty-clipboard guard),
    /// or `-1` if no timer is armed (allowing infinite sleep in `poll(2)`).
    pub fn next_timeout_ms(&self) -> i32 {
        match self.empty_guard_deadline {
            Some(deadline) => {
                let rem = deadline.saturating_duration_since(Instant::now());
                i32::try_from(rem.as_millis().max(1)).unwrap_or(1000)
            }
            None => -1,
        }
    }

    /// Block efficiently on the X11 socket using `poll(2)` until an X11 event arrives,
    /// an internal timer fires, or `max_wait_ms` expires (`-1` = infinite).
    pub fn wait_readable(&self, max_wait_ms: i32) -> Result<bool> {
        let timeout = match (self.next_timeout_ms(), max_wait_ms) {
            (-1, m) => m,
            (t, -1) => t,
            (t, m) => t.min(m),
        };
        let mut pfd = libc::pollfd {
            fd: self.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `pfd` is a valid single-element pollfd pointing to our live X11 socket fd.
        let rc = unsafe { libc::poll(&mut pfd, 1, timeout) };
        if rc < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                return Ok(false);
            }
            return Err(err).context("poll X11 socket");
        }
        Ok(rc > 0)
    }

    /// Obtain a fresh X11 server timestamp by appending zero bytes to a private property.
    fn get_server_timestamp(&mut self) -> Result<xproto::Timestamp> {
        self.conn.change_property8(
            PropMode::APPEND,
            self.window,
            self.atoms.prop_ts,
            AtomEnum::STRING,
            &[],
        )?;
        self.conn.flush()?;

        let deadline = Instant::now() + STEP_TIMEOUT;
        let mut deferred = Vec::new();
        let mut result = x11rb::CURRENT_TIME;

        while Instant::now() < deadline {
            if let Some(ev) = self.conn.poll_for_event()? {
                match ev {
                    Event::PropertyNotify(pn)
                        if pn.window == self.window && pn.atom == self.atoms.prop_ts =>
                    {
                        result = pn.time;
                        break;
                    }
                    other => deferred.push(other),
                }
            } else {
                std::thread::sleep(Duration::from_millis(1));
            }
        }

        // Process any selection requests that arrived while waiting for the timestamp
        for ev in deferred {
            let _ = self.handle_service_event(ev);
        }
        Ok(result)
    }

    fn atom_name_cached(&mut self, atom: xproto::Atom) -> String {
        if let Some(name) = self.atom_name_cache.get(&atom) {
            return name.clone();
        }
        let name = crate::atom_name(&self.conn, atom);
        self.atom_name_cache.insert(atom, name.clone());
        name
    }

    /// Best-effort owner identification (§7.2 #6):
    /// 1. `_NET_WM_PID` -> `/proc/<pid>/comm` (and `/proc/<pid>/exe` basename)
    /// 2. `WM_CLASS` on owner or `WM_CLIENT_LEADER`
    /// 3. Fallback to `_NET_ACTIVE_WINDOW`
    fn identify_owner_full(&self, owner: xproto::Window) -> (Option<u32>, Option<String>) {
        if owner == x11rb::NONE {
            return (None, None);
        }
        let (mut pid, mut wm_class) = crate::identify_owner(&self.conn, owner);

        if wm_class.is_none() {
            if let Some(leader) = self.get_window_prop_u32(owner, self.atoms.wm_client_leader) {
                let (l_pid, l_class) = crate::identify_owner(&self.conn, leader);
                if pid.is_none() {
                    pid = l_pid;
                }
                wm_class = l_class;
            }
        }

        if pid.is_none() && wm_class.is_none() {
            if let Some(active) = self.get_window_prop_u32(self.root, self.atoms.net_active_window)
            {
                let (a_pid, a_class) = crate::identify_owner(&self.conn, active);
                pid = a_pid;
                wm_class = a_class;
            }
        }

        let proc_comm = pid.and_then(read_proc_comm);
        let source_app = match (wm_class, proc_comm) {
            (Some(c), Some(p)) if !c.to_ascii_lowercase().contains(&p.to_ascii_lowercase()) => {
                Some(format!("{c}/{p}"))
            }
            (Some(c), _) => Some(c),
            (None, Some(p)) => Some(p),
            (None, None) => None,
        };
        (pid, source_app)
    }

    fn get_window_prop_u32(&self, win: xproto::Window, prop: xproto::Atom) -> Option<u32> {
        self.conn
            .get_property(false, win, prop, AtomEnum::ANY, 0, 1)
            .ok()?
            .reply()
            .ok()?
            .value32()?
            .next()
    }

    /// Read a single target from `selection` with ICCCM direct + `INCR` support,
    /// enforcing `max_bytes`, `STEP_TIMEOUT`, and `overall_deadline`.
    fn read_target(
        &mut self,
        selection: xproto::Atom,
        target: xproto::Atom,
        max_bytes: usize,
        overall_deadline: Instant,
    ) -> Result<Option<Vec<u8>>> {
        let _ = self.conn.delete_property(self.window, self.atoms.prop_read);
        self.conn.convert_selection(
            self.window,
            selection,
            target,
            self.atoms.prop_read,
            x11rb::CURRENT_TIME,
        )?;
        self.conn.flush()?;

        let mut step_deadline = (Instant::now() + STEP_TIMEOUT).min(overall_deadline);
        let mut in_incr = false;
        let mut buf = Vec::new();

        loop {
            let now = Instant::now();
            if now >= step_deadline || now >= overall_deadline {
                debug!(target, "selection target read timed out");
                return Ok(None);
            }

            if let Some(ev) = self.conn.poll_for_event()? {
                match ev {
                    Event::SelectionNotify(sn)
                        if sn.requestor == self.window
                            && sn.selection == selection
                            && sn.target == target =>
                    {
                        if sn.property == x11rb::NONE {
                            return Ok(None);
                        }
                        let reply = self
                            .conn
                            .get_property(
                                true,
                                self.window,
                                sn.property,
                                AtomEnum::ANY,
                                0,
                                u32::MAX / 4,
                            )?
                            .reply()?;
                        if reply.type_ == self.atoms.incr {
                            in_incr = true;
                            step_deadline = (Instant::now() + STEP_TIMEOUT).min(overall_deadline);
                            self.conn.flush()?;
                        } else {
                            if reply.value.len() > max_bytes {
                                warn!(
                                    bytes = reply.value.len(),
                                    max_bytes,
                                    "clipboard payload exceeds cap; aborting"
                                );
                                return Ok(None);
                            }
                            return Ok(Some(reply.value));
                        }
                    }
                    Event::PropertyNotify(pn)
                        if in_incr
                            && pn.window == self.window
                            && pn.atom == self.atoms.prop_read
                            && pn.state == xproto::Property::NEW_VALUE =>
                    {
                        let reply = self
                            .conn
                            .get_property(
                                true,
                                self.window,
                                self.atoms.prop_read,
                                AtomEnum::ANY,
                                0,
                                u32::MAX / 4,
                            )?
                            .reply()?;
                        step_deadline = (Instant::now() + STEP_TIMEOUT).min(overall_deadline);
                        if reply.value.is_empty() {
                            return Ok(Some(buf));
                        }
                        if buf.len() + reply.value.len() > max_bytes {
                            warn!(
                                bytes = buf.len() + reply.value.len(),
                                max_bytes,
                                "INCR payload exceeds cap; aborting"
                            );
                            return Ok(None);
                        }
                        buf.extend_from_slice(&reply.value);
                        self.conn.flush()?;
                    }
                    other => {
                        // Keep servicing outgoing selection requests while reading
                        let _ = self.handle_service_event(other);
                    }
                }
            } else {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }

    fn read_targets_list(
        &mut self,
        selection: xproto::Atom,
        overall_deadline: Instant,
    ) -> Result<Option<Vec<xproto::Atom>>> {
        let Some(raw) = self.read_target(selection, self.atoms.targets, 64 * 1024, overall_deadline)?
        else {
            return Ok(None);
        };
        let mut atoms = Vec::with_capacity(raw.len() / 4);
        for chunk in raw.chunks_exact(4) {
            atoms.push(u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
        }
        Ok(Some(atoms))
    }

    fn capture_selection(
        &mut self,
        selection_atom: xproto::Atom,
        owner: xproto::Window,
    ) -> Result<Option<RawClip>> {
        let selection_kind = if selection_atom == self.atoms.primary {
            SelectionKind::Primary
        } else {
            SelectionKind::Clipboard
        };

        let (owner_pid, source_app) = self.identify_owner_full(owner);
        if self.config.is_excluded(source_app.as_deref()) {
            info!(
                source_app = source_app.as_deref().unwrap_or("unknown"),
                "skipping excluded application"
            );
            return Ok(None);
        }

        let overall_deadline = Instant::now() + TOTAL_TIMEOUT;
        let Some(targets) = self.read_targets_list(selection_atom, overall_deadline)? else {
            return Ok(None);
        };

        // Never re-capture our own origin marker
        if targets.contains(&self.atoms.pasteflow_origin) {
            debug!("skipping selection with application/x-pasteflow-origin");
            return Ok(None);
        }

        // Password-manager hint check (§7.3 #1): read hint BEFORE any data target!
        if targets.contains(&self.atoms.kde_password_hint) {
            if let Some(hint_bytes) = self.read_target(
                selection_atom,
                self.atoms.kde_password_hint,
                1024,
                overall_deadline,
            )? {
                let hint = String::from_utf8_lossy(&hint_bytes);
                if hint.trim().eq_ignore_ascii_case("secret") {
                    info!(
                        source_app = source_app.as_deref().unwrap_or("unknown"),
                        "skipping selection marked x-kde-passwordManagerHint=secret"
                    );
                    return Ok(None);
                }
            }
        }

        let mut formats = Vec::new();

        // Files (§7.3 #3)
        let has_files = targets.contains(&self.atoms.gnome_copied_files)
            || targets.contains(&self.atoms.uri_list);
        if has_files {
            if targets.contains(&self.atoms.gnome_copied_files) {
                if let Some(data) = self.read_target(
                    selection_atom,
                    self.atoms.gnome_copied_files,
                    self.config.max_text_bytes,
                    overall_deadline,
                )? {
                    formats.push(RawFormat {
                        mime: "x-special/gnome-copied-files".to_string(),
                        data,
                    });
                }
            }
            if targets.contains(&self.atoms.uri_list) {
                if let Some(data) = self.read_target(
                    selection_atom,
                    self.atoms.uri_list,
                    self.config.max_text_bytes,
                    overall_deadline,
                )? {
                    formats.push(RawFormat {
                        mime: "text/uri-list".to_string(),
                        data,
                    });
                }
            }
        } else {
            // Text (§7.3 #5)
            let text_candidates = [
                (self.atoms.utf8_string, "UTF8_STRING"),
                (self.atoms.text_plain_utf8, "text/plain;charset=utf-8"),
                (self.atoms.text_plain, "text/plain"),
                (self.atoms.string, "STRING"),
            ];
            let mut captured_text: Option<String> = None;
            for (atom, mime) in text_candidates {
                if targets.contains(&atom) {
                    if let Some(data) = self.read_target(
                        selection_atom,
                        atom,
                        self.config.max_text_bytes,
                        overall_deadline,
                    )? {
                        captured_text = Some(if mime == "STRING" {
                            data.iter().map(|&b| char::from(b)).collect()
                        } else {
                            String::from_utf8_lossy(&data).into_owned()
                        });
                        formats.push(RawFormat {
                            mime: mime.to_string(),
                            data,
                        });
                        break;
                    }
                }
            }

            if captured_text.is_some() && targets.contains(&self.atoms.text_html) {
                if let Some(html) = self.read_target(
                    selection_atom,
                    self.atoms.text_html,
                    self.config.max_html_bytes,
                    overall_deadline,
                )? {
                    formats.push(RawFormat {
                        mime: "text/html".to_string(),
                        data: html,
                    });
                }
            }

            // Image arbitration (§7.3 #4): only read image if no text, or text is empty/URL/filename
            let should_read_image = self.config.capture_images
                && match &captured_text {
                    None => true,
                    Some(t) => is_url_or_filename_only(t),
                };

            if should_read_image {
                let image_candidates = [
                    (self.atoms.image_png, "image/png"),
                    (self.atoms.image_jpeg, "image/jpeg"),
                    (self.atoms.image_webp, "image/webp"),
                    (self.atoms.image_bmp, "image/bmp"),
                    (self.atoms.image_gif, "image/gif"),
                ];
                for (atom, mime) in image_candidates {
                    if targets.contains(&atom) {
                        if let Some(data) = self.read_target(
                            selection_atom,
                            atom,
                            self.config.max_image_bytes,
                            overall_deadline,
                        )? {
                            formats.push(RawFormat {
                                mime: mime.to_string(),
                                data,
                            });
                            break;
                        }
                    }
                }
            }
        }

        if formats.is_empty() {
            return Ok(None);
        }

        if selection_kind == SelectionKind::Clipboard {
            self.last_captured_formats = Some(formats.clone());
        }

        Ok(Some(RawClip {
            selection: selection_kind,
            formats,
            source_app,
            owner_pid,
            captured_at_ms: now_unix_ms(),
        }))
    }

    fn handle_service_event(&mut self, event: Event) -> Result<Option<ClipboardEvent>> {
        match event {
            Event::SelectionRequest(req) => self.handle_selection_request(req),
            Event::PropertyNotify(ev) if ev.state == xproto::Property::DELETE => {
                self.advance_incr(ev.window, ev.atom)?;
                Ok(None)
            }
            Event::SelectionClear(ev) => {
                if ev.selection == self.atoms.clipboard {
                    self.owns_clipboard = false;
                    debug!("lost CLIPBOARD ownership");
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    fn handle_selection_request(
        &mut self,
        req: xproto::SelectionRequestEvent,
    ) -> Result<Option<ClipboardEvent>> {
        let prop = if req.property == x11rb::NONE {
            req.target
        } else {
            req.property
        };
        let mut notify_prop = prop;
        let mut served_target: Option<String> = None;

        if req.target == self.atoms.targets {
            let target_atoms = self.advertised_target_atoms()?;
            self.conn.change_property32(
                PropMode::REPLACE,
                req.requestor,
                prop,
                AtomEnum::ATOM,
                &target_atoms,
            )?;
        } else if req.target == self.atoms.timestamp {
            self.conn.change_property32(
                PropMode::REPLACE,
                req.requestor,
                prop,
                AtomEnum::INTEGER,
                &[self.ownership_timestamp],
            )?;
        } else if req.target == self.atoms.pasteflow_origin {
            self.conn.change_property8(
                PropMode::REPLACE,
                req.requestor,
                prop,
                req.target,
                b"1",
            )?;
        } else if let Some(data) = self.resolve_served_payload(req.target)? {
            let target_name = self.atom_name_cached(req.target);
            if data.len() > INCR_THRESHOLD {
                let _ = self.conn.change_window_attributes(
                    req.requestor,
                    &xproto::ChangeWindowAttributesAux::new()
                        .event_mask(xproto::EventMask::PROPERTY_CHANGE),
                );
                let size_u32 = u32::try_from(data.len()).unwrap_or(u32::MAX);
                self.conn.change_property32(
                    PropMode::REPLACE,
                    req.requestor,
                    prop,
                    self.atoms.incr,
                    &[size_u32],
                )?;
                self.pending_incrs.insert(
                    (req.requestor, prop),
                    PendingIncr {
                        requestor: req.requestor,
                        property: prop,
                        target: req.target,
                        data,
                        offset: 0,
                    },
                );
            } else {
                self.conn.change_property8(
                    PropMode::REPLACE,
                    req.requestor,
                    prop,
                    req.target,
                    &data,
                )?;
            }
            served_target = Some(target_name);
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
        self.conn
            .send_event(false, req.requestor, xproto::EventMask::NO_EVENT, notify)?;
        self.conn.flush()?;

        let is_clipboard_manager = if served_target.is_some() {
            let cm_owner = self
                .conn
                .get_selection_owner(self.atoms.clipboard_manager)
                .ok()
                .and_then(|c| c.reply().ok())
                .map(|r| r.owner)
                .unwrap_or(x11rb::NONE);
            req.requestor == self.window
                || (cm_owner != x11rb::NONE && req.requestor == cm_owner)
        } else {
            false
        };

        Ok(served_target.map(|target| ClipboardEvent::SelectionServed {
            target,
            is_clipboard_manager,
        }))
    }

    fn advertised_target_atoms(&mut self) -> Result<Vec<xproto::Atom>> {
        let mut out = vec![
            self.atoms.targets,
            self.atoms.timestamp,
            self.atoms.pasteflow_origin,
        ];
        let mut has_text = false;
        for fmt in &self.served_formats {
            if is_text_mime(&fmt.mime) {
                has_text = true;
            }
            let atom = self
                .conn
                .intern_atom(false, fmt.mime.as_bytes())?
                .reply()?
                .atom;
            if !out.contains(&atom) {
                out.push(atom);
            }
        }
        if has_text {
            for alias in [
                self.atoms.utf8_string,
                self.atoms.text_plain_utf8,
                self.atoms.text_plain,
                self.atoms.string,
                self.atoms.text,
            ] {
                if !out.contains(&alias) {
                    out.push(alias);
                }
            }
        }
        Ok(out)
    }

    fn resolve_served_payload(&mut self, target: xproto::Atom) -> Result<Option<Vec<u8>>> {
        let target_name = self.atom_name_cached(target);
        if let Some(fmt) = self.served_formats.iter().find(|f| f.mime == target_name) {
            return Ok(Some(fmt.data.clone()));
        }
        // Serve text aliases from stored UTF-8 text
        if target == self.atoms.utf8_string
            || target == self.atoms.text_plain_utf8
            || target == self.atoms.text_plain
            || target == self.atoms.text
            || target == self.atoms.string
        {
            if let Some(fmt) = self.served_formats.iter().find(|f| is_text_mime(&f.mime)) {
                return Ok(Some(fmt.data.clone()));
            }
        }
        Ok(None)
    }

    fn advance_incr(&mut self, window: xproto::Window, prop: xproto::Atom) -> Result<()> {
        let Some(incr) = self.pending_incrs.get_mut(&(window, prop)) else {
            return Ok(());
        };
        if incr.offset < incr.data.len() {
            let end = (incr.offset + INCR_CHUNK).min(incr.data.len());
            self.conn.change_property8(
                PropMode::REPLACE,
                incr.requestor,
                incr.property,
                incr.target,
                &incr.data[incr.offset..end],
            )?;
            incr.offset = end;
        } else {
            self.conn.change_property8(
                PropMode::REPLACE,
                incr.requestor,
                incr.property,
                incr.target,
                &[],
            )?;
            self.pending_incrs.remove(&(window, prop));
        }
        self.conn.flush()?;
        Ok(())
    }

    fn check_empty_guard(&mut self) -> Result<()> {
        let Some(deadline) = self.empty_guard_deadline else {
            return Ok(());
        };
        if Instant::now() < deadline {
            return Ok(());
        }
        self.empty_guard_deadline = None;
        let current_owner = self
            .conn
            .get_selection_owner(self.atoms.clipboard)?
            .reply()?
            .owner;
        if current_owner == x11rb::NONE {
            if let Some(formats) = self.last_captured_formats.clone() {
                info!("empty-clipboard guard taking ownership of last captured clip");
                self.own_clipboard(formats)?;
            }
        }
        Ok(())
    }
}

impl ClipboardBackend for X11Engine {
    fn poll_events(&mut self) -> Result<Vec<ClipboardEvent>> {
        let mut emitted = Vec::new();

        while let Some(ev) = self.conn.poll_for_event()? {
            match ev {
                Event::XfixesSelectionNotify(sev) => {
                    if sev.selection != self.atoms.clipboard
                        && (!self.config.capture_primary || sev.selection != self.atoms.primary)
                    {
                        continue;
                    }
                    if sev.owner == self.window {
                        continue;
                    }

                    let selection_kind = if sev.selection == self.atoms.primary {
                        SelectionKind::Primary
                    } else {
                        SelectionKind::Clipboard
                    };

                    let vanished = sev.owner == x11rb::NONE
                        || sev.subtype == xfixes::SelectionEvent::SELECTION_WINDOW_DESTROY
                        || sev.subtype == xfixes::SelectionEvent::SELECTION_CLIENT_CLOSE;

                    if vanished {
                        emitted.push(ClipboardEvent::OwnerVanished {
                            selection: selection_kind,
                        });
                        if self.config.empty_clipboard_guard
                            && selection_kind == SelectionKind::Clipboard
                            && self.last_captured_formats.is_some()
                        {
                            self.empty_guard_deadline = Some(Instant::now() + EMPTY_GUARD_DELAY);
                        }
                        continue;
                    }

                    self.empty_guard_deadline = None;
                    if self.paused {
                        debug!("capture paused; ignoring selection change");
                        continue;
                    }

                    match self.capture_selection(sev.selection, sev.owner) {
                        Ok(Some(raw)) => emitted.push(ClipboardEvent::Captured(raw)),
                        Ok(None) => {}
                        Err(e) => warn!(error = %e, "failed to capture selection"),
                    }
                }
                Event::KeyPress(kp) => {
                    if let Some(action) = self.hotkeys.match_key_press(&kp) {
                        let prev = self.active_window();
                        emitted.push(ClipboardEvent::HotkeyPressed {
                            action,
                            timestamp: kp.time,
                            previous_window: prev,
                        });
                    }
                }
                other => {
                    if let Some(ce) = self.handle_service_event(other)? {
                        emitted.push(ce);
                    }
                }
            }
        }

        self.check_empty_guard()?;
        Ok(emitted)
    }

    fn own_clipboard(&mut self, formats: Vec<RawFormat>) -> Result<()> {
        let ts = self.get_server_timestamp()?;
        self.served_formats = formats;
        self.ownership_timestamp = ts;
        self.conn
            .set_selection_owner(self.window, self.atoms.clipboard, ts)?;
        self.conn.flush()?;
        self.owns_clipboard = true;
        self.empty_guard_deadline = None;
        debug!(formats = self.served_formats.len(), "claimed CLIPBOARD ownership");
        Ok(())
    }

    fn handoff_on_exit(&mut self) -> Result<()> {
        if !self.owns_clipboard {
            return Ok(());
        }
        let cm_owner = self
            .conn
            .get_selection_owner(self.atoms.clipboard_manager)?
            .reply()?
            .owner;
        if cm_owner == x11rb::NONE {
            debug!("no CLIPBOARD_MANAGER owner present; skipping SAVE_TARGETS handoff");
            return Ok(());
        }

        info!(
            cm_owner = format_args!("{cm_owner:#x}"),
            "handing off clipboard via SAVE_TARGETS"
        );
        let ts = self.get_server_timestamp()?;
        self.conn.convert_selection(
            self.window,
            self.atoms.clipboard_manager,
            self.atoms.save_targets,
            self.atoms.prop_save,
            ts,
        )?;
        self.conn.flush()?;

        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            if let Some(ev) = self.conn.poll_for_event()? {
                match ev {
                    Event::SelectionNotify(sn)
                        if sn.requestor == self.window
                            && sn.selection == self.atoms.clipboard_manager =>
                    {
                        debug!("SAVE_TARGETS handoff completed");
                        break;
                    }
                    other => {
                        let _ = self.handle_service_event(other);
                    }
                }
            } else {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        Ok(())
    }
}

fn is_text_mime(mime: &str) -> bool {
    matches!(
        mime,
        "UTF8_STRING" | "text/plain;charset=utf-8" | "text/plain" | "STRING" | "TEXT"
    )
}

fn read_proc_comm(pid: u32) -> Option<String> {
    if let Ok(comm) = std::fs::read_to_string(format!("/proc/{pid}/comm")) {
        let trimmed = comm.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    if let Ok(exe) = std::fs::read_link(format!("/proc/{pid}/exe")) {
        if let Some(name) = exe.file_name() {
            return Some(name.to_string_lossy().into_owned());
        }
    }
    None
}
