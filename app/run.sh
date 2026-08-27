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
# the in-app updater may have replaced this bundle with a release build whose
# executable is named deck-app — kill both names and rebuild the bundle fresh
pkill -x deck 2>/dev/null || true
pkill -f "deck.app/Contents/MacOS" 2>/dev/null || true
sleep 0.3
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/debug/deck-app "$APP/Contents/MacOS/deck"
cp icons/icon.png "$APP/Contents/Resources/icon.png"
cp icons/icon.icns "$APP/Contents/Resources/deck.icns"
VER=$(python3 -c "import json;print(json.load(open('tauri.conf.json'))['version'])")
cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>deck</string>
  <key>CFBundleDisplayName</key><string>deck</string>
  <key>CFBundleExecutable</key><string>deck</string>
  <key>CFBundleIdentifier</key><string>io.c9r.deck</string>
  <key>CFBundleVersion</key><string>${VER}</string>
  <key>CFBundleShortVersionString</key><string>${VER}</string>
  <key>CFBundleIconFile</key><string>deck</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
</dict>
</plist>
EOF

open "$APP"
echo "deck launched. logs: ~/.deck/app.log"
