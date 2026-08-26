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
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/debug/deck-app "$APP/Contents/MacOS/deck"
cp icons/icon.png "$APP/Contents/Resources/icon.png"
cat > "$APP/Contents/Info.plist" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>deck</string>
  <key>CFBundleDisplayName</key><string>deck</string>
  <key>CFBundleExecutable</key><string>deck</string>
  <key>CFBundleIdentifier</key><string>io.c9r.deck</string>
  <key>CFBundleVersion</key><string>0.2.0</string>
  <key>CFBundleShortVersionString</key><string>0.2.0</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
</dict>
</plist>
EOF

pkill -x deck 2>/dev/null || true
sleep 0.3
open "$APP"
echo "deck launched. logs: ~/.deck/app.log"
