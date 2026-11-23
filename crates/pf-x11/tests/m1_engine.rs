//! M1 X11 clipboard engine + SQLite store integration tests.
//! Run in isolation under Xvfb:
//!   Xvfb :99 -screen 0 1280x720x24 &
//!   DISPLAY=:99 PASTEFLOW_XVFB_ACTIVE=1 cargo test -p pf-x11 --test m1_engine -- --test-threads=1

use image::{ImageBuffer, Rgba};
use pf_core::backend::{ClipboardBackend, ClipboardEvent};
use pf_core::config::Config;
use pf_core::model::{ClipFilter, ClipKind};
use pf_store::{InsertOutcome, Store};
use pf_testkit::{TestOwnerOptions, TestSelectionOwner};
use pf_x11::{X11Engine, read_selection_target};
use std::collections::HashMap;
use std::io::Cursor;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use x11rb::rust_connection::RustConnection;

fn xvfb_active() -> bool {
    std::env::var("PASTEFLOW_XVFB_ACTIVE").ok().as_deref() == Some("1")
}

fn pump_until_capture(
    engine: &mut X11Engine,
    timeout: Duration,
) -> Option<pf_core::model::RawClip> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let _ = engine.wait_readable(25);
        for ev in engine.poll_events().expect("poll_events") {
            if let ClipboardEvent::Captured(raw) = ev {
                return Some(raw);
            }
        }
    }
    None
}

