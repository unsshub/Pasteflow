//! Memory-only ephemeral store for sensitive clips and short-lived clips (expiry ≤ 1 h).
//! Payloads are stored in `Zeroizing<Vec<u8>>` and NEVER written to disk (Q3 / §17).

use pf_core::model::{ClipKind, ClipRecord, RawClip, RawFormat, canonical_hash, now_unix_ms};
use zeroize::Zeroizing;

pub const SHORT_EXPIRY_MAX_MS: i64 = 3_600_000; // 1 hour

struct EphemeralFormat {
    mime: String,
    data: Zeroizing<Vec<u8>>,
}

struct EphemeralEntry {
    record: ClipRecord,
    formats: Vec<EphemeralFormat>,
}

/// RAM-only clipboard store using negative IDs (`-1`, `-2`, …) and `zeroize` on drop/expiry.
#[derive(Default)]
pub struct EphemeralStore {
    next_id: i64,
    entries: Vec<EphemeralEntry>,
}

impl EphemeralStore {
    pub fn new() -> Self {
        Self {
            next_id: -1,
            entries: Vec::new(),
        }
    }

    /// Mask a sensitive string so only the first few and last 3 characters are visible in previews.
    pub fn mask_secret_preview(text: &str) -> String {
        let trimmed = text.trim();
        let chars: Vec<char> = trimmed.chars().collect();
        if chars.len() <= 8 {
            return "••••••••".to_string();
        }
        let head: String = chars[..4].iter().collect();
        let tail: String = chars[chars.len() - 3..].iter().collect();
        format!("{head}•••{tail} (sensitive)")
    }

    /// Insert a memory-only clip with an explicit `expires_at` Unix-ms timestamp.
    pub fn insert_raw(
        &mut self,
        raw: &RawClip,
        kind: ClipKind,
        primary_text: Option<&str>,
        is_secret: bool,
        expires_at: i64,
    ) -> ClipRecord {
        let primary_bytes: &[u8] = primary_text
            .map(str::as_bytes)
            .or_else(|| raw.formats.first().map(|f| f.data.as_slice()))
            .unwrap_or(&[]);
        let hash = canonical_hash(kind, primary_bytes);

        // Deduplicate in-memory by hash
        if let Some(existing) = self.entries.iter_mut().find(|e| e.record.content_hash == hash) {
            existing.record.last_used_at = raw.captured_at_ms;
            existing.record.copy_count += 1;
            existing.record.expires_at = Some(expires_at);
            return existing.record.clone();
        }

        let id = self.next_id;
        self.next_id -= 1;

        let preview = if is_secret {
            Self::mask_secret_preview(primary_text.unwrap_or(""))
        } else {
            pf_core::model::make_text_preview(primary_text.unwrap_or("(ephemeral clip)"))
        };

        let byte_size: i64 = raw.formats.iter().map(|f| f.data.len() as i64).sum();
        let meta = if is_secret {
            Some(r#"{"sensitive":true,"ephemeral":true}"#.to_string())
        } else {
            Some(r#"{"ephemeral":true}"#.to_string())
        };

        let record = ClipRecord {
            id,
            kind,
            content_hash: hash,
            title: None,
            preview,
            search_text: String::new(), // never indexed on disk
            meta,
            byte_size,
            space_id: 1,
            pinned: false,
            selection: raw.selection,
            source_app: raw.source_app.clone(),
            created_at: raw.captured_at_ms,
            last_used_at: raw.captured_at_ms,
            copy_count: 1,
            expires_at: Some(expires_at),
            image: None,
        };

        let formats = raw
            .formats
            .iter()
            .map(|f| EphemeralFormat {
                mime: f.mime.clone(),
                data: Zeroizing::new(f.data.clone()),
            })
            .collect();

        self.entries.push(EphemeralEntry {
            record: record.clone(),
            formats,
        });
        record
    }

    pub fn list(&self) -> Vec<ClipRecord> {
        let mut out: Vec<ClipRecord> = self.entries.iter().map(|e| e.record.clone()).collect();
        out.sort_by(|a, b| b.last_used_at.cmp(&a.last_used_at));
        out
    }

    pub fn get_formats(&self, id: i64) -> Option<Vec<RawFormat>> {
        let entry = self.entries.iter().find(|e| e.record.id == id)?;
        Some(
            entry
                .formats
                .iter()
                .map(|f| RawFormat {
                    mime: f.mime.clone(),
                    data: f.data.to_vec(),
                })
                .collect(),
        )
    }

    pub fn delete(&mut self, id: i64) -> Option<[u8; 32]> {
        let pos = self.entries.iter().position(|e| e.record.id == id)?;
        let removed = self.entries.remove(pos);
        // `removed.formats` drops here, zeroizing all byte buffers.
        Some(removed.record.content_hash)
    }

    pub fn clear(&mut self) -> Vec<[u8; 32]> {
        let hashes = self
            .entries
            .iter()
            .map(|e| e.record.content_hash)
            .collect();
        self.entries.clear();
        hashes
    }

    /// Return the earliest `expires_at` timestamp (Unix ms) among ephemeral entries.
    pub fn next_expiry_ms(&self) -> Option<i64> {
        self.entries
            .iter()
            .filter_map(|e| e.record.expires_at)
            .min()
    }

    /// Purge all entries whose `expires_at <= now_ms` (zeroizing their RAM buffers) and return
    /// their `content_hash`es so the clipboard can be cleared if it still holds them.
    pub fn purge_expired(&mut self, now_ms: i64) -> Vec<[u8; 32]> {
        let mut expired_hashes = Vec::new();
        let mut i = 0;
        while i < self.entries.len() {
            if let Some(exp) = self.entries[i].record.expires_at
                && exp <= now_ms
            {
                let removed = self.entries.remove(i);
                expired_hashes.push(removed.record.content_hash);
            } else {
                i += 1;
            }
        }
        expired_hashes
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn should_be_ephemeral(is_secret: bool, expiry_secs: Option<u64>) -> bool {
        if is_secret {
            return true;
        }
        if let Some(secs) = expiry_secs {
            return (secs as i64) * 1000 <= SHORT_EXPIRY_MAX_MS;
        }
        false
    }

    pub fn default_expiry_ms_from_now(secs: u64) -> i64 {
        now_unix_ms() + (secs as i64) * 1000
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pf_core::model::SelectionKind;

    #[test]
    fn ephemeral_store_masks_secrets_and_purges_on_expiry() {
        let mut eph = EphemeralStore::new();
        let raw = RawClip {
            selection: SelectionKind::Clipboard,
            formats: vec![RawFormat {
                mime: "UTF8_STRING".to_string(),
                data: b"sk-live-9f2a8b7c6d5e4f3a2b1c".to_vec(),
            }],
            source_app: Some("firefox".to_string()),
            owner_pid: None,
            captured_at_ms: 1000,
        };
        let rec = eph.insert_raw(
            &raw,
            ClipKind::Text,
            Some("sk-live-9f2a8b7c6d5e4f3a2b1c"),
            true,
            16_000,
        );
        assert!(rec.id < 0);
        assert!(!rec.preview.contains("9f2a8b7c"));
        assert!(rec.preview.contains("•••"));
        assert_eq!(eph.next_expiry_ms(), Some(16_000));

        assert!(eph.purge_expired(15_999).is_empty());
        let purged = eph.purge_expired(16_000);
        assert_eq!(purged.len(), 1);
        assert_eq!(purged[0], rec.content_hash);
        assert!(eph.is_empty());
    }
}
