#!/bin/sh
# Build deck and launch it as a proper .app via LaunchServices.
#
# Never launch target/debug/deck-app directly from a background shell: a bare
# binary outside the GUI login session can't reach macOS text-input services
# (TSM/IMK) — the window opens and mouse works, but keyboard input is dead.
set -e
APP_DIR=$(CDPATH= cd -- "$(dirname "$0")" && pwd)
REPO_ROOT=$(dirname "$APP_DIR")
cd "$APP_DIR/src-tauri"

# A smoke launch must never fall into the default development path below,
# whose pkill pattern also matches an installed deck.app. Refuse partial
# smoke settings and validate the isolation arguments before building.
if [ -z "${DECK_SMOKE_DATA_DIR:-}" ] && { [ -n "${DECK_SMOKE_WKWEBVIEW:-}" ] || [ -n "${DECK_SMOKE_TMUX_SOCKET:-}" ]; }; then
  echo "DECK_SMOKE_WKWEBVIEW / DECK_SMOKE_TMUX_SOCKET require DECK_SMOKE_DATA_DIR" >&2
  exit 2
fi
if [ -n "${DECK_SMOKE_DATA_DIR:-}" ]; then
  case "$DECK_SMOKE_DATA_DIR" in
    /*) ;;
    *) echo "DECK_SMOKE_DATA_DIR must be absolute" >&2; exit 2 ;;
  esac
  case "${DECK_SMOKE_TMUX_SOCKET:-deck-smoke-$$}" in
    deck-smoke*) ;;
    *) echo "DECK_SMOKE_TMUX_SOCKET must start with deck-smoke" >&2; exit 2 ;;
  esac
fi
if [ "${DECK_SMOKE_WKWEBVIEW:-}" = channel-first-send ]; then
  case "$DECK_SMOKE_DATA_DIR:$DECK_SMOKE_TMUX_SOCKET" in
    /tmp/deck-channel-?*:deck-smoke-channel-?*) ;;
    *) echo "channel-first-send requires /tmp/deck-channel-* and deck-smoke-channel-*" >&2; exit 2 ;;
  esac
  case "$DECK_SMOKE_DATA_DIR" in
    *[!-A-Za-z0-9_./]*) echo "channel-first-send data path contains unsafe characters" >&2; exit 2 ;;
    */../*|*/..) echo "channel-first-send data path must not contain parent traversal" >&2; exit 2 ;;
  esac
  case "$DECK_SMOKE_TMUX_SOCKET" in
    *[!a-z0-9-]*) echo "channel-first-send socket must use lowercase letters, digits and hyphens" >&2; exit 2 ;;
  esac
  if [ ! -d "$DECK_SMOKE_DATA_DIR" ] || [ -L "$DECK_SMOKE_DATA_DIR" ] \
    || [ -n "$(/usr/bin/find "$DECK_SMOKE_DATA_DIR" -mindepth 1 -maxdepth 1 -print -quit)" ]; then
    echo "channel-first-send requires a new empty non-symlink mktemp directory" >&2
    exit 2
  fi
  # Build the harmless literal `claude` before assembling the carrier. It
  # lives in the signed app, outside the data tree whose hardening removes
  # execute bits from ordinary data files.
  cargo build --example channel_fixture
fi

cargo build

