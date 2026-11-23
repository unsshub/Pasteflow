# PasteFlow — Product & Technical Specification

> **Status / Triage Label:** `ready-for-agent`  
> **Source:** Synthesized from the approved technical planning document for PasteFlow (native Linux clipboard manager targeting Linux Mint 22.1 Cinnamon on X11, inspired functionally by Pastily).

---

## Problem Statement

Linux desktop users on Cinnamon (X11) lose clipboard data constantly: copying a new item overwrites the previous one, closing the source application often wipes the clipboard entirely, and pasting multiple fields into forms or terminals requires tedious back-and-forth window switching. Existing cross-platform or web-based clipboard managers either rely on wasteful polling, consume excessive memory (especially on systems without hardware GPU acceleration), clash with native desktop themes, risk persisting passwords and secrets to disk, or leak data over the network. Users need a fast, completely offline, native Linux clipboard manager that reliably captures rich history without draining CPU, organizes clips cleanly, supports sequential pasting, and handles sensitive data honestly and safely.

---

## Solution

PasteFlow is a single-process, offline-only, native Rust and GTK4 clipboard manager designed for Linux Mint Cinnamon (X11-first) that stays resident in the background with a system tray icon and opens an instant keyboard-driven floating overlay on a global shortcut (`Super+V`). It captures text, formatted HTML, images, and file lists via event-driven X11 notifications (zero polling), indexes history locally in SQLite with full-text substring search, automatically classifies clip types (code, URLs, colors, contacts, numbers, images, files, and secrets), organizes saved items into color-coded Spaces, provides a FIFO Universal Paste Queue (`Super+Shift+V`) with per-application paste chords, and keeps sensitive or short-lived clips strictly in zeroized memory so they never touch disk.

---

## User Stories

### Capture & Clipboard History

1. As a desktop user, I want every text item I copy to the X11 CLIPBOARD to be captured automatically in the background, so that I never lose previously copied content.
2. As a desktop user, I want multiline text with exact line breaks, tabs, and whitespace preserved, so that formatted snippets and logs paste back identically.
3. As a desktop user, I want formatted HTML copied alongside plain text to be stored as a secondary format, so that pasting back into rich editors preserves formatting.
4. As a desktop user, I want copied file lists from file managers like Nemo to be stored as file-path references without duplicating file contents, so that I can re-paste file selections safely and without wasting disk space.
5. As a desktop user, I want large clipboard transfers (such as multi-megabyte text or high-resolution screenshots) to be read cleanly using incremental X11 transfers, so that large copies succeed without hanging or truncating.
6. As a desktop user, I want clipboard captures exceeding configurable size caps (text, HTML, image) to be aborted gracefully with a content-free log notice, so that oversized copies cannot exhaust memory or disk space.
7. As a desktop user, I want spreadsheet or office copies that offer both text and an image rendering (such as LibreOffice Calc cells) to be stored primarily as text rather than an image, so that my text data remains searchable and pasteable as text.
8. As a desktop user, I want re-copying an identical item to collapse into the existing entry by bumping its timestamp and usage count (with an option to disable duplicate collapsing), so that my history stays uncluttered.
9. As a desktop user, I want my clipboard history to persist across application restarts and system reboots, so that important clips remain available later.
10. As a desktop user, I want my clipboard history cap (default 2,000, configurable from 100 to 10,000) to prune only unpinned clips in the default Space, so that pinned items and items saved to custom Spaces are never automatically deleted.
11. As a desktop user, I want to pin and unpin any clip, so that frequently used snippets stay at the top of my list and survive history pruning.
12. As a desktop user, I want to delete individual clips or clear history in bulk (all, older than a threshold, or unpinned only), so that I can keep my history clean.
13. As a desktop user, I want to see concise previews, relative timestamps (with exact timestamps on hover), and source application names for each clip, so that I can quickly identify what I copied and when.
14. As a desktop user, I want to copy any historical clip back to the system clipboard with a single click or Enter key, so that I can reuse it immediately.
15. As a desktop user, I want my most recent clipboard item to remain pasteable even after the application I copied it from exits, so that closing a terminal or editor does not clear my clipboard.
16. As a desktop user, I want PasteFlow to hand off clipboard ownership cleanly to the desktop clipboard manager when I quit PasteFlow, so that my current clipboard item survives quitting the app.
17. As a Linux power user, I want an optional, off-by-default setting to capture the X11 PRIMARY selection only after a selection stabilizes with no mouse button held, so that I can record middle-click selections without flooding my history during text dragging.
18. As a Linux power user, I want incremental adjustments to an active PRIMARY highlight to replace the immediately preceding PRIMARY entry instead of creating duplicates, so that adjusting a selection produces one clean clip.

