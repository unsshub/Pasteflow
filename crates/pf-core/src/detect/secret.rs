//! Best-effort detection of secret-looking clipboard text (API keys, private keys, JWTs, OTPs,
//! high-entropy single tokens).

use regex::Regex;
use std::sync::OnceLock;

/// Returns `Some(reason)` if `text` looks like a secret token, private key, JWT, OTP, or
/// high-entropy single-word secret.
pub fn detect_secret(text: &str) -> Option<&'static str> {
    let s = text.trim();
    if s.is_empty() {
        return None;
    }

    // PEM private keys
    if s.contains("-----BEGIN") && s.contains("PRIVATE KEY-----") {
        return Some("private_key");
    }

    // Known API key / token prefixes
    const PREFIXES: &[(&str, &str)] = &[
        ("sk-", "api_key"),
        ("sk_live_", "api_key"),
        ("sk_test_", "api_key"),
        ("ghp_", "github_token"),
        ("github_pat_", "github_token"),
        ("gho_", "github_token"),
        ("xoxb-", "slack_token"),
        ("xoxp-", "slack_token"),
        ("AKIA", "aws_key"),
        ("AIza", "google_api_key"),
    ];
    for &(prefix, reason) in PREFIXES {
        if s.starts_with(prefix) && s.len() >= 16 && !s.contains(char::is_whitespace) {
            return Some(reason);
        }
    }

    // JWT (`eyJ... . eyJ... . ...`)
    if s.starts_with("eyJ") && !s.contains(char::is_whitespace) {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() == 3 && parts[0].len() >= 10 && parts[1].len() >= 10 && parts[2].len() >= 10
        {
            return Some("jwt");
        }
    }

    // 6-to-8 digit standalone OTP code
    static OTP_RE: OnceLock<Regex> = OnceLock::new();
    let otp_re = OTP_RE.get_or_init(|| Regex::new(r"^\d{6,8}$").unwrap());
    if otp_re.is_match(s) {
        return Some("otp");
    }

    // High-entropy single token (16..=128 chars, no whitespace, mixed character classes)
    if (16..=128).contains(&s.len())
        && !s.contains(char::is_whitespace)
        && !s.starts_with("http://")
        && !s.starts_with("https://")
        && !s.starts_with('/')
    {
        let has_upper = s.bytes().any(|b| b.is_ascii_uppercase());
        let has_lower = s.bytes().any(|b| b.is_ascii_lowercase());
        let has_digit = s.bytes().any(|b| b.is_ascii_digit());
        let classes = usize::from(has_upper) + usize::from(has_lower) + usize::from(has_digit);
        if classes >= 3 && shannon_entropy(s.as_bytes()) >= 3.8 {
            return Some("high_entropy");
        }
    }

    None
}

fn shannon_entropy(data: &[u8]) -> f64 {
    if data.is_empty() {
        return 0.0;
    }
    let mut counts = [0usize; 256];
    for &b in data {
        counts[b as usize] += 1;
    }
    let len = data.len() as f64;
    let mut entropy = 0.0;
    for &c in &counts {
        if c > 0 {
            let p = c as f64 / len;
            entropy -= p * p.log2();
        }
    }
    entropy
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_secrets_without_false_positives() {
        assert_eq!(detect_secret("sk-live-9f2a8b7c6d5e4f3a2b1c"), Some("api_key"));
        assert_eq!(
            detect_secret("ghp_1234567890abcdefghijklmnopqrstuvwxyz"),
            Some("github_token")
        );
        assert_eq!(detect_secret("AKIAIOSFODNN7EXAMPLE"), Some("aws_key"));
        assert_eq!(
            detect_secret("-----BEGIN RSA PRIVATE KEY-----\nMIIE..."),
            Some("private_key")
        );
        assert_eq!(
            detect_secret("eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U"),
            Some("jwt")
        );
        assert_eq!(detect_secret("482910"), Some("otp"));
        assert_eq!(detect_secret("aB9$kL2!mN7@pQ4#rT8"), Some("high_entropy"));

        // False positive traps
        assert_eq!(detect_secret("hello world"), None);
        assert_eq!(detect_secret("https://example.com/aB9kL2mN7pQ4rT8"), None);
        assert_eq!(detect_secret("/home/unss/Documents/MyProject2026"), None);
        assert_eq!(detect_secret("1234"), None);
    }
}
