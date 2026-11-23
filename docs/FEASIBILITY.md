# PasteFlow — M0 Technical Feasibility Report

Date: 2026-11-23  
Host: Linux Mint 22.1 Cinnamon (X11 session `DISPLAY=:0`), Mesa `llvmpipe` (LLVM 20.1.2, no NVIDIA driver loaded)  
Toolchain: `rustc 1.100.0`, `cargo 1.100.0`, system GTK `4.14.5`, GtkSourceView `5.12.0`

---

## Summary & Go/No-Go Decisions

| Spike | Area | Result | Decision |
|---|---|---|---|
| **S1** | XFixes probe (`pf-x11-probe`) | `XFixes 5.0`, `XTEST 2.2`, `CLIPBOARD_MANAGER=0x1200002` (`csd-clipboard`), `max_req=16,777,212 B` | **GO** |
| **S2** | 20 MB `INCR` transfer | Verified 20 MiB round-trip with BLAKE3 match on both live X11 (`0.53 s`) and `Xvfb` (`0.52 s`) | **GO** |
| **S3** | Selection serving & `csd-clipboard` | `csd-clipboard` confirmed active as `CLIPBOARD_MANAGER`; `TestSelectionOwner` + `pf-x11` reader/server verified | **GO** (cooperate via `SAVE_TARGETS`, never claim `CLIPBOARD_MANAGER`) |
| **S4** | Global `XGrabKey` + `XTEST` | `Super+V` and `Super+Shift+V` grabbed and ungrabbed on live Cinnamon root window with zero `BadAccess` conflicts; `XTEST` `Ctrl+V` verified under `Xvfb` | **GO** |
| **S5** | GTK4 overlay XID placement under Muffin | With `GSK_RENDERER=cairo`, `USPosition` + `_NET_WM_WINDOW_TYPE_UTILITY` + `ConfigureWindow(240,180)` placed window at exact `(240, 180)` | **GO** (keep center fallback) |
| **S6** | `GSK_RENDERER`: `cairo` vs `ngl` on `llvmpipe` | `cairo`: **138.1 MB RSS** (443.7 ms cold spike w/ 350 ms timer, exact pos); `ngl`: **188.6 MB RSS** (+50.5 MB, pos `(240, 0)`) | **GO — default `GSK_RENDERER=cairo`** |
| **S7** | `ksni` StatusNotifierItem tray | `S7_TRAY_OK` registered cleanly over session D-Bus with zero GTK3 | **GO** |
| **S8** | Password manager & conflicting managers | Running processes confirmed: `bitwarden-app` (Flatpak `--ozone-platform=x11`), `csd-clipboard`, `copyq` | **GO** — include `bitwarden-app` alongside `Bitwarden`, `KeePassXC`, `1Password` in default exclusions; warn when `copyq` is running |
| **S9** | Cinnamon Wayland & portals | `cinnamon-wayland.desktop` present; portals installed: `gtk`, `xapp`, `gnome-keyring`, `xapp-gnome-keyring` (no `GlobalShortcuts` or `RemoteDesktop` portal); `org.cinnamon.desktop.keybindings custom-list` = `['custom0']` | **GO (M10 degraded target)** — Cinnamon custom keybindings + manual queue + centered overlay |

---

## Verified Environment Findings Affecting M1+

1. **Default GTK Renderer (`cairo`):** `GSK_RENDERER=cairo` saves ~50 MB RSS over `ngl` on `llvmpipe` and positions undecorated utility overlays accurately under Muffin. `pf-app` will default to `cairo` before `gtk::init` unless overridden by the user.
2. **Bitwarden Exclusion Name:** Flatpak Bitwarden runs with `/proc/<pid>/comm` = `bitwarden-app`. Default exclusions must match both `bitwarden-app` and `Bitwarden` (case-insensitive) in addition to `x-kde-passwordManagerHint=secret`.
3. **CopyQ Active Conflict:** `copyq` (`--clipboard-access monitorClipboard`) is active on the host; it must be quit during manual Mint testing and flagged in Diagnostics.
4. **Cinnamon Keybindings:** Existing `custom0` entry must be preserved when writing PasteFlow custom keybindings.

---

## Automated Verification Commands

- `cargo test --workspace` (runs `pf-x11` S2 20 MB `INCR` and S4 `Super+V`/`Super+Shift+V` grab checks)
- `Xvfb :99 -screen 0 1280x720x24 & DISPLAY=:99 PASTEFLOW_XVFB_ACTIVE=1 cargo test -p pf-x11 --test m0_spikes` (verifies `INCR` + `XTEST` in isolation)
- `cargo run -p pf-x11 --bin pf-x11-probe -- --once`
- `GSK_RENDERER=cairo cargo run -p pf-app --bin pf-m0-spike`
- `cargo run -p pf-app --bin pf-m0-spike -- --tray`
