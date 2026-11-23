pub mod detail;
pub mod main_window;
pub mod overlay;
pub mod queue_view;
pub mod settings;
pub mod shortcut_dialog;
pub mod spaces_dialog;

use gtk4::CssProvider;
use gtk4::gdk::Display;
use pf_core::model::ClipKind;

const STYLE_CSS: &str = include_str!("style.css");

pub const ALL_CARD_CLASSES: &[&str] = &[
    "pf-card-text",
    "pf-card-code",
    "pf-card-url",
    "pf-card-color",
    "pf-card-email",
    "pf-card-phone",
    "pf-card-number",
    "pf-card-image",
    "pf-card-files",
    "pf-card-ephemeral",
];

pub fn card_css_class(kind: ClipKind, is_ephemeral: bool) -> &'static str {
    if is_ephemeral {
        return "pf-card-ephemeral";
    }
    match kind {
        ClipKind::Text => "pf-card-text",
        ClipKind::Code => "pf-card-code",
        ClipKind::Url => "pf-card-url",
        ClipKind::Color => "pf-card-color",
        ClipKind::Email => "pf-card-email",
        ClipKind::Phone => "pf-card-phone",
        ClipKind::Number => "pf-card-number",
        ClipKind::Image => "pf-card-image",
        ClipKind::Files => "pf-card-files",
    }
}

pub fn kind_display_name(kind: ClipKind) -> &'static str {
    match kind {
        ClipKind::Text => "Text",
        ClipKind::Code => "Code",
        ClipKind::Url => "Links",
        ClipKind::Color => "Colors",
        ClipKind::Email => "Email",
        ClipKind::Phone => "Phone",
        ClipKind::Number => "Numbers",
        ClipKind::Image => "Images",
        ClipKind::Files => "Files",
    }
}

pub fn kind_icon_name(kind: ClipKind) -> &'static str {
    match kind {
        ClipKind::Image => "image-x-generic-symbolic",
        ClipKind::Files => "folder-documents-symbolic",
        ClipKind::Code => "utilities-terminal-symbolic",
        ClipKind::Url => "insert-link-symbolic",
        ClipKind::Color => "applications-graphics-symbolic",
        ClipKind::Email => "mail-unread-symbolic",
        ClipKind::Phone => "call-start-symbolic",
        ClipKind::Number => "accessories-calculator-symbolic",
        ClipKind::Text => "edit-paste-symbolic",
    }
}

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