### Content Recognition & Rich Previews

19. As a developer, I want copied code snippets to be automatically recognized as code with a guessed programming language and syntax highlighting in the detail panel, so that I can read and distinguish snippets at a glance.
20. As a developer, I want to manually override the detected programming language on a code clip, so that misclassified snippets display with the right syntax highlighting.
21. As a user, I want copied web addresses to be recognized as URLs showing the parsed host and path without making any network requests, so that links are easy to inspect while remaining completely offline.
22. As a designer or developer, I want copied color values (HEX, RGB, HSL) to be recognized as colors with a visual swatch and one-click copy buttons for converted HEX, RGB, and HSL formats, so that I can convert colors without external tools.
23. As a user, I want copied email addresses, phone numbers, and standalone numbers to be classified into their respective types, so that I can filter and find contact details or numeric values quickly.
24. As a user, I want to assign an optional custom title or label to any clip, so that I can give memorable names to cryptic snippets or IDs.
25. As a user, I want virtual type Collections (Text, Code, Links, Colors, Images, Files, Contacts, Numbers) in the sidebar, so that I can browse clips by content type without manually filing them.

### Images

26. As a user, I want copied screenshots and images from browsers or image editors to be captured in their original format without re-encoding loss, so that re-pasting preserves full image quality.
27. As a user, I want image rows in the clip list to show fast-loading thumbnails along with dimensions, format, and file size, so that browsing many screenshots stays smooth and low-memory.
28. As a user, I want to inspect the full-resolution image in the detail panel, copy it back to the clipboard (including on-demand format conversion if a target app requests another format such as BMP), save it to my Downloads folder with one click and a confirmation toast ("Show in folder"), open it in my default image viewer, or delete it, so that I can manage captured screenshots effortlessly.
29. As a user, I want deleting an image clip to remove both its database record and its underlying stored file on disk, so that deleted screenshots do not consume storage or linger behind.

### Spaces & Organization

30. As a user, I want five starter Spaces (General as default, plus Development, Design, Personal, and Business) out of the box, so that I can organize clips immediately without setup friction.
31. As a user, I want to create, rename, recolor (from a 12-color palette or custom color), change the icon of, and delete custom Spaces, so that my workspace matches my projects.
32. As a user, I want to prevent accidental deletion of the default Space while still allowing it to be renamed, so that new clips always have a valid destination.
33. As a user, I want deleting a non-default Space to prompt me to either move its clips to the default Space or delete them, so that I never lose saved clips unexpectedly.
34. As a user, I want to move any clip into a Space via drag-and-drop onto the sidebar, a context menu, or a keyboard shortcut (`Ctrl+M`), so that I can organize clips with either mouse or keyboard.
35. As a user, I want each clip to belong to one Space and moving a clip out of the default Space to exempt it from automatic history-cap pruning, so that organized clips behave as saved items.
36. As a user, I want instant switching between Spaces with live clip counts shown beside each Space, so that I can navigate organized collections without lag.

### Paste Queue & Universal Paste

