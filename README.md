# PasteFlow

A fast, private, offline-first native Linux clipboard manager built with **Rust**, **GTK4**, **SQLite (WAL + FTS5 trigram)**, and **x11rb**, designed for **Linux Mint Cinnamon (X11)**.

## Features

- **Zero-polling X11 clipboard engine (`pf-x11`)**: event-driven XFixes monitoring, full ICCCM direct and `INCR` transfers (verified up to 20 MiB), `SAVE_TARGETS` hand-off to `csd-clipboard` on exit, and an automatic empty-clipboard guard.
- **Local SQLite + content-addressed blob storage (`pf-store`)**: WAL mode, FTS5 trigram substring search, automatic deduplication, configurable history cap (preserving pinned clips and non-default Spaces), bounded image decoding with 256 px thumbnails, `VACUUM INTO` rotating backups, and automatic corruption recovery.
- **Smart content recognition & rich detail panel (`F9`)**: detects URLs, Colors (`#HEX` / `rgb()` / `hsl()` with 1-click conversions), Emails, Phones, Numbers, Code (13+ languages with native `GtkSourceView 5` syntax highlighting and manual override), Images (Save to Downloads + Open in Viewer), Files, and secret-looking tokens.
- **Floating Quick Overlay (`Super+V`)**: opens at the mouse pointer with instant search, `↑`/`↓` navigation, `Enter` copy, `Shift+Enter` copy & paste, `Ctrl+1..9` quick pick, `Tab` type filter cycling, `Ctrl+P` pin, `Ctrl+Q` queue, and `Esc` focus restoration.
- **Paste Queue & Universal Paste (`Super+Shift+V` / `Ctrl+Shift+U`)**: FIFO sequential pasting with `XTEST` per-app chords (`Ctrl+V` default, `Ctrl+Shift+V` for terminals), `SelectionRequest` confirmation, rapid-press buffering, and manual-paste fallback.
- **Spaces & organization**: custom color/icon Spaces, drag-and-drop from clip list to sidebar Spaces, `Ctrl+M` Move to Space, and `F2` clip labeling.
- **Privacy first**: zero network crates, content-free logs, `x-kde-passwordManagerHint=secret` early abort, app exclusions (`bitwarden-app`, `Bitwarden`, `KeePassXC`, `1Password`), and RAM-only `zeroize` storage for sensitive clips and ≤ 1 h expiry timers.

## Default Shortcuts

| Action | Default | Pastily Preset |
|---|---|---|
| Open floating overlay | `Super+V` | `Ctrl+Shift+B` |
| Paste next queued item | `Super+Shift+V` | `Ctrl+Shift+V` |
| Search (in window) | `Ctrl+F` | — |
| Toggle detail panel | `F9` | — |
| Add to Paste Queue | `Ctrl+Q` | — |
| Toggle Universal Paste | `Ctrl+Shift+U` | — |
| Move clip to Space | `Ctrl+M` | — |
| Rename / label clip | `F2` | — |

## Building & Running

### System Dependencies (Linux Mint 22.x / Ubuntu 24.04)

```bash
sudo apt install build-essential pkg-config libgtk-4-dev libgtksourceview-5-dev
```

### Build & Run

```bash
cargo build --release -p pf-app --bin pasteflow
./target/release/pasteflow --background   # Start resident in system tray
./target/release/pasteflow                # Open main window
./target/release/pasteflow --toggle       # Toggle floating overlay
./target/release/pasteflow --headless     # Run headless X11 daemon
./target/release/pasteflow list           # CLI history list
./target/release/pasteflow search <query> # CLI FTS5 search
```

> **Note**: If another clipboard manager such as `copyq` is running, quit it to avoid X11 clipboard ownership conflicts. PasteFlow's **Settings → Advanced** diagnostics panel checks for conflicting managers automatically.

## Testing

```bash
# Unit tests
cargo test --workspace

# Full X11 integration tests in isolated Xvfb
Xvfb :99 -screen 0 1280x720x24 &
DISPLAY=:99 PASTEFLOW_XVFB_ACTIVE=1 cargo test -p pf-x11 --test m1_engine -- --test-threads=1
DISPLAY=:99 PASTEFLOW_XVFB_ACTIVE=1 cargo test -p pf-x11 --test m3_hotkeys -- --test-threads=1

# Verify zero network crates
cargo tree -e normal | grep -E 'reqwest|hyper|ureq' || echo "OK"
```

## Packaging

- **Debian/Mint `.deb`**: `cargo install cargo-deb && cargo deb -p pf-app`
- **AppImage**: `./packaging/appimage/build-appimage.sh`

## License

GPL-3.0-or-later
