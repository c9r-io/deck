#!/bin/sh
# Build deck and launch it as a proper .app via LaunchServices.
#
# Never launch target/debug/deck-app directly from a background shell: a bare
# binary outside the GUI login session can't reach macOS text-input services
# (TSM/IMK) — the window opens and mouse works, but keyboard input is dead.
set -e
cd "$(dirname "$0")/src-tauri"

cargo build

APP=target/debug/deck.app
BUNDLE_NAME=deck
BUNDLE_ID=io.c9r.deck
if [ -n "${DECK_SMOKE_DATA_DIR:-}" ]; then
  # Smoke must coexist with the user's normal deck. Give LaunchServices a
  # separate bundle identity/path and never run the normal-instance pkill.
  APP=target/debug/deck-smoke.app
  BUNDLE_NAME="deck smoke"
  BUNDLE_ID=io.c9r.deck.smoke
else
  # the in-app updater may have replaced this bundle with a release build whose
  # executable is named deck-app — kill both names and rebuild the bundle fresh
  pkill -x deck 2>/dev/null || true
  pkill -f "deck.app/Contents/MacOS" 2>/dev/null || true
  sleep 0.3
fi
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/debug/deck-app "$APP/Contents/MacOS/deck"
cp icons/icon.png "$APP/Contents/Resources/icon.png"
cp icons/icon.icns "$APP/Contents/Resources/deck.icns"
# bundle the static tmux sidecar into the dev bundle too (dmg parity)
if [ -f binaries/tmux-aarch64-apple-darwin ]; then
  cp binaries/tmux-aarch64-apple-darwin "$APP/Contents/MacOS/tmux"
fi
VER=$(python3 -c "import json;print(json.load(open('tauri.conf.json'))['version'])")
cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>${BUNDLE_NAME}</string>
  <key>CFBundleDisplayName</key><string>${BUNDLE_NAME}</string>
  <key>CFBundleExecutable</key><string>deck</string>
  <key>CFBundleIdentifier</key><string>${BUNDLE_ID}</string>
  <key>CFBundleVersion</key><string>${VER}</string>
  <key>CFBundleShortVersionString</key><string>${VER}</string>
  <key>CFBundleIconFile</key><string>deck</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
</dict>
</plist>
EOF

if [ -n "${DECK_SMOKE_DATA_DIR:-}" ]; then
  case "$DECK_SMOKE_DATA_DIR" in
    /*) ;;
    *) echo "DECK_SMOKE_DATA_DIR must be absolute" >&2; exit 2 ;;
  esac
  DECK_SMOKE_TMUX_SOCKET=${DECK_SMOKE_TMUX_SOCKET:-deck-smoke}
  if [ -n "${DECK_SMOKE_WKWEBVIEW:-}" ]; then
    SMOKE_MODE=$DECK_SMOKE_WKWEBVIEW
    case "$SMOKE_MODE" in
      run|restart|ambiguous) ;;
      *) SMOKE_MODE=run ;;
    esac
    open -n "$APP" --args \
      --smoke-data-dir "$DECK_SMOKE_DATA_DIR" \
      --smoke-tmux-socket "$DECK_SMOKE_TMUX_SOCKET" \
      --smoke-wkwebview "$SMOKE_MODE"
  else
    open -n "$APP" --args \
      --smoke-data-dir "$DECK_SMOKE_DATA_DIR" \
      --smoke-tmux-socket "$DECK_SMOKE_TMUX_SOCKET"
  fi
  echo "deck smoke bundle launched with isolated data and tmux socket"
else
  open "$APP"
  echo "deck launched. logs: ~/.deck/app.log"
fi