37. As a user filling out multi-field forms, I want to add clips to an ordered FIFO Paste Queue, view the queue with a remaining-items count, reorder items via drag-and-drop or keyboard (`Alt+Up`/`Alt+Down`), skip items, remove individual items, or clear the queue, so that I can prepare a sequence of pastes in advance.
38. As a user, I want a "Universal Paste" toggle (accessible from the tray, main window, or `Ctrl+Shift+U`) that automatically appends every newly copied item to the Paste Queue while active, so that I can copy several fields in a row and paste them sequentially.
39. As a user, I want a global "paste next" shortcut (`Super+Shift+V` by default) that loads the head of the queue into the clipboard, waits for held modifier keys to be released, and synthesizes the right paste chord for the focused application, so that I can paste queued items rapidly without stuck modifiers.
40. As a terminal user, I want PasteFlow to send `Ctrl+Shift+V` to terminal emulators (and allow me to customize the per-application paste-key table), so that sequential pasting works seamlessly in both terminals and standard apps.
41. As a user pasting rapidly, I want repeated "paste next" keypresses to be serialized strictly in order and confirmed by observing the target app's actual clipboard read before advancing the queue, so that fast pasting never drops or scrambles items.
42. As a user pasting into an application that blocks synthetic input (such as a VM, remote desktop, or Wayland window), I want PasteFlow to detect when a synthetic paste did not occur, keep the item at the head of the queue, notify me to press `Ctrl+V` manually, and support a manual-paste queue mode, so that the queue remains usable everywhere.
43. As a user, I want options to automatically turn off Universal Paste when the queue finishes and to restore the clipboard to its pre-queue item after the queue empties, so that my normal clipboard state resumes cleanly.

### Privacy, Expiry & Security

44. As a security-conscious user, I want clips copied from password managers that signal a password-manager secret hint to be skipped completely without ever reading their data, so that passwords never enter PasteFlow.
45. As a security-conscious user, I want a configurable list of excluded applications (pre-populated with Bitwarden, KeePassXC, and 1Password, plus a picker for currently running windows), so that copies from sensitive apps are ignored on a best-effort basis.
46. As a security-conscious user, I want PasteFlow to automatically detect secret-looking content (such as API tokens, private key headers, JWTs, OTP codes, and high-entropy single-word tokens), mask its preview, store it strictly in memory (never on disk), and assign a short default expiry (30 seconds), so that accidentally copied secrets disappear automatically.
47. As a security-conscious user, I want to configure or disable secret-detection patterns and mark any clip as sensitive manually, so that I can balance convenience and protection.
48. As a user, I want to set per-clip expiration timers (presets of 15 s, 30 s, 1 m, 5 m, 15 m, 1 h, 24 h, or custom) and see a live countdown pill on expiring clips, so that temporary items clean themselves up.
49. As a user, I want clips flagged as sensitive or given an expiry of 1 hour or less to live exclusively in zeroized RAM and never be written to SQLite, WAL, blobs, or backups, so that short-lived secrets never touch persistent storage.
50. As a user, I want expiring clips to be wiped automatically when their timer reaches zero and for the system clipboard to be cleared if it still holds the expired value, so that expired secrets cannot be pasted afterward.
51. As a user, I want pinned clips to be protected from accidental history pruning and clearly governed by explicit user actions, so that pinned items behave predictably.
52. As a user, I want to pause clipboard capture from the tray or settings (for 5 minutes, 1 hour, or until resumed), so that I can handle sensitive work temporarily without recording anything.
53. As a privacy-conscious user, I want PasteFlow to contain zero network code, zero telemetry, content-free logs, and owner-only file permissions (`0700`/`0600`), so that my clipboard history never leaves my machine or leaks into log files.
54. As a user, I want honest UI wording about deletion guarantees ("removed from PasteFlow's storage", with SQLite secure-delete and WAL truncation on persistent deletes) rather than misleading forensic-erasure claims, so that I understand the true security boundary.

### Search & Filtering

55. As a user with thousands of clips, I want instant substring search across clip text, titles, code, URLs, and image metadata (including 1–2 character prefix/substring queries and 3+ character trigram FTS queries), so that I can locate any clip in milliseconds.
56. As a user, I want to combine search queries with filters for content type, Space, pinned status, and date range (Today, 7 days, 30 days, or custom), so that I can narrow large histories quickly.
57. As a user, I want search results ranked with pinned clips first and most recently used items next, so that my most relevant clips appear immediately.

### Shortcuts, Overlay & UI

