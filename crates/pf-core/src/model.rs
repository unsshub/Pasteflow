use crate::redact::Redacted;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

pub const PREVIEW_MAX_CHARS: usize = 300;
pub const SEARCH_TEXT_MAX_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClipKind {
    Text,
    Code,
    Url,
    Color,
    Email,
    Phone,
    Number,
    Image,
    Files,
}

impl ClipKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Code => "code",
            Self::Url => "url",
            Self::Color => "color",
            Self::Email => "email",
            Self::Phone => "phone",
            Self::Number => "number",
            Self::Image => "image",
            Self::Files => "files",
        }
    }

    pub fn from_db(s: &str) -> Option<Self> {
        match s {
            "text" => Some(Self::Text),
            "code" => Some(Self::Code),
            "url" => Some(Self::Url),
            "color" => Some(Self::Color),
            "email" => Some(Self::Email),
            "phone" => Some(Self::Phone),
            "number" => Some(Self::Number),
            "image" => Some(Self::Image),
            "files" => Some(Self::Files),
            _ => None,
        }
    }
}

impl fmt::Display for ClipKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum SelectionKind {
    Clipboard,
    Primary,
}

impl SelectionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Clipboard => "CLIPBOARD",
            Self::Primary => "PRIMARY",
        }
    }

    pub fn from_db(s: &str) -> Option<Self> {
        match s {
            "CLIPBOARD" => Some(Self::Clipboard),
            "PRIMARY" => Some(Self::Primary),
            _ => None,
        }
    }
}

/// A single MIME payload captured from or served to the clipboard.
/// `Debug` is explicitly redacted so clipboard content never reaches logs.
#[derive(Clone, PartialEq, Eq)]
pub struct RawFormat {
    pub mime: String,
    pub data: Vec<u8>,
}

impl fmt::Debug for RawFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawFormat")
            .field("mime", &self.mime)
            .field("data", &Redacted(&self.data))
            .finish()
    }
}

/// A raw clipboard capture emitted by a `ClipboardBackend`.
#[derive(Clone)]
pub struct RawClip {
    pub selection: SelectionKind,
    pub formats: Vec<RawFormat>,
    pub source_app: Option<String>,
    pub owner_pid: Option<u32>,
    pub captured_at_ms: i64,
}

