//! Local SQLite (WAL + FTS5) and content-addressed blob storage for PasteFlow.

pub mod blobs;

use anyhow::{Context, Result, bail};
use blobs::BlobStore;
use pf_core::config::{AppPaths, Config};
use pf_core::model::{
    ClipFilter, ClipKind, ClipRecord, ImageMeta, QueueItemRecord, RawClip, RawFormat,
    SelectionKind, SpaceRecord, canonical_hash, is_url_or_filename_only, make_search_text,
    make_text_preview, now_unix_ms, parse_file_list,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::collections::HashSet;
use std::path::Path;
use tracing::{debug, info, warn};

const MIGRATION_001: &str = include_str!("../migrations/001_init.sql");
const CURRENT_SCHEMA_VERSION: i64 = 1;

/// Result of attempting to store a captured clipboard payload.
#[derive(Debug, Clone)]
pub enum InsertOutcome {
    Inserted(ClipRecord),
    Deduplicated(ClipRecord),
    Skipped(&'static str),
}

/// Primary SQLite + blob store handle.
pub struct Store {
    conn: Connection,
    blobs: BlobStore,
}

impl Store {
    /// Open (or create) the persistent PasteFlow database and blob/thumbnail directories.
    pub fn open(paths: &AppPaths) -> Result<Self> {
        paths.ensure_dirs()?;
        Self::open_at(&paths.db_file(), paths.blobs_dir(), paths.thumbs_dir())
    }

    /// Open at explicit paths (useful for tests and isolated environments).
    pub fn open_at(
        db_path: &Path,
        blobs_dir: std::path::PathBuf,
        thumbs_dir: std::path::PathBuf,
    ) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            pf_core::config::create_private_dir(parent)?;
        }
        let blobs = BlobStore::new(blobs_dir, thumbs_dir)?;
        let conn = Connection::open(db_path)
            .with_context(|| format!("open sqlite db {}", db_path.display()))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(db_path, std::fs::Permissions::from_mode(0o600));
        }

        let store = Self { conn, blobs };
        store.configure_pragmas()?;
        store.check_integrity()?;
        store.run_migrations()?;
        Ok(store)
    }

    fn configure_pragmas(&self) -> Result<()> {
        self.conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA secure_delete = ON;
             PRAGMA temp_store = MEMORY;
             PRAGMA busy_timeout = 2000;",
        )?;
        Ok(())
    }

    fn check_integrity(&self) -> Result<()> {
        let status: String = self
            .conn
            .query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        if status != "ok" {
            bail!("SQLite quick_check failed: {status}");
        }
        Ok(())
    }

    fn run_migrations(&self) -> Result<()> {
        let ver: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if ver == 0 {
            self.conn.execute_batch(MIGRATION_001)?;
            self.conn
                .execute_batch(&format!("PRAGMA user_version = {CURRENT_SCHEMA_VERSION};"))?;
            info!(version = CURRENT_SCHEMA_VERSION, "initialized database schema");
        } else if ver > CURRENT_SCHEMA_VERSION {
            bail!(
                "database schema version {ver} is newer than supported version {CURRENT_SCHEMA_VERSION}"
            );
        }
        Ok(())
    }

    pub fn blobs(&self) -> &BlobStore {
        &self.blobs
    }

    /// Truncate the WAL file after privacy-sensitive deletions or before exit.
    pub fn wal_checkpoint_truncate(&self) -> Result<()> {
        let _ = self
            .conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
        Ok(())
    }

    /// List all Spaces ordered by `position`.
    pub fn list_spaces(&self) -> Result<Vec<SpaceRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, color, icon, position, is_default, created_at
             FROM spaces ORDER BY position ASC, id ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(SpaceRecord {
                id: r.get(0)?,
                name: r.get(1)?,
                color: r.get(2)?,
                icon: r.get(3)?,
                position: r.get(4)?,
                is_default: r.get::<_, i64>(5)? != 0,
                created_at: r.get(6)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Return the ID of the default Space ("General").
    pub fn default_space_id(&self) -> Result<i64> {
        self.conn
            .query_row("SELECT id FROM spaces WHERE is_default = 1 LIMIT 1", [], |r| {
                r.get(0)
            })
            .context("default space missing")
    }

    /// Insert a captured `RawClip` (handling classification, deduplication, blob storage,
    /// thumbnailing, and retention pruning).
    pub fn insert_raw_clip(&mut self, raw: &RawClip, config: &Config) -> Result<InsertOutcome> {
        if raw.formats.is_empty() {
            return Ok(InsertOutcome::Skipped("empty formats"));
        }

        let prepared = match prepare_clip(raw, config) {
            PreparedClip::Ready(p) => p,
            PreparedClip::Skip(reason) => {
                debug!(reason, "skipped clip");
                return Ok(InsertOutcome::Skipped(reason));
            }
        };

        // Check for duplicate hash first
        if config.ignore_duplicates {
            let existing_id: Option<i64> = self
                .conn
                .query_row(
                    "SELECT id FROM clips WHERE content_hash = ?1",
                    params![prepared.content_hash.as_slice()],
                    |r| r.get(0),
                )
                .optional()?;

            if let Some(id) = existing_id {
                self.conn.execute(
                    "UPDATE clips
                     SET last_used_at = ?1, copy_count = copy_count + 1
                     WHERE id = ?2",
                    params![raw.captured_at_ms, id],
                )?;
                let rec = self
                    .get_clip(id)?
                    .context("existing clip vanished during dedupe")?;
                debug!(id = rec.id, hash = %rec.short_hash(), "deduplicated clip");
                return Ok(InsertOutcome::Deduplicated(rec));
            }
        }

        // Write external blobs and thumbnails BEFORE opening the SQLite transaction (§11).
        let mut stored_formats: Vec<(String, Option<Vec<u8>>, Option<String>, i64)> =
            Vec::with_capacity(prepared.formats.len());

        let mut image_meta: Option<ImageMeta> = None;
        let mut preview = prepared.preview;
        let mut search_text = prepared.search_text;

        if prepared.kind == ClipKind::Image {
            if let Some(img_fmt) = prepared
                .formats
                .iter()
                .find(|f| f.mime.starts_with("image/"))
            {
                let (w, h, thumb_key) = self
                    .blobs
                    .process_image(&img_fmt.data, &img_fmt.mime)
                    .context("process captured image")?;
                let short_fmt = img_fmt
                    .mime
                    .strip_prefix("image/")
                    .unwrap_or("img")
                    .to_ascii_uppercase();
                preview = format!("{short_fmt} · {w}×{h}");
                search_text = format!(
                    "{} {}x{} {}",
                    short_fmt.to_ascii_lowercase(),
                    w,
                    h,
                    raw.source_app.as_deref().unwrap_or("")
                )
                .trim()
                .to_string();
                image_meta = Some(ImageMeta {
                    width: w,
                    height: h,
                    original_mime: img_fmt.mime.clone(),
                    thumb_key: Some(thumb_key),
                });
            }
        }

        for fmt in &prepared.formats {
            let needs_blob =
                fmt.mime.starts_with("image/") || fmt.data.len() > config.inline_threshold_bytes;
            if needs_blob {
                let blob_key = self.blobs.write_blob(&fmt.data, &fmt.mime)?;
                stored_formats.push((fmt.mime.clone(), None, Some(blob_key), fmt.data.len() as i64));
            } else {
                stored_formats.push((
                    fmt.mime.clone(),
                    Some(fmt.data.clone()),
                    None,
                    fmt.data.len() as i64,
                ));
            }
        }

        let default_space = self.default_space_id()?;
        let total_bytes: i64 = prepared.formats.iter().map(|f| f.data.len() as i64).sum();

        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO clips (
                kind, content_hash, title, preview, search_text, meta,
                byte_size, space_id, pinned, selection, source_app,
                created_at, last_used_at, copy_count, expires_at
             ) VALUES (
                ?1, ?2, NULL, ?3, ?4, ?5,
                ?6, ?7, 0, ?8, ?9,
                ?10, ?10, 1, NULL
             )",
            params![
                prepared.kind.as_str(),
                prepared.content_hash.as_slice(),
                preview,
                search_text,
                prepared.meta,
                total_bytes,
                default_space,
                raw.selection.as_str(),
                raw.source_app,
                raw.captured_at_ms,
            ],
        )?;
        let clip_id = tx.last_insert_rowid();

        for (mime, inline_data, blob_key, byte_size) in &stored_formats {
            tx.execute(
                "INSERT INTO clip_formats (clip_id, mime, inline_data, blob_key, byte_size)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![clip_id, mime, inline_data, blob_key, byte_size],
            )?;
        }

        if let Some(ref im) = image_meta {
            tx.execute(
                "INSERT INTO images (clip_id, width, height, original_mime, thumb_key)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    clip_id,
                    i64::from(im.width),
                    i64::from(im.height),
                    im.original_mime,
                    im.thumb_key,
                ],
            )?;
        }

        let pruned_files = prune_retention_tx(&tx, default_space, config.max_history)?;
        tx.commit()?;

        // Unlink unreferenced blobs & thumbnails AFTER commit.
        self.cleanup_unreferenced_files(&pruned_files.0, &pruned_files.1)?;

        let rec = self
            .get_clip(clip_id)?
            .context("inserted clip missing after commit")?;
        debug!(
            id = rec.id,
            kind = rec.kind.as_str(),
            bytes = rec.byte_size,
            hash = %rec.short_hash(),
            "stored clip"
        );
        Ok(InsertOutcome::Inserted(rec))
    }

    /// Fetch a single clip's metadata by ID.
    pub fn get_clip(&self, id: i64) -> Result<Option<ClipRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.id, c.kind, c.content_hash, c.title, c.preview, c.search_text, c.meta,
                    c.byte_size, c.space_id, c.pinned, c.selection, c.source_app,
                    c.created_at, c.last_used_at, c.copy_count, c.expires_at,
                    i.width, i.height, i.original_mime, i.thumb_key
             FROM clips c
             LEFT JOIN images i ON i.clip_id = c.id
             WHERE c.id = ?1",
        )?;
        stmt.query_row(params![id], row_to_clip).optional().map_err(Into::into)
    }

    /// Load all stored MIME formats for a clip (reading external blobs from disk as needed).
    pub fn get_clip_formats(&self, id: i64) -> Result<Vec<RawFormat>> {
        let mut stmt = self.conn.prepare(
            "SELECT mime, inline_data, blob_key
             FROM clip_formats
             WHERE clip_id = ?1
             ORDER BY mime ASC",
        )?;
        let rows = stmt.query_map(params![id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<Vec<u8>>>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })?;

        let mut out = Vec::new();
        for row in rows {
            let (mime, inline, blob_key) = row?;
            let data = if let Some(bytes) = inline {
                bytes
            } else if let Some(key) = blob_key {
                self.blobs.read_blob(&key)?
            } else {
                continue;
            };
            out.push(RawFormat { mime, data });
        }
        Ok(out)
    }

    /// List or search clips using FTS5 trigram (for queries ≥ 3 chars) or `LIKE` (1–2 chars).
    pub fn list_clips(&self, filter: &ClipFilter) -> Result<Vec<ClipRecord>> {
        let limit = if filter.limit == 0 { 200 } else { filter.limit } as i64;
        let offset = filter.offset as i64;
        let query_trimmed = filter
            .query
            .as_deref()
            .map(str::trim)
            .filter(|q| !q.is_empty());

        let mut sql = String::from(
            "SELECT c.id, c.kind, c.content_hash, c.title, c.preview, c.search_text, c.meta,
                    c.byte_size, c.space_id, c.pinned, c.selection, c.source_app,
                    c.created_at, c.last_used_at, c.copy_count, c.expires_at,
                    i.width, i.height, i.original_mime, i.thumb_key
             FROM clips c
             LEFT JOIN images i ON i.clip_id = c.id",
        );

        let mut conditions: Vec<String> = Vec::new();
        let mut params_vec: Vec<rusqlite::types::Value> = Vec::new();

        if let Some(q) = query_trimmed {
            if q.chars().count() >= 3 {
                sql.push_str(" JOIN clips_fts f ON f.rowid = c.id");
                // Wrap in double quotes so arbitrary punctuation/code tokens do not trigger FTS syntax errors.
                let escaped = format!("\"{}\"", q.replace('"', "\"\""));
                params_vec.push(escaped.into());
                conditions.push(format!("clips_fts MATCH ?{}", params_vec.len()));
            } else {
                let pattern = format!("%{}%", escape_like(q));
                params_vec.push(pattern.into());
                let p = params_vec.len();
                conditions.push(format!(
                    "(c.preview LIKE ?{p} ESCAPE '\\' OR COALESCE(c.title, '') LIKE ?{p} ESCAPE '\\')"
                ));
            }
        }

        if let Some(kind) = filter.kind {
            params_vec.push(kind.as_str().to_string().into());
            conditions.push(format!("c.kind = ?{}", params_vec.len()));
        } else if !filter.kinds.is_empty() {
            let mut placeholders = Vec::with_capacity(filter.kinds.len());
            for k in &filter.kinds {
                params_vec.push(k.as_str().to_string().into());
                placeholders.push(format!("?{}", params_vec.len()));
            }
            conditions.push(format!("c.kind IN ({})", placeholders.join(",")));
        }
        if let Some(space_id) = filter.space_id {
            params_vec.push(space_id.into());
            conditions.push(format!("c.space_id = ?{}", params_vec.len()));
        }
        if filter.pinned_only {
            conditions.push("c.pinned = 1".to_string());
        }

        if !conditions.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&conditions.join(" AND "));
        }

        params_vec.push(limit.into());
        let p_limit = params_vec.len();
        params_vec.push(offset.into());
        let p_offset = params_vec.len();
        sql.push_str(&format!(
            " ORDER BY c.pinned DESC, c.last_used_at DESC, c.id DESC LIMIT ?{p_limit} OFFSET ?{p_offset}"
        ));

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params_vec), row_to_clip)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Pin or unpin a clip.
    pub fn set_pinned(&mut self, id: i64, pinned: bool) -> Result<bool> {
        let changed = self.conn.execute(
            "UPDATE clips SET pinned = ?1 WHERE id = ?2",
            params![i64::from(pinned), id],
        )?;
        Ok(changed > 0)
    }

    /// Move a clip to another Space.
    pub fn move_to_space(&mut self, id: i64, space_id: i64) -> Result<bool> {
        let changed = self.conn.execute(
            "UPDATE clips SET space_id = ?1 WHERE id = ?2",
            params![space_id, id],
        )?;
        Ok(changed > 0)
    }

    /// Update a clip's JSON metadata (e.g. manual syntax language override).
    pub fn update_clip_meta(&mut self, id: i64, meta: Option<&str>) -> Result<bool> {
        let changed = self.conn.execute(
            "UPDATE clips SET meta = ?1 WHERE id = ?2",
            params![meta, id],
        )?;
        Ok(changed > 0)
    }

    /// Delete a clip, remove unreferenced blobs/thumbnails, and truncate WAL.
    pub fn delete_clip(&mut self, id: i64) -> Result<bool> {
        let tx = self.conn.transaction()?;
        let (blob_keys, thumb_keys) = collect_clip_files_tx(&tx, &[id])?;
        let deleted = tx.execute("DELETE FROM clips WHERE id = ?1", params![id])?;
        tx.commit()?;

        if deleted > 0 {
            self.cleanup_unreferenced_files(&blob_keys, &thumb_keys)?;
            self.wal_checkpoint_truncate()?;
        }
        Ok(deleted > 0)
    }

    /// Clear unpinned clips (or all clips if `include_pinned` is true), clean blobs, and truncate WAL.
    pub fn clear_history(&mut self, include_pinned: bool) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let ids: Vec<i64> = {
            let sql = if include_pinned {
                "SELECT id FROM clips"
            } else {
                "SELECT id FROM clips WHERE pinned = 0"
            };
            let mut stmt = tx.prepare(sql)?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };

        if ids.is_empty() {
            return Ok(0);
        }

        let (blob_keys, thumb_keys) = collect_clip_files_tx(&tx, &ids)?;
        let deleted = if include_pinned {
            tx.execute("DELETE FROM clips", [])?
        } else {
            tx.execute("DELETE FROM clips WHERE pinned = 0", [])?
        };
        tx.commit()?;

        self.cleanup_unreferenced_files(&blob_keys, &thumb_keys)?;
        self.wal_checkpoint_truncate()?;
        Ok(deleted)
    }

    /// List all Paste Queue items in FIFO order (`ord ASC`).
    pub fn queue_list(&self) -> Result<Vec<QueueItemRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT q.id, q.clip_id, q.ord, q.added_at,
                    c.id, c.kind, c.content_hash, c.title, c.preview, c.search_text, c.meta,
                    c.byte_size, c.space_id, c.pinned, c.selection, c.source_app,
                    c.created_at, c.last_used_at, c.copy_count, c.expires_at,
                    i.width, i.height, i.original_mime, i.thumb_key
             FROM queue_items q
             JOIN clips c ON c.id = q.clip_id
             LEFT JOIN images i ON i.clip_id = c.id
             ORDER BY q.ord ASC, q.id ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            let kind_s: String = r.get(5)?;
            let hash_vec: Vec<u8> = r.get(6)?;
            let mut content_hash = [0u8; 32];
            if hash_vec.len() == 32 {
                content_hash.copy_from_slice(&hash_vec);
            }
            let sel_s: String = r.get(14)?;
            let img_w: Option<i64> = r.get(20)?;
            let image = match img_w {
                Some(w) => Some(ImageMeta {
                    width: u32::try_from(w).unwrap_or(0),
                    height: u32::try_from(r.get::<_, i64>(21)?).unwrap_or(0),
                    original_mime: r.get(22)?,
                    thumb_key: r.get(23)?,
                }),
                None => None,
            };
            Ok(QueueItemRecord {
                id: r.get(0)?,
                clip_id: r.get(1)?,
                ord: r.get(2)?,
                added_at: r.get(3)?,
                clip: ClipRecord {
                    id: r.get(4)?,
                    kind: ClipKind::from_db(&kind_s).unwrap_or(ClipKind::Text),
                    content_hash,
                    title: r.get(7)?,
                    preview: r.get(8)?,
                    search_text: r.get(9)?,
                    meta: r.get(10)?,
                    byte_size: r.get(11)?,
                    space_id: r.get(12)?,
                    pinned: r.get::<_, i64>(13)? != 0,
                    selection: SelectionKind::from_db(&sel_s).unwrap_or(SelectionKind::Clipboard),
                    source_app: r.get(15)?,
                    created_at: r.get(16)?,
                    last_used_at: r.get(17)?,
                    copy_count: r.get(18)?,
                    expires_at: r.get(19)?,
                    image,
                },
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn queue_count(&self) -> Result<usize> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM queue_items", [], |r| r.get(0))?;
        Ok(usize::try_from(count).unwrap_or(0))
    }

    pub fn queue_enqueue(&mut self, clip_id: i64) -> Result<i64> {
        let next_ord: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(ord), -1) + 1 FROM queue_items",
            [],
            |r| r.get(0),
        )?;
        self.conn.execute(
            "INSERT INTO queue_items (clip_id, ord, added_at) VALUES (?1, ?2, ?3)",
            params![clip_id, next_ord, now_unix_ms()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn queue_peek_head(&self) -> Result<Option<QueueItemRecord>> {
        Ok(self.queue_list()?.into_iter().next())
    }

    pub fn queue_remove(&mut self, queue_item_id: i64) -> Result<bool> {
        let tx = self.conn.transaction()?;
        let deleted = tx.execute("DELETE FROM queue_items WHERE id = ?1", params![queue_item_id])?;
        if deleted > 0 {
            renumber_queue_tx(&tx)?;
        }
        tx.commit()?;
        Ok(deleted > 0)
    }

    /// Move a queue item up (`delta = -1`) or down (`delta = 1`) and renumber `ord`.
    pub fn queue_move(&mut self, queue_item_id: i64, delta: i32) -> Result<bool> {
        let tx = self.conn.transaction()?;
        let mut ids: Vec<i64> = {
            let mut stmt = tx.prepare("SELECT id FROM queue_items ORDER BY ord ASC, id ASC")?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        let Some(pos) = ids.iter().position(|&id| id == queue_item_id) else {
            return Ok(false);
        };
        let target = (pos as isize + delta as isize).clamp(0, (ids.len().saturating_sub(1)) as isize)
            as usize;
        if target == pos {
            return Ok(false);
        }
        let item = ids.remove(pos);
        ids.insert(target, item);
        for (ord, id) in ids.iter().enumerate() {
            tx.execute(
                "UPDATE queue_items SET ord = ?1 WHERE id = ?2",
                params![ord as i64, id],
            )?;
        }
        tx.commit()?;
        Ok(true)
    }

    pub fn queue_clear(&mut self) -> Result<usize> {
        let deleted = self.conn.execute("DELETE FROM queue_items", [])?;
        Ok(deleted)
    }

    /// Sweep `blobs/` for files that are not referenced by any row in `clip_formats`.
    pub fn sweep_orphan_blobs(&self) -> Result<usize> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT blob_key FROM clip_formats WHERE blob_key IS NOT NULL")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut referenced = HashSet::new();
        for row in rows {
            referenced.insert(row?);
        }
        let removed = self.blobs.sweep_orphans(&referenced)?;
        if removed > 0 {
            warn!(removed, "swept orphan blob files");
        }
        Ok(removed)
    }

    fn cleanup_unreferenced_files(
        &self,
        candidate_blobs: &[String],
        candidate_thumbs: &[String],
    ) -> Result<()> {
        for key in candidate_blobs {
            let still_used: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM clip_formats WHERE blob_key = ?1",
                params![key],
                |r| r.get(0),
            )?;
            if still_used == 0 {
                let _ = self.blobs.delete_blob(key);
            }
        }
        for tkey in candidate_thumbs {
            let still_used: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM images WHERE thumb_key = ?1",
                params![tkey],
                |r| r.get(0),
            )?;
            if still_used == 0 {
                let _ = self.blobs.delete_thumb(tkey);
            }
        }
        Ok(())
    }
}