58. As a user, I want a global shortcut (`Super+V` by default) to open a compact floating overlay at my mouse pointer (falling back to screen center) with the search input immediately focused, so that I can find and paste a clip without taking my hands off the keyboard.
59. As a user, I want full keyboard navigation in the overlay (`Up`/`Down` to move, `Enter` to copy and close, `Shift+Enter` to copy and paste into the previous window, `Ctrl+1`…`9` for quick pick, `Ctrl+P` to pin, `Ctrl+Q` to queue, `Del` to delete, `Tab` to cycle type filters, `Esc` or focus loss to close), so that retrieving clips takes only a couple of keystrokes.
60. As a user, I want the overlay to work properly with Cinnamon's focus-stealing prevention and return focus to my previous window on close, so that pasting lands where I was working.
61. As a user, I want to customize global shortcuts with a shortcut recorder that detects binding conflicts, choose a one-click Pastily-compatible shortcut preset (`Ctrl+Shift+B` / `Ctrl+Shift+V`), or switch to Cinnamon custom keybindings, so that shortcuts fit my muscle memory and keyboard layout (including non-QWERTY layouts like AZERTY).
62. As a user, I want a full main window with a sidebar (All Clips, Pinned, Paste Queue with badge, Collections, Spaces, Settings), a virtualized/recycled center clip list with filter controls, and a collapsible right-hand detail panel (`F9`), so that I can manage large histories comfortably.
63. As a Mint Cinnamon user, I want PasteFlow to follow my native Mint-Y theme (supporting System, Light, and Dark modes) and offer Compact and Comfortable density options without heavy CPU-bound blur or shadow effects, so that it feels native and stays snappy even under software rendering.

### Desktop Integration, Lifecycle & Maintenance

64. As a user, I want PasteFlow to run as a single instance that stays resident in the background with no windows open, controlled via a Cinnamon system tray icon (StatusNotifierItem) and menu, so that closing the window keeps clipboard monitoring active.
65. As a user, I want an option to launch PasteFlow automatically at login (minimized to tray) and to control it via CLI flags (`--background`, `--toggle`, `--paste-next`, `--quit`), so that it integrates cleanly into my desktop session and custom scripts.
66. As a user, I want automatic database integrity checks at startup, rotating snapshots (cleared on "Clear history"), orphan blob cleanup, and automatic corruption recovery (renaming a corrupt DB, restoring the latest backup, and notifying me), so that a crash or power loss never leaves PasteFlow broken.
67. As a user, I want a Settings → Advanced & Diagnostics view showing storage location, database statistics, manual Vacuum/Backup actions, Export/Import, renderer override, and content-free diagnostics (including detection of conflicting clipboard managers like CopyQ), so that I can inspect and maintain my installation easily.
68. As a Linux Mint user, I want clean `.deb` and `.AppImage` packages with proper desktop entry, icons, and metainfo where uninstalling the package preserves my user data, so that installing and upgrading PasteFlow is straightforward.

---

## Implementation Decisions

### Workspace & Module Architecture
- **Language & Toolkit:** Rust (stable toolchain ≥ 1.92) with **plain GTK4** (`gtk4` v4.14 feature, `gdk4-x11`, `glib`/`gio`), explicitly **without `libadwaita`** so the app inherits the native Mint-Y GTK4 theme and accent colors. Syntax highlighting uses native `sourceview5` (`libgtksourceview-5`).
- **Workspace Crates:** The project is structured into five workspace crates with clear separation of concerns:
  1. **Core domain crate (`pf-core`):** Pure Rust with no GTK or X11 dependencies. Owns domain models, configuration schema (`config.toml` via `serde`/`toml`), modular detector registry (URL, color with in-house HEX/RGB/HSL conversion, code language heuristics, email/phone, number, secret), backend traits (`ClipboardBackend`, `InputInjector`), and log redaction wrapper (`Redacted<T>`).
  2. **Storage crate (`pf-store`):** Owns SQLite (`rusqlite` with `bundled` SQLite for guaranteed FTS5 trigram and FTS5 `secure-delete` support), versioned SQL migrations (`PRAGMA user_version`), content-addressed blob storage, thumbnail metadata, FTS5 search queries, retention pruning, rotating `VACUUM INTO` backups, orphan sweep, and startup corruption recovery.
  3. **X11 engine crate (`pf-x11`):** Pure-Rust X11 protocol engine (`x11rb` with `xfixes`, `xtest`, `xkb` features). Owns the single shared X11 connection, XFixes selection monitor, ICCCM/INCR transfer reader, selection server/owner, `SAVE_TARGETS` hand-off, empty-clipboard guard, best-effort owner identification, root-window global key grabs (`XGrabKey`), and `XTEST` synthetic input. Also includes a standalone diagnostic probe binary for M0 feasibility testing.
  4. **GTK4 application crate (`pf-app`, binary `pasteflow`, app ID `app.pasteflow.PasteFlow`):** Owns `GtkApplication` single-instance lifecycle (`hold()`, CLI flags, D-Bus actions), `GNotification`, `ksni` D-Bus StatusNotifierItem tray (avoiding GTK3/GTK4 conflict), application services (detector pipeline, expiry scheduler, memory-only ephemeral store with `zeroize`, Paste Queue controller), and GTK4 UI screens (Main Window, Floating Overlay, Spaces, Paste Queue, Settings).
  5. **Testkit crate (`pf-testkit`):** Test-only X11 client capable of offering arbitrary `TARGETS`, multi-megabyte `INCR` transfers, slow/vanishing owners, password-manager hints, and exotic MIME types for automated integration tests.

