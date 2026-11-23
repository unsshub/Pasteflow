# PasteFlow — Wayland Compatibility & Degraded Mode

PasteFlow's primary target is **Linux Mint Cinnamon on X11**, where XFixes, ICCCM/INCR, `XGrabKey`, `XTEST`, and XID overlay positioning provide full functionality.

## Verified Cinnamon Wayland Probe Results (M0 S9)

Checked on Linux Mint 22.1 Cinnamon (`cinnamon-wayland.desktop` installed):
- **Installed XDG Desktop Portals**: `gtk`, `xapp`, `gnome-keyring`, `xapp-gnome-keyring`
- **`GlobalShortcuts` portal**: Not available
- **`RemoteDesktop` portal**: Not available
- **Cinnamon custom keybindings (`gsettings`)**: Available (`org.cinnamon.desktop.keybindings custom-list`), works on both X11 and Wayland sessions.

## Feature Matrix: X11 vs. Cinnamon Wayland

| Capability | X11 (Primary) | Cinnamon Wayland (Degraded Mode) |
|---|---|---|
| Background clipboard monitoring | Event-driven XFixes (`pf-x11`) | Limited to XWayland bridge when running under `GDK_BACKEND=x11`; native Wayland background capture requires `ext-data-control-v1` / `wlr-data-control` compositor support |
| Global shortcuts | In-process `XGrabKey` (default) or Cinnamon custom keybindings | **Cinnamon custom keybindings** (`Settings → Appearance → Configure Global Shortcuts → Use Cinnamon custom keybindings`), invoking `pasteflow --toggle` and `pasteflow --paste-next` |
| Sequential Paste Queue | Automatic `XTEST` (`Ctrl+V` / `Ctrl+Shift+V` for terminals) with `SelectionRequest` confirmation | **Automatically switches to Manual Mode** when `XDG_SESSION_TYPE=wayland`: hotkey loads the next clip and shows a notification; user presses `Ctrl+V` |
| Floating overlay position | At mouse pointer (clamped to screen via XID) | **Centered** by compositor |
| System tray & notifications | StatusNotifierItem (`ksni`) + `GNotification` | Full support (D-Bus) |
| Local SQLite / FTS5 / Spaces / Privacy | Full support | Full support |

## Recommended Wayland Setup

1. Open **PasteFlow → Settings → Appearance → Configure Global Shortcuts**.
2. Enable **Use Cinnamon custom keybindings instead of X11 grab** and click **Apply**.
3. Use `Super+V` to open the centered quick overlay and `Super+Shift+V` to load the next Paste Queue item in Manual Mode.
