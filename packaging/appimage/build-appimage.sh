#!/usr/bin/env bash
set -euo pipefail

# Build a self-contained PasteFlow AppImage using linuxdeploy + linuxdeploy-plugin-gtk.
# Prerequisites:
#   - cargo (Rust >= 1.92)
#   - linuxdeploy-x86_64.AppImage in $PATH (or LINUXDEPLOY env var)
#   - linuxdeploy-plugin-gtk.sh in $PATH

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
APPDIR="$ROOT_DIR/target/AppDir"
LINUXDEPLOY="${LINUXDEPLOY:-linuxdeploy-x86_64.AppImage}"

cargo build --release -p pf-app --bin pasteflow

rm -rf "$APPDIR"
mkdir -p \
  "$APPDIR/usr/bin" \
  "$APPDIR/usr/share/applications" \
  "$APPDIR/usr/share/metainfo" \
  "$APPDIR/usr/share/icons/hicolor/scalable/apps"

install -m755 "$ROOT_DIR/target/release/pasteflow" "$APPDIR/usr/bin/pasteflow"
install -m644 "$ROOT_DIR/data/app.pasteflow.PasteFlow.desktop" \
  "$APPDIR/usr/share/applications/app.pasteflow.PasteFlow.desktop"
install -m644 "$ROOT_DIR/data/app.pasteflow.PasteFlow.metainfo.xml" \
  "$APPDIR/usr/share/metainfo/app.pasteflow.PasteFlow.metainfo.xml"
install -m644 "$ROOT_DIR/data/icons/hicolor/scalable/apps/app.pasteflow.PasteFlow.svg" \
  "$APPDIR/usr/share/icons/hicolor/scalable/apps/app.pasteflow.PasteFlow.svg"

export DEPLOY_GTK_VERSION=4
export OUTPUT="$ROOT_DIR/target/PasteFlow-x86_64.AppImage"

"$LINUXDEPLOY" \
  --appdir "$APPDIR" \
  --desktop-file "$APPDIR/usr/share/applications/app.pasteflow.PasteFlow.desktop" \
  --icon-file "$APPDIR/usr/share/icons/hicolor/scalable/apps/app.pasteflow.PasteFlow.svg" \
  --plugin gtk \
  --output appimage

echo "Built $OUTPUT"