### Threading & Event Loop
- **Single-process, zero-idle-polling design:** The X11 engine runs on the GTK main thread as a non-blocking state machine woken by registering the `x11rb` socket fd on the GLib main loop (`unix_fd_add_local`). No dedicated X11 polling thread and zero periodic timers when idle (a single timer is armed only for the next pending expiry; countdown labels tick only while visible).
- **Coherence rule:** Only `pf-x11` interacts with X11 selections; GTK never touches the clipboard. Self-copies are detected trivially by comparing the selection owner against the engine's own hidden `1×1` unmapped `InputOnly` window.
- **Background workers:** SQLite writes are serialized on a dedicated store-worker thread while UI reads use a concurrent read-only WAL connection. CPU-heavy tasks (BLAKE3 hashing of large payloads, image decoding with limits, thumbnail generation, on-demand image format conversion) run on `gio::spawn_blocking`. The `ksni` tray runs on its own D-Bus thread communicating with the GTK main loop via `async-channel`.
- **Renderer selection for software GPU (`llvmpipe`):** Because the target system runs without the NVIDIA driver, M0 benchmarks `GSK_RENDERER=cairo` vs `ngl`; the winning renderer (expected `cairo`) is set programmatically before `gtk::init` unless overridden by the user in Settings or environment. UI styling avoids blur, heavy shadows, and long animations (CSS transitions ≤ 150 ms).

### X11 Clipboard Engine & Policy
- **Monitoring & Reading:** Subscribes to XFixes (`SetSelectionOwner | SelectionWindowDestroy | SelectionClientClose`). Uses a per-transfer state machine (`RequestTargets → ChooseTargets → RequestData → INCR chunks → Done | Timeout | TooLarge`) with dedicated property atoms, 1 s per-step / 5 s overall timeouts, and configurable caps (default: text 8 MiB, image 64 MiB, HTML 2 MiB).
- **Target Selection Policy:**
  1. If `x-kde-passwordManagerHint` equals `secret`, abort before reading payload data.
  2. If capture is paused or the best-effort owner (`_NET_WM_PID` → `/proc/<pid>/comm`/`exe`, `WM_CLIENT_LEADER` → `WM_CLASS`, fallback `_NET_ACTIVE_WINDOW`) matches the exclusion list, skip.
  3. File lists (`x-special/gnome-copied-files`, `text/uri-list`) become `files` clips (paths only).
  4. Images become the primary representation only when no non-empty text target exists or the text is merely a URL/filename (preventing LibreOffice Calc cells from turning into images).
  5. Text priority: `UTF8_STRING` > `text/plain;charset=utf-8` > `text/plain` > `STRING` (Latin-1 converted); `text/html` (≤ 2 MiB) stored as a secondary format.
  6. App-private/meta targets are ignored.