APP=target/debug/deck-dev.app
BUNDLE_NAME="deck dev"
BUNDLE_ID=io.c9r.deck.dev
LEGACY_DEBUG_APP=target/debug/deck.app
if [ -n "${DECK_SMOKE_DATA_DIR:-}" ]; then
  # Smoke must coexist with the user's normal deck. Give LaunchServices a
  # separate bundle identity/path and never run the normal-instance pkill.
  APP=target/debug/deck-smoke.app
  BUNDLE_NAME="deck smoke"
  BUNDLE_ID=io.c9r.deck.smoke
  if [ "${DECK_SMOKE_WKWEBVIEW:-}" = channel-first-send ]; then
    CHANNEL_SMOKE_SUFFIX=${DECK_SMOKE_TMUX_SOCKET#deck-smoke-channel-}
    APP="target/debug/deck-channel-smoke-$CHANNEL_SMOKE_SUFFIX.app"
    BUNDLE_NAME="deck channel smoke $CHANNEL_SMOKE_SUFFIX"
    BUNDLE_ID="io.c9r.deck.smoke.channel.x$CHANNEL_SMOKE_SUFFIX"
  fi
else
  # the in-app updater may have replaced this bundle with a release build whose
  # executable is named deck-app — kill both names and rebuild the bundle fresh
  pkill -x deck 2>/dev/null || true
  pkill -f "deck.app/Contents/MacOS" 2>/dev/null || true
  sleep 0.3
  # Older source builds registered a debug app as io.c9r.deck. Remove only
  # that generated target bundle so future launches use the dedicated dev ID.
  rm -rf "$LEGACY_DEBUG_APP"
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
# agent-hook status helper, built by build.rs into binaries/ (dmg parity)
if [ -f binaries/deck-status-helper-aarch64-apple-darwin ]; then
  cp binaries/deck-status-helper-aarch64-apple-darwin "$APP/Contents/MacOS/deck-status-helper"
fi
# MCP adapter and managed terminal runner, built by build.rs into binaries/.
# They remain ordinary signed bundle sidecars and are never installed globally.
if [ -f binaries/deck-mcp-aarch64-apple-darwin ]; then
  cp binaries/deck-mcp-aarch64-apple-darwin "$APP/Contents/MacOS/deck-mcp"
fi
if [ -f binaries/deck-mcp-runner-aarch64-apple-darwin ]; then
  cp binaries/deck-mcp-runner-aarch64-apple-darwin "$APP/Contents/MacOS/deck-mcp-runner"
fi
if [ "${DECK_SMOKE_WKWEBVIEW:-}" = channel-first-send ]; then
  mkdir -p "$APP/Contents/MacOS/channel-fixture-bin"
  cp target/debug/examples/channel_fixture "$APP/Contents/MacOS/channel-fixture-bin/claude"
  chmod 700 "$APP/Contents/MacOS/channel-fixture-bin/claude"
  # Sign the nested executable before sealing the containing carrier.
  codesign --force --sign - "$APP/Contents/MacOS/channel-fixture-bin/claude"
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
  <key>NSMicrophoneUsageDescription</key><string>Record your voice to compose a prompt using on-device recognition.</string>
  <key>NSSpeechRecognitionUsageDescription</key><string>Convert your voice into an editable prompt on this Mac.</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSLocalNetworkUsageDescription</key><string>Terminal tools running in deck may connect to local services and devices you choose.</string>
</dict>
</plist>
EOF

# Stable ad-hoc bundle identity for local TCC permission checks; release uses
# Developer ID signing and the same microphone entitlement via Tauri.
codesign --force --sign - --entitlements Entitlements.plist "$APP"

if [ -n "${DECK_SMOKE_DATA_DIR:-}" ]; then
  DECK_SMOKE_TMUX_SOCKET=${DECK_SMOKE_TMUX_SOCKET:-deck-smoke-$$}
  if [ -n "${DECK_SMOKE_WKWEBVIEW:-}" ]; then
    SMOKE_MODE=$DECK_SMOKE_WKWEBVIEW
    case "$SMOKE_MODE" in
      run|restart|ambiguous|settings|attention|review|review-restart|voice|translation|translation-native|translation-guard|resume|buffer|buffer-narrow|channel|channel-fault|channel-first-send|connector|connector-transport|selection-events|signal-finish|authority-live|empty-start|clock-live|reminder|reminder-native|board-lost|approval|ux-layers) ;;
      *) SMOKE_MODE=run ;;
    esac
    # the signal-finish smoke's fake agent: a debug example, never bundled
    if [ "$SMOKE_MODE" = signal-finish ]; then
      cargo build --example signal_fixture
    fi
    OPEN_BACKGROUND=
    if [ "$SMOKE_MODE" = channel-first-send ]; then
      FIXTURE_BIN=$(CDPATH= cd -- "$APP/Contents/MacOS/channel-fixture-bin" && pwd -P)
      mkdir -p "$DECK_SMOKE_DATA_DIR/home" "$DECK_SMOKE_DATA_DIR/tmp" \
        "$DECK_SMOKE_DATA_DIR/channel-fixture"
      chmod 700 "$DECK_SMOKE_DATA_DIR" "$DECK_SMOKE_DATA_DIR/home" "$DECK_SMOKE_DATA_DIR/tmp" \
        "$DECK_SMOKE_DATA_DIR/channel-fixture"
      # macOS /etc/zprofile runs path_helper for login shells and prepends
      # host paths. The private user profile restores the closed fixture PATH
      # afterward, before Deck asks that shell to run literal `claude`.
      printf '%s\n' "export PATH='$FIXTURE_BIN:/usr/bin:/bin'" > "$DECK_SMOKE_DATA_DIR/home/.zprofile"
      chmod 600 "$DECK_SMOKE_DATA_DIR/home/.zprofile"
      OPEN_BACKGROUND=-g
    fi
    open $OPEN_BACKGROUND -n "$APP" --args \
      --smoke-data-dir "$DECK_SMOKE_DATA_DIR" \
      --smoke-tmux-socket "$DECK_SMOKE_TMUX_SOCKET" \
      --smoke-wkwebview "$SMOKE_MODE" \
      "$@"
    echo "when the run has finished, judge it: $REPO_ROOT/scripts/smoke-verdict $DECK_SMOKE_DATA_DIR $SMOKE_MODE"
  else
    open -n "$APP" --args \
      --smoke-data-dir "$DECK_SMOKE_DATA_DIR" \
      --smoke-tmux-socket "$DECK_SMOKE_TMUX_SOCKET" \
      "$@"
  fi
  echo "deck smoke bundle launched with isolated data and tmux socket"
  echo "cleanup after evidence capture: $REPO_ROOT/scripts/edr_runtime.py --cleanup --socket $DECK_SMOKE_TMUX_SOCKET"
else
  if [ "$#" -gt 0 ]; then
    open "$APP" --args "$@"
  else
    open "$APP"
  fi
  echo "deck launched. logs: ~/.deck/app.log"
fi
