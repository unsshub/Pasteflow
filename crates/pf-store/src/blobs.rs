use anyhow::{Context, Result};
use image::imageops::FilterType;
use image::{ImageFormat, ImageReader, Limits};
use pf_core::config::create_private_dir;
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{Cursor, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use tracing::debug;

const THUMB_MAX_DIM: u32 = 256;

/// Map a MIME type to a clean file extension for blob storage.
pub fn mime_to_ext(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/webp" => "webp",
        "image/bmp" | "image/x-bmp" => "bmp",
        "image/gif" => "gif",
        "text/html" => "html",
        "text/uri-list" | "x-special/gnome-copied-files" => "uris",
        _ => "txt",
    }
}

/// Content-addressed blob and thumbnail store.
#[derive(Debug, Clone)]
pub struct BlobStore {
    blobs_dir: PathBuf,
    thumbs_dir: PathBuf,
}

impl BlobStore {
    pub fn new(blobs_dir: PathBuf, thumbs_dir: PathBuf) -> Result<Self> {
        create_private_dir(&blobs_dir)?;
        create_private_dir(&thumbs_dir)?;
        Ok(Self {
            blobs_dir,
            thumbs_dir,
        })
    }

    pub fn blobs_dir(&self) -> &Path {
        &self.blobs_dir
    }

    pub fn thumbs_dir(&self) -> &Path {
        &self.thumbs_dir
    }

    /// Write raw bytes to `blobs/<hash[0..2]>/<hash>.<ext>` atomically (`tmp -> fsync -> rename`).
    /// Returns the relative `blob_key` (e.g. `ab/abcdef...png`).
    pub fn write_blob(&self, data: &[u8], mime: &str) -> Result<String> {
        let hash = blake3::hash(data).to_hex();
        let prefix = &hash[..2];
        let ext = mime_to_ext(mime);
        let rel_key = format!("{prefix}/{hash}.{ext}");
        let sub_dir = self.blobs_dir.join(prefix);
        create_private_dir(&sub_dir)?;

        let final_path = self.blobs_dir.join(&rel_key);
        if final_path.exists() {
            return Ok(rel_key);
        }

        let tmp_path = sub_dir.join(format!(".{hash}.{ext}.tmp.{}", std::process::id()));
        write_atomic(&tmp_path, &final_path, data)?;
        debug!(blob_key = %rel_key, bytes = data.len(), "wrote blob");
        Ok(rel_key)
    }

    /// Read a blob by its relative `blob_key`.
    pub fn read_blob(&self, blob_key: &str) -> Result<Vec<u8>> {
        let path = self.blobs_dir.join(blob_key);
        fs::read(&path).with_context(|| format!("read blob {}", path.display()))
    }

    /// Best-effort overwrite with zeros, `fsync`, and `unlink`.
    pub fn delete_blob(&self, blob_key: &str) -> Result<()> {
        let path = self.blobs_dir.join(blob_key);
        secure_unlink(&path)
    }

    /// Decode an image safely (with memory/dimension limits), return `(width, height)`,
    /// and write a ≤ 256 px PNG thumbnail to `thumbs/<hash>-256.png`.
    pub fn process_image(&self, data: &[u8], mime: &str) -> Result<(u32, u32, String)> {
        let mut reader = ImageReader::new(Cursor::new(data));
        if let Some(fmt) = ImageFormat::from_mime_type(mime) {
            reader.set_format(fmt);
        } else {
            reader = reader.with_guessed_format().context("guess image format")?;
        }

        let mut limits = Limits::default();
        limits.max_image_width = Some(16_384);
        limits.max_image_height = Some(16_384);
        limits.max_alloc = Some(256 * 1024 * 1024);
        reader.limits(limits);

        let img = reader.decode().context("decode image")?;
        let width = img.width();
        let height = img.height();

        let hash = blake3::hash(data).to_hex();
        let thumb_key = format!("{hash}-{THUMB_MAX_DIM}.png");
        let thumb_path = self.thumbs_dir.join(&thumb_key);

        if !thumb_path.exists() {
            let thumb = if width > THUMB_MAX_DIM || height > THUMB_MAX_DIM {
                img.resize(THUMB_MAX_DIM, THUMB_MAX_DIM, FilterType::Triangle)
            } else {
                img
            };
            let mut png_bytes = Vec::new();
            thumb
                .write_to(&mut Cursor::new(&mut png_bytes), ImageFormat::Png)
                .context("encode PNG thumbnail")?;
            let tmp = self
                .thumbs_dir
                .join(format!(".{thumb_key}.tmp.{}", std::process::id()));
            write_atomic(&tmp, &thumb_path, &png_bytes)?;
        }

        Ok((width, height, thumb_key))
    }

    /// Remove a thumbnail file from cache if present.
    pub fn delete_thumb(&self, thumb_key: &str) -> Result<()> {
        let path = self.thumbs_dir.join(thumb_key);
        if path.exists() {
            let _ = fs::remove_file(&path);
        }
        Ok(())
    }

    /// Delete any files under `blobs/` whose relative key is not in `referenced_keys`.
    /// Also removes stale `.tmp.*` files left behind by a crash.
    pub fn sweep_orphans(&self, referenced_keys: &HashSet<String>) -> Result<usize> {
        let mut removed = 0usize;
        if !self.blobs_dir.exists() {
            return Ok(0);
        }
        for entry in fs::read_dir(&self.blobs_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                let prefix = entry.file_name().to_string_lossy().into_owned();
                for sub in fs::read_dir(&path)? {
                    let sub = sub?;
                    let sub_path = sub.path();
                    if !sub_path.is_file() {
                        continue;
                    }
                    let fname = sub.file_name().to_string_lossy().into_owned();
                    let rel_key = format!("{prefix}/{fname}");
                    if fname.starts_with('.') || !referenced_keys.contains(&rel_key) {
                        let _ = secure_unlink(&sub_path);
                        removed += 1;
                    }
                }
            }
        }
        Ok(removed)
    }
}

fn write_atomic(tmp_path: &Path, final_path: &Path, data: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(tmp_path)
        .with_context(|| format!("create temp blob {}", tmp_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = file.set_permissions(fs::Permissions::from_mode(0o600));
    }

    file.write_all(data)
        .with_context(|| format!("write temp blob {}", tmp_path.display()))?;
    file.sync_all()
        .with_context(|| format!("fsync temp blob {}", tmp_path.display()))?;
    drop(file);

    fs::rename(tmp_path, final_path).with_context(|| {
        format!(
            "rename {} -> {}",
            tmp_path.display(),
            final_path.display()
        )
    })?;
    Ok(())
}

fn secure_unlink(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    if let Ok(meta) = fs::metadata(path) {
        let len = meta.len();
        if len > 0 && len <= 64 * 1024 * 1024 {
            if let Ok(mut f) = OpenOptions::new().write(true).open(path) {
                let _ = f.seek(SeekFrom::Start(0));
                let zeros = [0u8; 64 * 1024];
                let mut remaining = len as usize;
                while remaining > 0 {
                    let chunk = remaining.min(zeros.len());
                    if f.write_all(&zeros[..chunk]).is_err() {
                        break;
                    }
                    remaining -= chunk;
                }
                let _ = f.sync_all();
            }
        }
    }
    fs::remove_file(path).with_context(|| format!("unlink {}", path.display()))?;
    Ok(())
}