fn make_png(w: u32, h: u32) -> Vec<u8> {
    let img = ImageBuffer::from_fn(w, h, |x, y| {
        Rgba([(x % 256) as u8, (y % 256) as u8, 200, 255])
    });
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

#[test]
fn m1_text_multiline_large_incr_image_and_persistence() {
    if !xvfb_active() {
        return;
    }

    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("pasteflow.db");
    let blobs_dir = dir.path().join("blobs");
    let thumbs_dir = dir.path().join("thumbs");

    let mut cfg = Config::default();
    cfg.max_text_bytes = 6 * 1024 * 1024; // 6 MiB cap so 5 MiB passes and 7 MiB is skipped

    let mut store = Store::open_at(&db_path, blobs_dir.clone(), thumbs_dir.clone()).unwrap();
    let mut engine = X11Engine::connect(None, cfg.clone()).unwrap();

    // #1 Plain text
    {
        let mut map = HashMap::new();
        map.insert("UTF8_STRING".to_string(), b"hello pasteflow".to_vec());
        let _owner = TestSelectionOwner::spawn(None, "CLIPBOARD", map).unwrap();
        let raw = pump_until_capture(&mut engine, Duration::from_secs(3))
            .expect("capture plain text");
        let rec = match store.insert_raw_clip(&raw, &cfg).unwrap() {
            InsertOutcome::Inserted(c) => c,
            other => panic!("expected Inserted, got {other:?}"),
        };
        assert_eq!(rec.kind, ClipKind::Text);
        assert_eq!(rec.preview, "hello pasteflow");
    }

    // #2 Multiline text
    {
        let multi = (1..=20)
            .map(|i| format!("line {i} with   spaces"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut map = HashMap::new();
        map.insert("UTF8_STRING".to_string(), multi.as_bytes().to_vec());
        let _owner = TestSelectionOwner::spawn(None, "CLIPBOARD", map).unwrap();
        let raw = pump_until_capture(&mut engine, Duration::from_secs(3))
            .expect("capture multiline text");
        let rec = match store.insert_raw_clip(&raw, &cfg).unwrap() {
            InsertOutcome::Inserted(c) => c,
            other => panic!("expected Inserted, got {other:?}"),
        };
        let fmts = store.get_clip_formats(rec.id).unwrap();
        assert_eq!(fmts[0].data, multi.as_bytes());
    }

    // #3a Large 5 MiB text (INCR -> blob)
    {
        let big = vec![b'A'; 5 * 1024 * 1024];
        let mut map = HashMap::new();
        map.insert("UTF8_STRING".to_string(), big.clone());
        let _owner = TestSelectionOwner::spawn(None, "CLIPBOARD", map).unwrap();
        let raw = pump_until_capture(&mut engine, Duration::from_secs(10))
            .expect("capture 5 MiB INCR text");
        let rec = match store.insert_raw_clip(&raw, &cfg).unwrap() {
            InsertOutcome::Inserted(c) => c,
            other => panic!("expected Inserted, got {other:?}"),
        };
        let fmts = store.get_clip_formats(rec.id).unwrap();
        assert_eq!(fmts[0].data.len(), big.len());
        assert_eq!(blake3::hash(&fmts[0].data), blake3::hash(&big));
    }

    // #3b Over-cap 7 MiB text -> aborted/skipped
    {
        let huge = vec![b'B'; 7 * 1024 * 1024];
        let mut map = HashMap::new();
        map.insert("UTF8_STRING".to_string(), huge);
        let _owner = TestSelectionOwner::spawn(None, "CLIPBOARD", map).unwrap();
        let raw = pump_until_capture(&mut engine, Duration::from_millis(800));
        assert!(raw.is_none(), "over-cap payload should be aborted");
    }

    // #7 Image PNG capture
    let png = make_png(320, 240);
    let png_id = {
        let mut map = HashMap::new();
        map.insert("image/png".to_string(), png.clone());
        let _owner = TestSelectionOwner::spawn(None, "CLIPBOARD", map).unwrap();
        let raw = pump_until_capture(&mut engine, Duration::from_secs(3))
            .expect("capture PNG image");
        let rec = match store.insert_raw_clip(&raw, &cfg).unwrap() {
            InsertOutcome::Inserted(c) => c,
            other => panic!("expected Inserted, got {other:?}"),
        };
        assert_eq!(rec.kind, ClipKind::Image);
        let im = rec.image.as_ref().unwrap();
        assert_eq!((im.width, im.height), (320, 240));
        let fmts = store.get_clip_formats(rec.id).unwrap();
        assert_eq!(blake3::hash(&fmts[0].data), blake3::hash(&png));
        rec.id
    };

    // #9 / #10 Persistence across Store reopen
    drop(store);
    let mut reopened = Store::open_at(&db_path, blobs_dir, thumbs_dir).unwrap();
    let all = reopened.list_clips(&ClipFilter::recent(50)).unwrap();
    assert_eq!(all.len(), 4);

    // #12 Delete image clip cleanly
    assert!(reopened.delete_clip(png_id).unwrap());
    assert!(reopened.get_clip(png_id).unwrap().is_none());
}

#[test]
fn m1_password_hint_exclusions_and_pause() {
    if !xvfb_active() {
        return;
    }

    let cfg = Config::default();
    let mut engine = X11Engine::connect(None, cfg).unwrap();

    // #27a x-kde-passwordManagerHint = secret -> must NEVER be captured
    {
        let mut map = HashMap::new();
        map.insert("UTF8_STRING".to_string(), b"top-secret-password".to_vec());
        map.insert(
            "x-kde-passwordManagerHint".to_string(),
            b"secret".to_vec(),
        );
        let _owner = TestSelectionOwner::spawn(None, "CLIPBOARD", map).unwrap();
        assert!(
            pump_until_capture(&mut engine, Duration::from_millis(400)).is_none(),
            "passwordManagerHint=secret must be skipped"
        );
    }

    // #27b Excluded app WM_CLASS = bitwarden-app -> must NEVER be captured
    {
        let mut map = HashMap::new();
        map.insert("UTF8_STRING".to_string(), b"bitwarden-vault-secret".to_vec());
        let opts = TestOwnerOptions {
            wm_class: Some(("bitwarden-app".to_string(), "Bitwarden".to_string())),
            pid: None,
        };
        let _owner =
            TestSelectionOwner::spawn_with_options(None, "CLIPBOARD", map, opts).unwrap();
        assert!(
            pump_until_capture(&mut engine, Duration::from_millis(400)).is_none(),
            "excluded bitwarden-app must be skipped"
        );
    }

    // Pause capture -> skipped while paused, captured after resume
    engine.set_paused(true);
    {
        let mut map = HashMap::new();
        map.insert("UTF8_STRING".to_string(), b"ignored while paused".to_vec());
        let _owner = TestSelectionOwner::spawn(None, "CLIPBOARD", map).unwrap();
        assert!(pump_until_capture(&mut engine, Duration::from_millis(300)).is_none());
    }
    engine.set_paused(false);
    {
        let mut map = HashMap::new();
        map.insert("UTF8_STRING".to_string(), b"captured after resume".to_vec());
        let _owner = TestSelectionOwner::spawn(None, "CLIPBOARD", map).unwrap();
        let raw = pump_until_capture(&mut engine, Duration::from_secs(2))
            .expect("capture after resume");
        assert_eq!(raw.formats[0].data, b"captured after resume");
    }
}

#[test]
fn m1_vanishing_owner_triggers_empty_clipboard_guard() {
    if !xvfb_active() {
        return;
    }

    let cfg = Config::default();
    let mut engine = X11Engine::connect(None, cfg).unwrap();

    // Owner copies text, engine captures it, then owner exits (drops)
    {
        let mut map = HashMap::new();
        map.insert("UTF8_STRING".to_string(), b"survives owner exit".to_vec());
        let _owner = TestSelectionOwner::spawn(None, "CLIPBOARD", map).unwrap();
        let raw = pump_until_capture(&mut engine, Duration::from_secs(2))
            .expect("capture before owner vanishes");
        assert_eq!(raw.formats[0].data, b"survives owner exit");
    }

    // Wait > 300 ms while pumping engine so empty-clipboard guard takes ownership
    let deadline = Instant::now() + Duration::from_millis(700);
    while Instant::now() < deadline && !engine.owns_clipboard() {
        let _ = engine.wait_readable(50);
        let _ = engine.poll_events().unwrap();
    }
    assert!(
        engine.owns_clipboard(),
        "empty-clipboard guard should claim CLIPBOARD after owner vanishes"
    );

    // Spawn a reader thread to read CLIPBOARD back while engine serves it
    let reader = std::thread::spawn(|| {
        let (conn, screen_num) = RustConnection::connect(None).unwrap();
        read_selection_target(
            &conn,
            screen_num,
            "CLIPBOARD",
            "UTF8_STRING",
            1024 * 1024,
            Duration::from_secs(3),
        )
    });

    let serve_deadline = Instant::now() + Duration::from_secs(3);
    let mut served = false;
    while Instant::now() < serve_deadline && !served {
        let _ = engine.wait_readable(25);
        for ev in engine.poll_events().unwrap() {
            if matches!(ev, ClipboardEvent::SelectionServed { .. }) {
                served = true;
            }
        }
    }

    let data = reader.join().unwrap().expect("read served clipboard");
    assert_eq!(data, b"survives owner exit");
}

#[test]
fn m1_rapid_20_copies_no_crash() {
    if !xvfb_active() {
        return;
    }

    let dir = TempDir::new().unwrap();
    let mut store = Store::open_at(
        &dir.path().join("rapid.db"),
        dir.path().join("blobs"),
        dir.path().join("thumbs"),
    )
    .unwrap();
    let cfg = Config::default();
    let mut engine = X11Engine::connect(None, cfg.clone()).unwrap();

    let mut captured = 0usize;
    for i in 0..20 {
        let mut map = HashMap::new();
        map.insert("UTF8_STRING".to_string(), format!("rapid copy #{i}").into_bytes());
        let _owner = TestSelectionOwner::spawn(None, "CLIPBOARD", map).unwrap();
        if let Some(raw) = pump_until_capture(&mut engine, Duration::from_secs(2)) {
            if matches!(
                store.insert_raw_clip(&raw, &cfg).unwrap(),
                InsertOutcome::Inserted(_)
            ) {
                captured += 1;
            }
        }
    }
    assert_eq!(captured, 20, "all 20 sequential copies should be captured");
}
