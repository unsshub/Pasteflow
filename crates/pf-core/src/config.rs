use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// XDG paths for PasteFlow runtime storage.
#[derive(Debug, Clone)]
pub struct AppPaths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub state_dir: PathBuf,
}

impl AppPaths {
    /// Resolve XDG directories, honoring `PASTEFLOW_*_DIR` overrides for testing/isolation.
    pub fn resolve() -> Result<Self> {
        let proj = ProjectDirs::from("app", "pasteflow", "PasteFlow")
            .context("unable to determine XDG project directories")?;

        let config_dir = std::env::var_os("PASTEFLOW_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| proj.config_dir().to_path_buf());
        let data_dir = std::env::var_os("PASTEFLOW_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| proj.data_dir().to_path_buf());
        let cache_dir = std::env::var_os("PASTEFLOW_CACHE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| proj.cache_dir().to_path_buf());
        let state_dir = std::env::var_os("PASTEFLOW_STATE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                proj.state_dir()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| data_dir.join("state"))
            });

        Ok(Self {
            config_dir,
            data_dir,
            cache_dir,
            state_dir,
        })
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    pub fn db_file(&self) -> PathBuf {
        self.data_dir.join("pasteflow.db")
    }

    pub fn blobs_dir(&self) -> PathBuf {
        self.data_dir.join("blobs")
    }

    pub fn backups_dir(&self) -> PathBuf {
        self.data_dir.join("backups")
    }

    pub fn thumbs_dir(&self) -> PathBuf {
        self.cache_dir.join("thumbs")
    }

    /// Create all runtime directories with `0700` permissions on Unix.
    pub fn ensure_dirs(&self) -> Result<()> {
        for dir in [
            &self.config_dir,
            &self.data_dir,
            &self.cache_dir,
            &self.state_dir,
            &self.blobs_dir(),
            &self.backups_dir(),
            &self.thumbs_dir(),
        ] {
            create_private_dir(dir)?;
        }
        Ok(())
    }
}

pub fn create_private_dir(path: &std::path::Path) -> Result<()> {
    fs::create_dir_all(path).with_context(|| format!("create dir {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

/// Parsed keyboard shortcut (GTK/Cinnamon accelerator format, e.g. `<Super>v`, `<Primary><Shift>b`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutSpec {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub super_key: bool,
    /// Lowercase ASCII/X11 keysym name (e.g. `"v"`, `"b"`, `"space"`).
    pub key: String,
}

impl ShortcutSpec {
    /// Parse a GTK/Cinnamon accelerator string like `<Super>v` or `<Primary><Shift>v`.
    pub fn parse(input: &str) -> Option<Self> {
        let mut s = input.trim();
        if s.is_empty() {
            return None;
        }
        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        let mut super_key = false;

        while s.starts_with('<') {
            let end = s.find('>')?;
            let token = &s[1..end];
            match token.to_ascii_lowercase().as_str() {
                "control" | "ctrl" | "primary" => ctrl = true,
                "alt" | "mod1" => alt = true,
                "shift" => shift = true,
                "super" | "mod4" | "meta" => super_key = true,
                _ => return None,
            }
            s = &s[end + 1..];
        }

        let key = s.trim().to_ascii_lowercase();
        if key.is_empty() {
            return None;
        }
        Some(Self {
            ctrl,
            alt,
            shift,
            super_key,
            key,
        })
    }

    /// Format back into canonical Cinnamon/GTK accelerator syntax.
    pub fn to_accel_string(&self) -> String {
        let mut out = String::new();
        if self.super_key {
            out.push_str("<Super>");
        }
        if self.ctrl {
            out.push_str("<Primary>");
        }
        if self.alt {
            out.push_str("<Alt>");
        }
        if self.shift {
            out.push_str("<Shift>");
        }
        out.push_str(&self.key);
        out
    }

    /// Map `self.key` to an X11 keysym value (supports ASCII letters/digits and common keys).
    pub fn x11_keysym(&self) -> Option<u32> {
        if self.key.len() == 1 {
            let ch = self.key.chars().next()?;
            if ch.is_ascii_alphanumeric() {
                return Some(ch.to_ascii_lowercase() as u32);
            }
        }
        match self.key.as_str() {
            "space" => Some(0x0020),
            "return" | "enter" => Some(0xff0d),
            "tab" => Some(0xff09),
            "escape" | "esc" => Some(0xff1b),
            "backspace" => Some(0xff08),
            "insert" => Some(0xff63),
            "delete" => Some(0xffff),
            _ => None,
        }
    }
}

/// Persistent PasteFlow configuration (`~/.config/pasteflow/config.toml`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub max_history: usize,
    pub max_text_bytes: usize,
    pub max_html_bytes: usize,
    pub max_image_bytes: usize,
    pub inline_threshold_bytes: usize,
    pub capture_images: bool,
    pub capture_primary: bool,
    pub ignore_duplicates: bool,
    pub empty_clipboard_guard: bool,
    pub paused: bool,
    pub excluded_apps: Vec<String>,
    pub overlay_shortcut: String,
    pub paste_next_shortcut: String,
    pub use_cinnamon_keybindings: bool,
    pub overlay_at_pointer: bool,
    pub terminal_paste_classes: Vec<String>,
    pub detect_secrets: bool,
    pub secret_expiry_secs: u64,
    pub default_expiry_secs: Option<u64>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_history: 2_000,
            max_text_bytes: 8 * 1024 * 1024,
            max_html_bytes: 2 * 1024 * 1024,
            max_image_bytes: 64 * 1024 * 1024,
            inline_threshold_bytes: 256 * 1024,
            capture_images: true,
            capture_primary: false,
            ignore_duplicates: true,
            empty_clipboard_guard: true,
            paused: false,
            excluded_apps: vec![
                "bitwarden-app".to_string(),
                "Bitwarden".to_string(),
                "KeePassXC".to_string(),
                "1Password".to_string(),
            ],
            overlay_shortcut: "<Super>v".to_string(),
            paste_next_shortcut: "<Super><Shift>v".to_string(),
            use_cinnamon_keybindings: false,
            overlay_at_pointer: true,
            terminal_paste_classes: vec![
                "gnome-terminal-server".to_string(),
                "kitty".to_string(),
                "Alacritty".to_string(),
                "xfce4-terminal".to_string(),
                "tilix".to_string(),
                "xterm".to_string(),
                "konsole".to_string(),
            ],
            detect_secrets: true,
            secret_expiry_secs: 30,
            default_expiry_secs: None,
        }
    }
}

impl Config {
    pub fn apply_default_shortcuts(&mut self) {
        self.overlay_shortcut = "<Super>v".to_string();
        self.paste_next_shortcut = "<Super><Shift>v".to_string();
    }

    pub fn apply_pastily_shortcuts(&mut self) {
        self.overlay_shortcut = "<Primary><Shift>b".to_string();
        self.paste_next_shortcut = "<Primary><Shift>v".to_string();
    }

    /// Check if `wm_class` belongs to a terminal that pastes with `Ctrl+Shift+V`.
    pub fn uses_shift_paste(&self, wm_class: Option<&str>) -> bool {
        let Some(cls) = wm_class else {
            return false;
        };
        let lower = cls.to_ascii_lowercase();
        self.terminal_paste_classes
            .iter()
            .any(|t| !t.is_empty() && lower.contains(&t.to_ascii_lowercase()))
    }
    pub fn load_or_default(paths: &AppPaths) -> Result<Self> {
        let file = paths.config_file();
        if !file.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(&file)
            .with_context(|| format!("read config {}", file.display()))?;
        let cfg: Config =
            toml::from_str(&raw).with_context(|| format!("parse config {}", file.display()))?;
        Ok(cfg)
    }

    pub fn save(&self, paths: &AppPaths) -> Result<()> {
        create_private_dir(&paths.config_dir)?;
        let file = paths.config_file();
        let data = toml::to_string_pretty(self).context("serialize config")?;
        fs::write(&file, data).with_context(|| format!("write config {}", file.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&file, fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    /// Case-insensitive check whether `source_app` (WM_CLASS or `/proc/<pid>/comm`) matches
    /// any configured excluded application.
    pub fn is_excluded(&self, source_app: Option<&str>) -> bool {
        let Some(src) = source_app else {
            return false;
        };
        if src.is_empty() {
            return false;
        }
        let parts: Vec<String> = src
            .split('/')
            .map(|p| p.trim().to_ascii_lowercase())
            .filter(|p| !p.is_empty())
            .collect();

        for excl in &self.excluded_apps {
            let e = excl.trim().to_ascii_lowercase();
            if e.is_empty() {
                continue;
            }
            if parts.iter().any(|p| p == &e || p.contains(&e)) {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_exclusions_match_bitwarden_and_keepassxc() {
        let cfg = Config::default();
        assert!(cfg.is_excluded(Some("bitwarden-app")));
        assert!(cfg.is_excluded(Some("Bitwarden/Bitwarden")));
        assert!(cfg.is_excluded(Some("keepassxc/KeePassXC")));
        assert!(cfg.is_excluded(Some("1password")));
        assert!(!cfg.is_excluded(Some("firefox/Navigator")));
        assert!(!cfg.is_excluded(None));
    }

    #[test]
    fn config_roundtrip_toml() {
        let cfg = Config::default();
        let s = toml::to_string_pretty(&cfg).unwrap();
        let parsed: Config = toml::from_str(&s).unwrap();
        assert_eq!(parsed.max_history, 2_000);
        assert_eq!(parsed.excluded_apps.len(), 4);
        assert_eq!(parsed.overlay_shortcut, "<Super>v");
    }

    #[test]
    fn shortcut_spec_parse_and_format() {
        let s = ShortcutSpec::parse("<Super><Shift>v").unwrap();
        assert!(s.super_key && s.shift && !s.ctrl && !s.alt);
        assert_eq!(s.key, "v");
        assert_eq!(s.x11_keysym(), Some(0x0076));
        assert_eq!(s.to_accel_string(), "<Super><Shift>v");

        let p = ShortcutSpec::parse("<Primary><Shift>B").unwrap();
        assert!(p.ctrl && p.shift && !p.super_key);
        assert_eq!(p.key, "b");
        assert_eq!(p.to_accel_string(), "<Primary><Shift>b");
    }
}