- **Serving & Hand-off:** Serves `TARGETS`, `TIMESTAMP`, stored MIME formats, text aliases, on-demand converted image formats (cached per ownership), and an internal origin marker (`application/x-pasteflow-origin`). Uses real X server timestamps (never `CurrentTime`). Cooperates with `csd-clipboard` rather than claiming `CLIPBOARD_MANAGER`: on quit, sends `SAVE_TARGETS` to `CLIPBOARD_MANAGER` if owning `CLIPBOARD`; on owner disappearance, waits 300 ms (giving `csd-clipboard` priority) before the empty-clipboard guard reclaims ownership with the last captured clip.
- **PRIMARY Selection:** Off by default. When enabled, captures only after 750 ms of stability with no mouse buttons pressed (`QueryPointer`), minimum 3 characters, coalescing grow/shrink edits into the previous PRIMARY entry.

### Data Model, Storage & Privacy
- **SQLite Configuration:** WAL mode, `synchronous=NORMAL`, `foreign_keys=ON`, `secure_delete=ON`, `temp_store=MEMORY`, `busy_timeout=2000`.
- **Relational Schema:**
  - `spaces`: unique case-insensitive name, hex color, symbolic icon name, sort position, single default Space enforced by partial unique index, creation timestamp; seeded with General (default), Development, Design, Personal, Business.
  - `clips`: kind (`text`, `code`, `url`, `color`, `email`, `phone`, `number`, `image`, `files`), 32-byte BLAKE3 `content_hash` (unique index when duplicate collapsing is enabled), optional title, preview (≤ 300 chars), capped `search_text` (≤ 64 KiB), JSON metadata, byte size, single `space_id` (`ON DELETE RESTRICT`), `pinned`, `selection` (`CLIPBOARD`/`PRIMARY`), best-effort `source_app`, `created_at`, `last_used_at`, `copy_count`, `expires_at` (only populated for persisted > 1 h timers).
  - `clip_formats` (`WITHOUT ROWID`): composite key `(clip_id, mime)` storing either inline `BLOB` (≤ 256 KiB) or external content-addressed `blob_key` (mutually exclusive via `CHECK`), plus byte size.
  - `images`: `clip_id`, width, height, `original_mime`, regenerable `thumb_key`.
  - `queue_items`: `id`, `clip_id`, `ord` (renumbered transactionally on reorder), `added_at`.
  - `clips_fts`: external-content FTS5 virtual table over `search_text` and `title` using `trigram` tokenizer and FTS5 `secure-delete` enabled, kept in sync by insert/delete/update triggers.
- **Content-Addressed Blobs & Thumbnails:** Originals > 256 KiB (images in original encoding and large text) are written atomically (`tmp` → `fsync` → `rename`) into sharded subdirectories under the XDG data directory before DB commit. Thumbnails (≤ 256 px PNG) are generated off-thread into the XDG cache directory and loaded in the UI via a 64-entry `gdk::Texture` LRU. Orphan files/rows are swept at startup and daily.
- **Retention & Search:** History cap prunes only oldest unpinned clips in the default Space; unlinking blobs happens after transaction commit. Queries ≥ 3 chars use FTS5 `MATCH`; 1–2 char queries use `LIKE` on `preview` over the latest 2,000 rows; filters map to SQL `WHERE` clauses; ranking is `pinned DESC, last_used_at DESC`.
- **Memory-Only Ephemeral Store:** Clips flagged as sensitive or given an expiry ≤ 1 h are never written to SQLite, blobs, or backups; they live solely in `Zeroizing<Vec<u8>>` in RAM and are wiped on expiry or app exit. On expiry, if the system clipboard still holds the matching hash, PasteFlow briefly claims and releases ownership to clear it. Persistent privacy deletes and "Clear history" execute `PRAGMA wal_checkpoint(TRUNCATE)` and (for Clear history) remove rotating backups.
- **Offline & Redaction Enforcement:** Zero network dependencies (verified in CI by checking `cargo tree` for HTTP client crates). All clipboard content is wrapped in `Redacted<T>` for logging; logs record only IDs, MIME types, sizes, 8-char hash prefixes, and owner classes.