enum PreparedClip {
    Ready(PreparedData),
    Skip(&'static str),
}

struct PreparedData {
    kind: ClipKind,
    content_hash: [u8; 32],
    preview: String,
    search_text: String,
    meta: Option<String>,
    formats: Vec<RawFormat>,
}

fn prepare_clip(raw: &RawClip, config: &Config) -> PreparedClip {
    // 1. Files (`x-special/gnome-copied-files` or `text/uri-list`)
    if let Some(file_fmt) = raw
        .formats
        .iter()
        .find(|f| f.mime == "x-special/gnome-copied-files")
        .or_else(|| raw.formats.iter().find(|f| f.mime == "text/uri-list"))
    {
        if file_fmt.data.is_empty() {
            return PreparedClip::Skip("empty file list");
        }
        if file_fmt.data.len() > config.max_text_bytes {
            return PreparedClip::Skip("file list exceeds max_text_bytes");
        }
        let paths = parse_file_list(&file_fmt.mime, &file_fmt.data);
        if paths.is_empty() {
            return PreparedClip::Skip("no paths in file list");
        }
        let joined = paths.join("\n");
        let preview = if paths.len() == 1 {
            make_text_preview(&paths[0])
        } else {
            format!("{} files · {}", paths.len(), make_text_preview(&paths[0]))
        };
        let search_text = make_search_text(&joined);
        let hash = canonical_hash(ClipKind::Files, joined.as_bytes());
        let kept: Vec<RawFormat> = raw
            .formats
            .iter()
            .filter(|f| {
                f.mime == "x-special/gnome-copied-files"
                    || f.mime == "text/uri-list"
                    || f.mime == "text/plain;charset=utf-8"
            })
            .cloned()
            .collect();
        return PreparedClip::Ready(PreparedData {
            kind: ClipKind::Files,
            content_hash: hash,
            preview,
            search_text,
            meta: Some(serde_json::json!({ "count": paths.len() }).to_string()),
            formats: kept,
        });
    }

    // Extract best text and best image candidates for arbitration (§7.3 #4)
    let text_fmt = find_best_text(&raw.formats);
    let image_fmt = find_best_image(&raw.formats);

    let text_string = text_fmt.map(|f| decode_text(&f.mime, &f.data));
    let image_is_primary = match (&text_string, &image_fmt) {
        (None, Some(_)) => true,
        (Some(t), Some(_)) => is_url_or_filename_only(t),
        _ => false,
    };

    // 2. Image clip
    if image_is_primary {
        if !config.capture_images {
            return PreparedClip::Skip("image capture disabled");
        }
        let img = image_fmt.unwrap();
        if img.data.is_empty() {
            return PreparedClip::Skip("empty image");
        }
        if img.data.len() > config.max_image_bytes {
            return PreparedClip::Skip("image exceeds max_image_bytes");
        }
        let hash = canonical_hash(ClipKind::Image, &img.data);
        return PreparedClip::Ready(PreparedData {
            kind: ClipKind::Image,
            content_hash: hash,
            preview: String::new(),
            search_text: String::new(),
            meta: None,
            formats: vec![img.clone()],
        });
    }

    // 3. Text (+ optional HTML) clip
    if let (Some(tf), Some(text)) = (text_fmt, text_string) {
        if text.is_empty() {
            return PreparedClip::Skip("empty text");
        }
        if tf.data.len() > config.max_text_bytes {
            return PreparedClip::Skip("text exceeds max_text_bytes");
        }
        let mut formats = vec![RawFormat {
            mime: "text/plain;charset=utf-8".to_string(),
            data: text.as_bytes().to_vec(),
        }];
        if let Some(html) = raw.formats.iter().find(|f| f.mime == "text/html") {
            if !html.data.is_empty() && html.data.len() <= config.max_html_bytes {
                formats.push(html.clone());
            }
        }
        let det = pf_core::detect::classify_text(&text);
        let hash = canonical_hash(det.kind, text.as_bytes());
        return PreparedClip::Ready(PreparedData {
            kind: det.kind,
            content_hash: hash,
            preview: make_text_preview(&text),
            search_text: make_search_text(&text),
            meta: det.meta_json,
            formats,
        });
    }

    PreparedClip::Skip("no supported formats")
}

fn find_best_text(formats: &[RawFormat]) -> Option<&RawFormat> {
    const PREF: &[&str] = &[
        "UTF8_STRING",
        "text/plain;charset=utf-8",
        "text/plain",
        "STRING",
    ];
    for wanted in PREF {
        if let Some(f) = formats.iter().find(|f| f.mime == *wanted) {
            return Some(f);
        }
    }
    None
}

fn find_best_image(formats: &[RawFormat]) -> Option<&RawFormat> {
    const PREF: &[&str] = &[
        "image/png",
        "image/jpeg",
        "image/webp",
        "image/bmp",
        "image/gif",
    ];
    for wanted in PREF {
        if let Some(f) = formats.iter().find(|f| f.mime == *wanted) {
            return Some(f);
        }
    }
    None
}

fn decode_text(mime: &str, data: &[u8]) -> String {
    if mime == "STRING" {
        // ICCCM STRING is ISO-8859-1 (Latin-1)
        data.iter().map(|&b| char::from(b)).collect()
    } else {
        String::from_utf8_lossy(data).into_owned()
    }
}

fn prune_retention_tx(
    tx: &Transaction<'_>,
    default_space_id: i64,
    max_history: usize,
) -> Result<(Vec<String>, Vec<String>)> {
    let overflow_ids: Vec<i64> = {
        let mut stmt = tx.prepare(
            "SELECT id FROM clips
             WHERE space_id = ?1 AND pinned = 0
             ORDER BY last_used_at DESC, id DESC
             LIMIT -1 OFFSET ?2",
        )?;
        let rows = stmt.query_map(params![default_space_id, max_history as i64], |r| r.get(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };

    if overflow_ids.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }

    let files = collect_clip_files_tx(tx, &overflow_ids)?;
    for id in &overflow_ids {
        tx.execute("DELETE FROM clips WHERE id = ?1", params![id])?;
    }
    debug!(pruned = overflow_ids.len(), "pruned clips beyond max_history");
    Ok(files)
}

fn collect_clip_files_tx(
    tx: &Transaction<'_>,
    ids: &[i64],
) -> Result<(Vec<String>, Vec<String>)> {
    let mut blobs = Vec::new();
    let mut thumbs = Vec::new();
    let mut blob_stmt = tx.prepare(
        "SELECT blob_key FROM clip_formats WHERE clip_id = ?1 AND blob_key IS NOT NULL",
    )?;
    let mut thumb_stmt = tx.prepare(
        "SELECT thumb_key FROM images WHERE clip_id = ?1 AND thumb_key IS NOT NULL",
    )?;
    for &id in ids {
        let b_rows = blob_stmt.query_map(params![id], |r| r.get::<_, String>(0))?;
        for b in b_rows {
            blobs.push(b?);
        }
        let t_rows = thumb_stmt.query_map(params![id], |r| r.get::<_, String>(0))?;
        for t in t_rows {
            thumbs.push(t?);
        }
    }
    Ok((blobs, thumbs))
}

fn row_to_clip(r: &rusqlite::Row<'_>) -> rusqlite::Result<ClipRecord> {
    let kind_s: String = r.get(1)?;
    let hash_vec: Vec<u8> = r.get(2)?;
    let mut content_hash = [0u8; 32];
    if hash_vec.len() == 32 {
        content_hash.copy_from_slice(&hash_vec);
    }
    let sel_s: String = r.get(10)?;
    let img_w: Option<i64> = r.get(16)?;
    let image = match img_w {
        Some(w) => Some(ImageMeta {
            width: u32::try_from(w).unwrap_or(0),
            height: u32::try_from(r.get::<_, i64>(17)?).unwrap_or(0),
            original_mime: r.get(18)?,
            thumb_key: r.get(19)?,
        }),
        None => None,
    };

    Ok(ClipRecord {
        id: r.get(0)?,
        kind: ClipKind::from_db(&kind_s).unwrap_or(ClipKind::Text),
        content_hash,
        title: r.get(3)?,
        preview: r.get(4)?,
        search_text: r.get(5)?,
        meta: r.get(6)?,
        byte_size: r.get(7)?,
        space_id: r.get(8)?,
        pinned: r.get::<_, i64>(9)? != 0,
        selection: SelectionKind::from_db(&sel_s).unwrap_or(SelectionKind::Clipboard),
        source_app: r.get(11)?,
        created_at: r.get(12)?,
        last_used_at: r.get(13)?,
        copy_count: r.get(14)?,
        expires_at: r.get(15)?,
        image,
    })
}

fn renumber_queue_tx(tx: &Transaction<'_>) -> Result<()> {
    let ids: Vec<i64> = {
        let mut stmt = tx.prepare("SELECT id FROM queue_items ORDER BY ord ASC, id ASC")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    for (ord, id) in ids.iter().enumerate() {
        tx.execute(
            "UPDATE queue_items SET ord = ?1 WHERE id = ?2",
            params![ord as i64, id],
        )?;
    }
    Ok(())
}

fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

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
    use image::{ImageBuffer, Rgba};
    use std::io::Cursor;
    use tempfile::TempDir;

    fn test_store() -> (TempDir, Store) {
        let dir = TempDir::new().unwrap();
        let store = Store::open_at(
            &dir.path().join("pasteflow.db"),
            dir.path().join("blobs"),
            dir.path().join("thumbs"),
        )
        .unwrap();
        (dir, store)
    }

    fn raw_text(s: &str, ts: i64) -> RawClip {
        RawClip {
            selection: SelectionKind::Clipboard,
            formats: vec![RawFormat {
                mime: "UTF8_STRING".to_string(),
                data: s.as_bytes().to_vec(),
            }],
            source_app: Some("xed/Xed".to_string()),
            owner_pid: Some(1234),
            captured_at_ms: ts,
        }
    }

    fn make_png(w: u32, h: u32) -> Vec<u8> {
        let img = ImageBuffer::from_fn(w, h, |x, y| {
            Rgba([(x % 256) as u8, (y % 256) as u8, 128, 255])
        });
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn bundled_sqlite_supports_fts5_trigram_and_secure_delete() {
        let ver = verify_sqlite_capabilities().expect("FTS5 trigram + secure-delete");
        assert!(ver.starts_with("3."), "unexpected sqlite version: {ver}");
    }

    #[test]
    fn seeds_five_default_spaces() {
        let (_dir, store) = test_store();
        let spaces = store.list_spaces().unwrap();
        assert_eq!(spaces.len(), 5);
        assert_eq!(spaces[0].name, "General");
        assert!(spaces[0].is_default);
    }

    #[test]
    fn insert_dedupe_and_fts_search() {
        let (_dir, mut store) = test_store();
        let cfg = Config::default();

        let r1 = store.insert_raw_clip(&raw_text("fn main() { println!(\"hello\"); }", 1000), &cfg).unwrap();
        let id1 = match r1 {
            InsertOutcome::Inserted(c) => c.id,
            other => panic!("expected Inserted, got {other:?}"),
        };

        // Re-copying bumps copy_count and last_used_at
        let r2 = store.insert_raw_clip(&raw_text("fn main() { println!(\"hello\"); }", 2000), &cfg).unwrap();
        match r2 {
            InsertOutcome::Deduplicated(c) => {
                assert_eq!(c.id, id1);
                assert_eq!(c.copy_count, 2);
                assert_eq!(c.last_used_at, 2000);
            }
            other => panic!("expected Deduplicated, got {other:?}"),
        }

        // Trigram substring search (≥ 3 chars)
        let hits = store
            .list_clips(&ClipFilter {
                query: Some("println".to_string()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, id1);

        // Short 2-char search fallback
        let short_hits = store
            .list_clips(&ClipFilter {
                query: Some("fn".to_string()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(short_hits.len(), 1);
    }

    #[test]
    fn large_text_uses_blob_and_deletes_cleanly() {
        let (_dir, mut store) = test_store();
        let cfg = Config::default();
        let big = "x".repeat(300 * 1024); // > 256 KiB inline threshold
        let rec = match store.insert_raw_clip(&raw_text(&big, 1000), &cfg).unwrap() {
            InsertOutcome::Inserted(c) => c,
            other => panic!("expected Inserted, got {other:?}"),
        };

        let formats = store.get_clip_formats(rec.id).unwrap();
        assert_eq!(formats.len(), 1);
        assert_eq!(formats[0].data.len(), big.len());

        // Blob file exists on disk
        let blob_count_before: usize = walk_files(store.blobs().blobs_dir());
        assert_eq!(blob_count_before, 1);

        assert!(store.delete_clip(rec.id).unwrap());
        let blob_count_after: usize = walk_files(store.blobs().blobs_dir());
        assert_eq!(blob_count_after, 0);
        assert!(store.get_clip(rec.id).unwrap().is_none());
    }

    #[test]
    fn image_capture_stores_blob_and_thumbnail() {
        let (_dir, mut store) = test_store();
        let cfg = Config::default();
        let png = make_png(640, 480);
        let raw = RawClip {
            selection: SelectionKind::Clipboard,
            formats: vec![RawFormat {
                mime: "image/png".to_string(),
                data: png.clone(),
            }],
            source_app: Some("gimp".to_string()),
            owner_pid: None,
            captured_at_ms: 5000,
        };
        let rec = match store.insert_raw_clip(&raw, &cfg).unwrap() {
            InsertOutcome::Inserted(c) => c,
            other => panic!("expected Inserted, got {other:?}"),
        };
        assert_eq!(rec.kind, ClipKind::Image);
        let im = rec.image.as_ref().expect("image metadata");
        assert_eq!((im.width, im.height), (640, 480));
        assert!(rec.preview.contains("640×480"));

        let thumb = store
            .blobs()
            .thumbs_dir()
            .join(im.thumb_key.as_ref().unwrap());
        assert!(thumb.exists());

        let formats = store.get_clip_formats(rec.id).unwrap();
        assert_eq!(formats[0].data, png);
    }

    #[test]
    fn retention_preserves_pinned_and_other_spaces() {
        let (_dir, mut store) = test_store();
        let mut cfg = Config::default();
        cfg.max_history = 2;

        let c1 = match store.insert_raw_clip(&raw_text("pinned item", 100), &cfg).unwrap() {
            InsertOutcome::Inserted(c) => c,
            _ => unreachable!(),
        };
        store.set_pinned(c1.id, true).unwrap();

        let dev_space = store
            .list_spaces()
            .unwrap()
            .into_iter()
            .find(|s| s.name == "Development")
            .unwrap()
            .id;
        let c2 = match store.insert_raw_clip(&raw_text("saved in dev", 200), &cfg).unwrap() {
            InsertOutcome::Inserted(c) => c,
            _ => unreachable!(),
        };
        store.move_to_space(c2.id, dev_space).unwrap();

        // Add 3 unpinned items in General -> oldest unpinned ("u1") should be pruned
        let u1 = match store.insert_raw_clip(&raw_text("unpinned 1", 300), &cfg).unwrap() {
            InsertOutcome::Inserted(c) => c,
            _ => unreachable!(),
        };
        let _u2 = store.insert_raw_clip(&raw_text("unpinned 2", 400), &cfg).unwrap();
        let _u3 = store.insert_raw_clip(&raw_text("unpinned 3", 500), &cfg).unwrap();

        assert!(store.get_clip(c1.id).unwrap().is_some(), "pinned kept");
        assert!(store.get_clip(c2.id).unwrap().is_some(), "non-default space kept");
        assert!(store.get_clip(u1.id).unwrap().is_none(), "oldest unpinned pruned");
    }

    #[test]
    fn orphan_blob_sweep_removes_unreferenced_files() {
        let (_dir, store) = test_store();
        let orphan_key = store.blobs().write_blob(b"orphan data", "text/plain").unwrap();
        assert_eq!(walk_files(store.blobs().blobs_dir()), 1);
        let removed = store.sweep_orphan_blobs().unwrap();
        assert_eq!(removed, 1);
        assert!(!store.blobs().blobs_dir().join(orphan_key).exists());
    }

    #[test]
    fn queue_fifo_reorder_and_clear() {
        let (_dir, mut store) = test_store();
        let cfg = Config::default();
        let c1 = match store.insert_raw_clip(&raw_text("first item", 100), &cfg).unwrap() {
            InsertOutcome::Inserted(c) => c.id,
            _ => unreachable!(),
        };
        let c2 = match store.insert_raw_clip(&raw_text("second item", 200), &cfg).unwrap() {
            InsertOutcome::Inserted(c) => c.id,
            _ => unreachable!(),
        };
        let c3 = match store.insert_raw_clip(&raw_text("third item", 300), &cfg).unwrap() {
            InsertOutcome::Inserted(c) => c.id,
            _ => unreachable!(),
        };

        let q1 = store.queue_enqueue(c1).unwrap();
        let q2 = store.queue_enqueue(c2).unwrap();
        let q3 = store.queue_enqueue(c3).unwrap();
        assert_eq!(store.queue_count().unwrap(), 3);

        // Move q3 up by 1 -> [q1, q3, q2]
        assert!(store.queue_move(q3, -1).unwrap());
        let list = store.queue_list().unwrap();
        let order: Vec<i64> = list.iter().map(|i| i.id).collect();
        assert_eq!(order, vec![q1, q3, q2]);

        // Remove head q1 -> [q3, q2]
        assert!(store.queue_remove(q1).unwrap());
        let head = store.queue_peek_head().unwrap().unwrap();
        assert_eq!(head.id, q3);

        assert_eq!(store.queue_clear().unwrap(), 2);
        assert_eq!(store.queue_count().unwrap(), 0);
    }

    fn walk_files(dir: &Path) -> usize {
        let mut count = 0;
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    count += walk_files(&p);
                } else if p.is_file() {
                    count += 1;
                }
            }
        }
        count
    }
}
