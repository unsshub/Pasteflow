//! M3 Global hotkey grab, conflict detection, and XTEST trigger tests.
//! Run under Xvfb:
//!   Xvfb :99 -screen 0 1280x720x24 &
//!   DISPLAY=:99 PASTEFLOW_XVFB_ACTIVE=1 cargo test -p pf-x11 --test m3_hotkeys -- --test-threads=1

use pf_core::backend::{ClipboardBackend, ClipboardEvent, HotkeyAction};
use pf_core::config::Config;
use pf_x11::{X11Engine, inject_xtest_chord, keycode_for_keysym};
use std::time::{Duration, Instant};
use x11rb::rust_connection::RustConnection;

fn xvfb_active() -> bool {
    std::env::var("PASTEFLOW_XVFB_ACTIVE").ok().as_deref() == Some("1")
}

#[test]
fn m3_global_hotkey_grab_conflict_and_trigger() {
    if !xvfb_active() {
        return;
    }

    let cfg = Config::default();
    let mut eng1 = X11Engine::connect(None, cfg.clone()).unwrap();
    let bindings = vec![
        (HotkeyAction::ToggleOverlay, "<Super>v".to_string()),
        (HotkeyAction::PasteNext, "<Super><Shift>v".to_string()),
    ];

    let s1 = eng1.register_hotkeys(&bindings);
    assert_eq!(s1.len(), 2);
    assert!(s1[0].grabbed, "eng1 should grab Super+V: {:?}", s1[0]);
    assert!(s1[1].grabbed, "eng1 should grab Super+Shift+V: {:?}", s1[1]);

    // A second client grabbing the same shortcut must report conflict (BadAccess)
    let mut eng2 = X11Engine::connect(None, cfg).unwrap();
    let s2 = eng2.register_hotkeys(&[(HotkeyAction::ToggleOverlay, "<Super>v".to_string())]);
    assert_eq!(s2.len(), 1);
    assert!(!s2[0].grabbed, "eng2 should detect Super+V conflict");
    assert!(s2[0].error.is_some());

    // Inject Super+V via XTEST and verify eng1 receives HotkeyPressed(ToggleOverlay)
    let (conn, _screen) = RustConnection::connect(None).unwrap();
    let super_l = keycode_for_keysym(&conn, 0xffeb /* Super_L */)
        .unwrap()
        .expect("Super_L");
    let v = keycode_for_keysym(&conn, 0x0076 /* v */)
        .unwrap()
        .expect("v");
    inject_xtest_chord(&conn, &[super_l], v).unwrap();

    let deadline = Instant::now() + Duration::from_secs(2);
    let mut triggered = false;
    while Instant::now() < deadline && !triggered {
        let _ = eng1.wait_readable(25);
        for ev in eng1.poll_events().unwrap() {
            if matches!(
                ev,
                ClipboardEvent::HotkeyPressed {
                    action: HotkeyAction::ToggleOverlay,
                    ..
                }
            ) {
                triggered = true;
            }
        }
    }
    assert!(triggered, "XTEST Super+V should fire ToggleOverlay hotkey");

    eng1.ungrab_hotkeys();
}
