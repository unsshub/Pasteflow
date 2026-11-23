//! Smart content recognition registry (URL, Color, Email, Phone, Number, Code, Secret).

pub mod code;
pub mod color;
pub mod secret;

use crate::model::ClipKind;
use regex::Regex;
use std::sync::OnceLock;
use url::Url;

/// Result of running the detector registry on a text clip.
#[derive(Debug, Clone)]
pub struct DetectionResult {
    pub kind: ClipKind,
    pub meta_json: Option<String>,
    pub is_secret: bool,
    pub secret_reason: Option<&'static str>,
}

/// Classify a plain text payload into its smart `ClipKind` and structured `meta` JSON.
pub fn classify_text(text: &str) -> DetectionResult {
    let trimmed = text.trim();
    let secret_reason = secret::detect_secret(trimmed);
    let is_secret = secret_reason.is_some();

    // 1. Color (#RRGGBB, rgb(...), hsl(...))
    if let Some(col) = color::parse_color(trimmed) {
        let meta = serde_json::json!({
            "r": col.r,
            "g": col.g,
            "b": col.b,
            "a": col.a,
            "hex": col.hex,
            "rgb": col.rgb,
            "hsl": col.hsl,
        });
        return DetectionResult {
            kind: ClipKind::Color,
            meta_json: Some(meta.to_string()),
            is_secret: false,
            secret_reason: None,
        };
    }

    // 2. URL (http, https, ftp, mailto)
    if !trimmed.contains(char::is_whitespace)
        && (trimmed.starts_with("http://")
            || trimmed.starts_with("https://")
            || trimmed.starts_with("ftp://"))
    {
        if let Ok(parsed) = Url::parse(trimmed)
            && parsed.host_str().is_some()
        {
            let meta = serde_json::json!({
                "scheme": parsed.scheme(),
                "host": parsed.host_str().unwrap_or(""),
                "path": parsed.path(),
            });
            return DetectionResult {
                kind: ClipKind::Url,
                meta_json: Some(meta.to_string()),
                is_secret: false,
                secret_reason: None,
            };
        }
    }

    // 3. Email
    if is_email(trimmed) {
        let domain = trimmed.split('@').nth(1).unwrap_or("");
        return DetectionResult {
            kind: ClipKind::Email,
            meta_json: Some(serde_json::json!({ "domain": domain }).to_string()),
            is_secret: false,
            secret_reason: None,
        };
    }

    // 4. Phone number (with formatting/country code, 7..=15 digits, not a plain small integer)
    if is_phone(trimmed) {
        return DetectionResult {
            kind: ClipKind::Phone,
            meta_json: None,
            is_secret: false,
            secret_reason: None,
        };
    }

    // 5. Number (integers, floats, currency, percentages)
    if is_number(trimmed) {
        return DetectionResult {
            kind: ClipKind::Number,
            meta_json: None,
            is_secret,
            secret_reason,
        };
    }

    // 6. Code (heuristic language detection)
    if let Some(lang) = code::detect_code_language(trimmed) {
        let mut meta = serde_json::json!({ "lang": lang });
        if let Some(reason) = secret_reason {
            meta["secret"] = serde_json::json!(reason);
        }
        return DetectionResult {
            kind: ClipKind::Code,
            meta_json: Some(meta.to_string()),
            is_secret,
            secret_reason,
        };
    }

    let meta_json = secret_reason.map(|r| serde_json::json!({ "secret": r }).to_string());
    DetectionResult {
        kind: ClipKind::Text,
        meta_json,
        is_secret,
        secret_reason,
    }
}

fn is_email(s: &str) -> bool {
    static EMAIL_RE: OnceLock<Regex> = OnceLock::new();
    let re = EMAIL_RE.get_or_init(|| {
        Regex::new(r"^[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}$").unwrap()
    });
    s.len() <= 254 && re.is_match(s)
}

fn is_phone(s: &str) -> bool {
    if s.len() < 7 || s.len() > 25 || s.contains('\n') {
        return false;
    }
    // Must contain phone punctuation or leading '+' so plain numbers like 12345678 stay Number/OTP
    let has_phone_punct =
        s.starts_with('+') || s.contains('-') || s.contains('(') || s.contains(' ');
    if !has_phone_punct {
        return false;
    }
    static PHONE_RE: OnceLock<Regex> = OnceLock::new();
    let re = PHONE_RE.get_or_init(|| Regex::new(r"^\+?[\d\s().\-]{7,25}$").unwrap());
    if !re.is_match(s) {
        return false;
    }
    let digits = s.bytes().filter(|b| b.is_ascii_digit()).count();
    (7..=15).contains(&digits)
}

fn is_number(s: &str) -> bool {
    static NUM_RE: OnceLock<Regex> = OnceLock::new();
    let re = NUM_RE.get_or_init(|| {
        Regex::new(r"^[+-]?(?:\$|€|£)?\d{1,3}(?:,\d{3})*(?:\.\d+)?%?$|^[-+]?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?%?$")
            .unwrap()
    });
    s.len() <= 40 && re.is_match(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_driven_detector_fixtures() {
        let cases: &[(&str, ClipKind)] = &[
            ("https://www.pastily.app/docs", ClipKind::Url),
            ("http://localhost:8080/api/v1/clips", ClipKind::Url),
            ("#FF6B6B", ClipKind::Color),
            ("rgb(53, 132, 228)", ClipKind::Color),
            ("hsl(210, 80%, 55%)", ClipKind::Color),
            ("alice.smith+dev@example.org", ClipKind::Email),
            ("+1 (555) 234-5678", ClipKind::Phone),
            ("+33 6 12 34 56 78", ClipKind::Phone),
            ("42", ClipKind::Number),
            ("$1,299.95", ClipKind::Number),
            ("98.6%", ClipKind::Number),
            ("3.14159e-4", ClipKind::Number),
            (
                "pub fn add(a: i32, b: i32) -> i32 { a + b }",
                ClipKind::Code,
            ),
            ("{\"key\": \"value\", \"ok\": true}", ClipKind::Code),
            (
                "SELECT * FROM users WHERE active = 1 ORDER BY created_at DESC",
                ClipKind::Code,
            ),
            ("Remember to buy milk and bread today.", ClipKind::Text),
            ("Meeting notes: Q4 roadmap review at 3pm", ClipKind::Text),
        ];

        let mut passed = 0usize;
        for &(input, expected) in cases {
            let res = classify_text(input);
            if res.kind == expected {
                passed += 1;
            } else {
                panic!("input {input:?}: expected {expected:?}, got {:?}", res.kind);
            }
        }
        assert_eq!(passed, cases.len());
    }
}
