pub mod detail;
pub mod main_window;
pub mod overlay;
pub mod shortcut_dialog;

use gtk4::CssProvider;
use gtk4::gdk::Display;

const STYLE_CSS: &str = include_str!("style.css");

/// Load PasteFlow's lightweight stylesheet onto the default GDK display.
pub fn install_css() {
    let provider = CssProvider::new();
    provider.load_from_string(STYLE_CSS);
    if let Some(display) = Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

/// Format a Unix-millisecond timestamp as a concise relative string ("just now", "4 m ago", etc.).
pub fn format_relative_time(ts_ms: i64, now_ms: i64) -> String {
    let diff_sec = (now_ms - ts_ms).max(0) / 1000;
    if diff_sec < 15 {
        "just now".to_string()
    } else if diff_sec < 60 {
        format!("{diff_sec} s ago")
    } else if diff_sec < 3600 {
        format!("{} m ago", diff_sec / 60)
    } else if diff_sec < 86_400 {
        format!("{} h ago", diff_sec / 3600)
    } else {
        format!("{} d ago", diff_sec / 86_400)
    }
}

/// Format a byte count for human display.
pub fn format_bytes(bytes: i64) -> String {
    let b = bytes.max(0) as f64;
    if b < 1024.0 {
        format!("{bytes} B")
    } else if b < 1024.0 * 1024.0 {
        format!("{:.1} KiB", b / 1024.0)
    } else {
        format!("{:.1} MiB", b / (1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_time_formatting() {
        let now = 10_000_000_000i64;
        assert_eq!(format_relative_time(now - 5_000, now), "just now");
        assert_eq!(format_relative_time(now - 45_000, now), "45 s ago");
        assert_eq!(format_relative_time(now - 240_000, now), "4 m ago");
        assert_eq!(format_relative_time(now - 7_200_000, now), "2 h ago");
        assert_eq!(format_relative_time(now - 172_800_000, now), "2 d ago");
    }

    #[test]
    fn byte_size_formatting() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2048), "2.0 KiB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MiB");
    }
}
