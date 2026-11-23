//! M0 X11 feasibility tests (S2: 20 MB INCR, S3: selection serving, S4: XGrabKey + XTEST).

use pf_testkit::TestSelectionOwner;
use pf_x11::{inject_xtest_chord, keycode_for_keysym, read_selection_target, test_global_grab};
use std::collections::HashMap;
use std::time::Duration;
use x11rb::connection::Connection;
use x11rb::protocol::xproto;
use x11rb::rust_connection::RustConnection;

#[test]
fn s2_incr_20mb_roundtrip_matches_blake3() {
    let payload: Vec<u8> = (0..(20 * 1024 * 1024)).map(|i| (i % 251) as u8).collect();
    let expected_hash = blake3::hash(&payload);

    let mut map = HashMap::new();
    map.insert("UTF8_STRING".to_string(), payload);

    let sel = "_PASTEFLOW_M0_INCR_TEST";
    let _owner = TestSelectionOwner::spawn(None, sel, map).expect("spawn test owner");

    let (conn, screen_num) = RustConnection::connect(None).expect("connect X11");
    let got = read_selection_target(
        &conn,
        screen_num,
        sel,
        "UTF8_STRING",
        25 * 1024 * 1024,
        Duration::from_secs(15),
    )
    .expect("read 20MB INCR");

    assert_eq!(got.len(), 20 * 1024 * 1024);
    assert_eq!(blake3::hash(&got), expected_hash);
}

#[test]
fn s4_super_v_and_super_shift_v_grab_succeeds_without_conflict() {
    let (conn, screen_num) = RustConnection::connect(None).expect("connect X11");
    let root = conn.setup().roots[screen_num].root;
    let v_keycode = keycode_for_keysym(&conn, 0x0076 /* XK_v */)
        .expect("query keymap")
        .expect("keycode for v");

    let ok_super_v = test_global_grab(&conn, root, v_keycode, xproto::ModMask::M4)
        .expect("test Super+V grab");
    let ok_super_shift_v = test_global_grab(
        &conn,
        root,
        v_keycode,
        xproto::ModMask::M4 | xproto::ModMask::SHIFT,
    )
    .expect("test Super+Shift+V grab");

    assert!(ok_super_v, "Super+V should be free to grab");
    assert!(ok_super_shift_v, "Super+Shift+V should be free to grab");
}

#[test]
fn s4_xtest_injects_ctrl_v_without_error() {
    // Only run XTEST key injection when explicitly pointed at an isolated Xvfb display
    if std::env::var("PASTEFLOW_XVFB_ACTIVE").ok().as_deref() != Some("1") {
        return;
    }
    let (conn, _screen_num) = RustConnection::connect(None).expect("connect Xvfb");
    let ctrl = keycode_for_keysym(&conn, 0xffe3 /* Control_L */)
        .unwrap()
        .expect("Control_L");
    let v = keycode_for_keysym(&conn, 0x0076 /* v */)
        .unwrap()
        .expect("v");
    inject_xtest_chord(&conn, &[ctrl], v).expect("inject Ctrl+V via XTEST");
}
