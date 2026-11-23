use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

const AUTOSTART_FILENAME: &str = "app.pasteflow.PasteFlow.desktop";

/// Resolve `~/.config/autostart/app.pasteflow.PasteFlow.desktop`
/// (honoring `PASTEFLOW_AUTOSTART_DIR` for isolated testing).
pub fn autostart_file_path() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("PASTEFLOW_AUTOSTART_DIR") {
        return Ok(PathBuf::from(dir).join(AUTOSTART_FILENAME));
    }
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))
        })
        .context("unable to resolve XDG config directory for autostart")?;
    Ok(config_home.join("autostart").join(AUTOSTART_FILENAME))
}

pub fn is_autostart_enabled() -> bool {
    autostart_file_path().map(|p| p.exists()).unwrap_or(false)
}

pub fn set_autostart_enabled(enabled: bool) -> Result<()> {
    let path = autostart_file_path()?;
    if enabled {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create autostart dir {}", parent.display()))?;
        }
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("pasteflow"));
        let entry = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=PasteFlow\n\
             Comment=Native Linux clipboard manager\n\
             Exec={} --background\n\
             Icon=edit-paste-symbolic\n\
             Terminal=false\n\
             Categories=Utility;\n\
             X-GNOME-Autostart-enabled=true\n",
            exe.display()
        );
        fs::write(&path, entry)
            .with_context(|| format!("write autostart file {}", path.display()))?;
    } else if path.exists() {
        fs::remove_file(&path)
            .with_context(|| format!("remove autostart file {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autostart_toggle_creates_and_removes_desktop_entry() {
        let tmp = std::env::temp_dir().join(format!("pf-autostart-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        // SAFETY: isolated test process with unique env value
        unsafe {
            std::env::set_var("PASTEFLOW_AUTOSTART_DIR", &tmp);
        }
        assert!(!is_autostart_enabled());
        set_autostart_enabled(true).unwrap();
        assert!(is_autostart_enabled());
        let content = fs::read_to_string(autostart_file_path().unwrap()).unwrap();
        assert!(content.contains("--background"));
        set_autostart_enabled(false).unwrap();
        assert!(!is_autostart_enabled());
        let _ = fs::remove_dir_all(&tmp);
    }
}
