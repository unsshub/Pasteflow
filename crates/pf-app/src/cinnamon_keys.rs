//! Cinnamon custom keybinding provider (`org.cinnamon.desktop.keybindings`).
//! Preserves all existing entries (such as `custom0`) when installing/removing PasteFlow bindings.

use anyhow::{Context, Result};
use std::process::Command;
use tracing::{info, warn};

const SCHEMA_ROOT: &str = "org.cinnamon.desktop.keybindings";
const SCHEMA_ENTRY: &str = "org.cinnamon.desktop.keybindings.custom-keybinding";
const BASE_PATH: &str = "/org/cinnamon/desktop/keybindings/custom-keybindings";

const ID_TOGGLE: &str = "pasteflow-toggle";
const ID_PASTE_NEXT: &str = "pasteflow-paste-next";

/// Parse a GVariant string array like `['custom0', 'pasteflow-toggle']` or `@as []`.
pub fn parse_gsettings_list(raw: &str) -> Vec<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "@as []" || trimmed == "[]" {
        return Vec::new();
    }
    let inner = trimmed
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim();
    if inner.is_empty() {
        return Vec::new();
    }
    inner
        .split(',')
        .map(|part| part.trim().trim_matches('\'').trim_matches('"').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Format a list of entry IDs as a GVariant string array for `gsettings set`.
pub fn format_gsettings_list(items: &[String]) -> String {
    if items.is_empty() {
        return "@as []".to_string();
    }
    let quoted: Vec<String> = items.iter().map(|s| format!("'{s}'")).collect();
    format!("[{}]", quoted.join(", "))
}

/// Merge PasteFlow entry IDs into an existing `custom-list`, preserving all other entries.
pub fn merge_custom_list(existing: &[String], enable: bool) -> Vec<String> {
    let mut out: Vec<String> = existing
        .iter()
        .filter(|id| id.as_str() != ID_TOGGLE && id.as_str() != ID_PASTE_NEXT)
        .cloned()
        .collect();
    if enable {
        out.push(ID_TOGGLE.to_string());
        out.push(ID_PASTE_NEXT.to_string());
    }
    out
}

/// Install or remove PasteFlow's Cinnamon custom keybindings via `gsettings`.
pub fn sync_cinnamon_keybindings(
    enable: bool,
    overlay_accel: &str,
    paste_next_accel: &str,
) -> Result<()> {
    let out = Command::new("gsettings")
        .args(["get", SCHEMA_ROOT, "custom-list"])
        .output()
        .context("run gsettings get custom-list")?;
    if !out.status.success() {
        warn!("Cinnamon keybindings schema not available on this desktop");
        return Ok(());
    }

    let current = parse_gsettings_list(&String::from_utf8_lossy(&out.stdout));
    let updated = merge_custom_list(&current, enable);

    if enable {
        let exe = std::env::current_exe()
            .ok()
            .and_then(|p| p.to_str().map(str::to_string))
            .unwrap_or_else(|| "pasteflow".to_string());

        set_entry(
            ID_TOGGLE,
            "PasteFlow: Toggle Overlay",
            &format!("{exe} --toggle"),
            overlay_accel,
        )?;
        set_entry(
            ID_PASTE_NEXT,
            "PasteFlow: Paste Next",
            &format!("{exe} --paste-next"),
            paste_next_accel,
        )?;
    }

    let formatted = format_gsettings_list(&updated);
    let status = Command::new("gsettings")
        .args(["set", SCHEMA_ROOT, "custom-list", &formatted])
        .status()
        .context("run gsettings set custom-list")?;
    if !status.success() {
        warn!("gsettings set custom-list exited with non-zero status");
    } else {
        info!(enable, "synced Cinnamon custom keybindings");
    }
    Ok(())
}

fn set_entry(id: &str, name: &str, command: &str, binding: &str) -> Result<()> {
    let reloc = format!("{SCHEMA_ENTRY}:{BASE_PATH}/{id}/");
    let binding_arr = format!("['{binding}']");
    for (key, val) in [
        ("name", name),
        ("command", command),
        ("binding", binding_arr.as_str()),
    ] {
        let _ = Command::new("gsettings")
            .args(["set", &reloc, key, val])
            .status()
            .with_context(|| format!("gsettings set {reloc} {key}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_existing_custom0_when_enabling_and_disabling() {
        let existing = parse_gsettings_list("['custom0']");
        assert_eq!(existing, vec!["custom0"]);

        let enabled = merge_custom_list(&existing, true);
        assert_eq!(
            enabled,
            vec!["custom0", "pasteflow-toggle", "pasteflow-paste-next"]
        );
        assert_eq!(
            format_gsettings_list(&enabled),
            "['custom0', 'pasteflow-toggle', 'pasteflow-paste-next']"
        );

        let disabled = merge_custom_list(&enabled, false);
        assert_eq!(disabled, vec!["custom0"]);
        assert_eq!(format_gsettings_list(&disabled), "['custom0']");
    }

    #[test]
    fn parses_empty_gvariant_array() {
        assert!(parse_gsettings_list("@as []").is_empty());
        assert_eq!(format_gsettings_list(&[]), "@as []");
    }
}
