use std::fmt;

/// Wrapper that prevents clipboard payloads from leaking into debug/tracing logs.
pub struct Redacted<T>(pub T);

impl<T: AsRef<[u8]>> fmt::Debug for Redacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<redacted {} B>", self.0.as_ref().len())
    }
}

impl<T: AsRef<[u8]>> fmt::Display for Redacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<redacted {} B>", self.0.as_ref().len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacted_never_prints_secret_payload() {
        let secret = "sk-live-super-secret-value-12345";
        let debug_out = format!("{:?}", Redacted(secret));
        let display_out = format!("{}", Redacted(secret));
        assert!(!debug_out.contains("super-secret"));
        assert!(!display_out.contains("super-secret"));
        assert_eq!(debug_out, format!("<redacted {} B>", secret.len()));
    }
}