### Global Shortcuts, Overlay & Paste Queue
- **Shortcut Defaults & Providers:** Defaults are `Super+V` (overlay) and `Super+Shift+V` (paste next), with a one-click Pastily-compatible preset (`Ctrl+Shift+B` / `Ctrl+Shift+V`) in Settings. Two interchangeable providers: (1) in-process `XGrabKey` on root window across Lock/NumLock modifier masks with layout-aware keycodes (`XKB`/`GetKeyboardMapping`) and `BadAccess` conflict reporting (default on X11); (2) Cinnamon custom keybindings (`org.cinnamon.desktop.keybindings`) invoking `pasteflow --toggle` / `--paste-next` via single-instance D-Bus activation (leaving existing user bindings intact, and serving as the Wayland path).
- **Overlay Window:** Single undecorated `GtkWindow` (520×420) created once and shown/hidden (releasing textures on hide). Positioned at pointer clamped to monitor work area via `gdk4-x11` XID (`WM_NORMAL_HINTS` `USPosition`, `_NET_WM_WINDOW_TYPE_UTILITY`, `ConfigureWindow`) before mapping, falling back to center if Muffin ignores placement. Presented with the grab `KeyPress` timestamp so Muffin allows focus, remembering `_NET_ACTIVE_WINDOW` for focus restoration and `Shift+Enter` paste.
- **Paste Queue Execution:** Serializes rapid hotkey presses strictly one-at-a-time. On paste-next: claims `CLIPBOARD` with queue head → waits up to 600 ms for held modifiers to release (sending corrective release events if timed out) → looks up active window's `WM_CLASS` in the editable per-app paste-key table (default `Ctrl+V`; terminals `Ctrl+Shift+V`) → injects layout-aware `XTEST` key events → waits up to 2 s for a non-self, non-`CLIPBOARD_MANAGER` `SelectionRequest` to confirm paste and advance the queue. On timeout, retains head and notifies user to press `Ctrl+V` manually. Manual-paste mode skips `XTEST` and notifies with next item and remaining count.

### UI & Packaging
- **Main Window:** `GtkApplicationWindow` (1000×640) using `GtkListView` + `GtkSignalListItemFactory` backed by a `gio::ListStore` paged 200 rows at a time, ensuring only visible rows are materialized.
- **Packaging:** `.deb` via `cargo-deb` (with `postrm` explicitly preserving user data) and `.AppImage` via `linuxdeploy` + GTK plugin, shipping desktop entry, autostart template, metainfo, and hicolor icons.
- **Repository:** Separate repository (`https://github.com/unsshub/Pasteflow.git`) located in `/home/unss/PasteFlow`.

---

## Testing Decisions

### What Makes a Good Test
- Tests must verify **external, observable behavior** (captured rows, served X11 selection bytes, search results, queue order, expired/redacted state, filesystem and DB side effects) rather than internal implementation details or private helper methods.
- Avoid mocking SQLite or X11 internals when real SQLite (temp dir) and real X11 (`Xvfb`) can be driven directly.
- Assert privacy invariants externally: verify no network client crates exist in the dependency tree, grep integration-test logs for test payload strings to verify redaction, and inspect DB/WAL/blob directories after expiry and deletion.

### Testing Seams & Modules Tested
- **Primary Seam — Headless App/Service Boundary under Xvfb (`pf-x11` + `pf-core` + `pf-store` + `pf-app` services):**
  - Driven externally by `pf-testkit` (an independent X11 client offering arbitrary `TARGETS`, 10 MB+ `INCR` transfers, slow/vanishing owners, password-manager hints, and exotic MIME types) and CLI / `GApplication` D-Bus actions (`--toggle`, `--paste-next`, `--quit`) against temporary XDG directories and an isolated `Xvfb` display.
  - Covers end-to-end capture, target selection policy, deduplication, classification, FTS5 search/filtering, Spaces, Paste Queue (including `XTEST` + `SelectionRequest` confirmation and manual fallback), memory-only sensitive clips, expiry + clipboard clearing, retention pruning, crash/orphan cleanup, corruption recovery, and `SAVE_TARGETS` hand-off.
- **Supplemental Seam 1 — Pure Domain & Storage Public APIs (`pf-core`, `pf-store`):**
  - High-volume table-driven unit tests for detectors (≥ 200 samples including false-positive traps), property-based round-trip tests for color conversions, migration tests from every prior `user_version`, retention edge cases, log redaction, and Criterion-style FTS5 benchmarks on synthetic 2k / 10k / 50k datasets.