impl fmt::Debug for RawClip {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawClip")
            .field("selection", &self.selection)
            .field("formats", &self.formats)
            .field("source_app", &self.source_app)
            .field("owner_pid", &self.owner_pid)
            .field("captured_at_ms", &self.captured_at_ms)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageMeta {
    pub width: u32,
    pub height: u32,
    pub original_mime: String,
    pub thumb_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipRecord {
    pub id: i64,
    pub kind: ClipKind,
    pub content_hash: [u8; 32],
    pub title: Option<String>,
    pub preview: String,
    pub search_text: String,
    pub meta: Option<String>,
    pub byte_size: i64,
    pub space_id: i64,
    pub pinned: bool,
    pub selection: SelectionKind,
    pub source_app: Option<String>,
    pub created_at: i64,
    pub last_used_at: i64,
    pub copy_count: i64,
    pub expires_at: Option<i64>,
    pub image: Option<ImageMeta>,
}

impl ClipRecord {
    pub fn short_hash(&self) -> String {
        hex_prefix(&self.content_hash, 4)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceRecord {
    pub id: i64,
    pub name: String,
    pub color: String,
    pub icon: String,
    pub position: i64,
    pub is_default: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, Default)]
pub struct ClipFilter {
    pub query: Option<String>,
    pub kind: Option<ClipKind>,
    pub space_id: Option<i64>,
    pub pinned_only: bool,
    pub limit: usize,
    pub offset: usize,
}

impl ClipFilter {
    pub fn recent(limit: usize) -> Self {
        Self {
            limit,
            ..Default::default()
        }
    }
}

pub fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Compute the canonical 32-byte BLAKE3 hash for deduplication.
pub fn canonical_hash(kind: ClipKind, primary_bytes: &[u8]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(kind.as_str().as_bytes());
    hasher.update(b"\0");
    hasher.update(primary_bytes);
    *hasher.finalize().as_bytes()
}

/// Format the first `num_bytes` bytes of a hash as lowercase hex.
pub fn hex_prefix(bytes: &[u8], num_bytes: usize) -> String {
    let n = num_bytes.min(bytes.len());
    let mut out = String::with_capacity(n * 2);
    for &b in &bytes[..n] {
        use std::fmt::Write;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

/// Format a full 32-byte hash as 64 lowercase hex characters.
pub fn hex_full(bytes: &[u8; 32]) -> String {
    hex_prefix(bytes, 32)
}

/// Build a ≤ 300-character preview string for text clips.
pub fn make_text_preview(text: &str) -> String {
    let trimmed = text.trim();
    let mut out = String::new();
    let mut count = 0usize;
    for ch in trimmed.chars() {
        if count >= PREVIEW_MAX_CHARS {
            out.push('…');
            break;
        }
        out.push(ch);
        count += 1;
    }
    out
}

/// Truncate search text safely on a UTF-8 char boundary to at most `SEARCH_TEXT_MAX_BYTES`.
pub fn make_search_text(text: &str) -> String {
    if text.len() <= SEARCH_TEXT_MAX_BYTES {
        return text.to_string();
    }
    let mut end = SEARCH_TEXT_MAX_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

/// Parse `x-special/gnome-copied-files` or `text/uri-list` into a displayable list of file paths/URIs.
pub fn parse_file_list(mime: &str, data: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(data);
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    if mime == "x-special/gnome-copied-files" {
        // First line is action: "copy" or "cut"
        let first = lines.next();
        if let Some(f) = first {
            if f != "copy" && f != "cut" {
                let mut res = vec![strip_file_scheme(f)];
                res.extend(
                    lines
                        .filter(|l| !l.starts_with('#'))
                        .map(strip_file_scheme),
                );
                return res;
            }
        }
    }
    lines
        .filter(|l| !l.starts_with('#'))
        .map(strip_file_scheme)
        .collect()
}

fn strip_file_scheme(uri: &str) -> String {
    if let Some(rest) = uri.strip_prefix("file://") {
        percent_decode(rest)
    } else {
        uri.to_string()
    }
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Heuristic: is this text just a single URL or bare filename?
/// Used by target arbitration (§7.3 #4) to decide whether an accompanying image is primary.
pub fn is_url_or_filename_only(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return true;
    }
    if t.contains('\n') || t.contains('\r') {
        return false;
    }
    if t.starts_with("http://") || t.starts_with("https://") || t.starts_with("file://") {
        return !t.contains(char::is_whitespace);
    }
    let lower = t.to_ascii_lowercase();
    let img_exts = [".png", ".jpg", ".jpeg", ".webp", ".gif", ".bmp", ".svg"];
    img_exts.iter().any(|ext| lower.ends_with(ext)) && !t.contains('\t')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_format_debug_is_redacted() {
        let fmt = RawFormat {
            mime: "text/plain;charset=utf-8".to_string(),
            data: b"super-secret-password-999".to_vec(),
        };
        let dbg = format!("{fmt:?}");
        assert!(!dbg.contains("super-secret"));
        assert!(dbg.contains("<redacted 25 B>"));
    }

    #[test]
    fn text_preview_truncates_at_300_chars() {
        let long = "a".repeat(350);
        let preview = make_text_preview(&long);
        assert_eq!(preview.chars().count(), 301);
        assert!(preview.ends_with('…'));
    }

    #[test]
    fn search_text_truncates_on_utf8_boundary() {
        let emoji = "🦀".repeat(20_000); // 80,000 bytes
        let st = make_search_text(&emoji);
        assert!(st.len() <= SEARCH_TEXT_MAX_BYTES);
        assert_eq!(st.len() % 4, 0);
    }

    #[test]
    fn parse_gnome_copied_files() {
        let data = b"copy\nfile:///home/unss/My%20Doc.pdf\nfile:///tmp/a.txt\n";
        let files = parse_file_list("x-special/gnome-copied-files", data);
        assert_eq!(files, vec!["/home/unss/My Doc.pdf", "/tmp/a.txt"]);
    }

    #[test]
    fn url_or_filename_detection() {
        assert!(is_url_or_filename_only("https://example.com/img.png"));
        assert!(is_url_or_filename_only("screenshot.png"));
        assert!(is_url_or_filename_only("   "));
        assert!(!is_url_or_filename_only("A1\tB1\nA2\tB2"));
        assert!(!is_url_or_filename_only("Hello world from LibreOffice"));
    }
}
