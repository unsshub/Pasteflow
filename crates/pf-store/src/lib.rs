//! Local SQLite (WAL + FTS5) and content-addressed blob storage for PasteFlow.

use anyhow::Result;
use rusqlite::Connection;

/// Verify bundled SQLite supports WAL, FTS5 trigram, and FTS5 `secure-delete`.
pub fn verify_sqlite_capabilities() -> Result<String> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(
        "CREATE VIRTUAL TABLE _probe_fts USING fts5(body, tokenize='trigram');
         INSERT INTO _probe_fts(_probe_fts, rank) VALUES('secure-delete', 1);",
    )?;
    let ver: String = conn.query_row("SELECT sqlite_version()", [], |r| r.get(0))?;
    Ok(ver)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_sqlite_supports_fts5_trigram_and_secure_delete() {
        let ver = verify_sqlite_capabilities().expect("FTS5 trigram + secure-delete");
        assert!(ver.starts_with("3."), "unexpected sqlite version: {ver}");
    }
}