- **Supplemental Seam 2 — Desktop / UI & Hardware Verification (`pf-app` on Mint 22.1 Cinnamon):**
  - GTK list-model/filter integration tests under Xvfb + test WM, plus the 30-case manual Mint 22.1 Cinnamon test matrix across real applications (Firefox, Chrome, LibreOffice Writer/Calc, Xed, VS Code, gnome-terminal, Nemo, GIMP, Mint screenshot tool, Bitwarden) and M0 feasibility spikes (`llvmpipe` renderer benchmarks, Muffin overlay placement/focus, `ksni` tray, AZERTY layout, and Cinnamon Wayland probe).

### Prior Art
- This is a greenfield repository with no existing test suite yet. The testing architecture establishes the initial harnesses (`pf-testkit`, Xvfb integration suite gated behind an `xvfb-tests` feature, synthetic dataset generators, and per-milestone performance tracking).

---

## Out of Scope

- **Network features & cloud sync:** No syncing, accounts, telemetry, auto-updaters, licensing checks, or URL title/favicon fetching.
- **Cosmetic Pastily macOS features:** No "Notch Island" UI and no decorative stickers/pins/magnets/tape in v1.0.
- **Forensic-grade disk erasure:** Guaranteed physical erasure on ext4 journaling filesystems or SSD wear-levelling is impossible and explicitly out of scope (mitigated instead by keeping sensitive/≤ 1 h clips memory-only).
- **Full Wayland parity in v1.0:** Cinnamon's Wayland session lacks GlobalShortcuts and RemoteDesktop portals; Wayland is a separate, reduced-capability milestone (M10) using Cinnamon keybindings, centered overlay, and manual queue mode rather than X11 parity.
- **Database encryption at rest (SQLCipher):** Deferred beyond v1.0 (full-disk/home encryption is recommended in the interim).
- **Many-to-many clip-to-Space membership:** Each clip belongs to exactly one Space in v1.0 (schema can be extended later without breaking changes).
- **`libadwaita` styling:** Intentionally excluded to preserve native Mint-Y theming.

---

## Further Notes

- **Verified target environment constraints (Linux Mint 22.1 Cinnamon X11):**
  - **CopyQ** is currently installed and autostarted; it must be quit during testing and disabled once PasteFlow replaces it to avoid selection/shortcut conflicts.
  - **`csd-clipboard`** owns `CLIPBOARD_MANAGER`; PasteFlow cooperates with it rather than replacing it.
  - **No NVIDIA/nouveau kernel module loaded (`llvmpipe` software rendering):** GTK renderer choice (`cairo` vs `ngl`) must be measured in M0 before UI build-out.
  - **Bitwarden** autostarts on the machine and is included in default exclusions from M1; whether Bitwarden Desktop sets `x-kde-passwordManagerHint` is verified in M0.
  - **Rust** is not yet installed and will be installed user-level via `rustup` in M0.
- **Milestone execution order:** M0 (9 feasibility spikes producing a go/no-go report and real-app `TARGETS` table) precedes M1 (headless core engine with images, exclusions, and persistence), followed by M2 (GTK4 shell, tray, lifecycle), M3 (global shortcuts & overlay), M4 (recognition & rich previews), M5 (Paste Queue), M6 (Spaces), M7 (privacy, expiry, backups, PRIMARY), M8 (Settings & polish), M9 (`.deb` / `.AppImage` packaging), and M10 (Wayland compatibility).
- **Target performance budgets (to be measured and recorded per milestone):** ≈ 0% idle CPU with 0 timer wakeups/s; ≤ 45 MB background RSS (no windows opened); ≤ 130 MB RSS with main window / 2k clips / 100 thumbnails (dropping to ≤ 60 MB after window destroy); ≤ 300 ms cold start to tray-ready; ≤ 100 ms warm hotkey-to-overlay paint; ≤ 20 ms text capture commit (≤ 64 KB); ≤ 400 ms off-thread 4K PNG thumbnailing; ≤ 15 ms p95 FTS5 query at 10k clips (≤ 50 ms keystroke-to-render); ≤ 150 MB DB size at 10k text clips.
