# Sourced by app/run.sh. The steps every debug bundle goes through, as
# functions, so that scripts/test_sidecar_provenance.py runs the SAME copy and
# the SAME checks on a stand-in tree and puts another build's sidecar in
# between them.
#
# binaries/ and ui-dist/ are ONE location each for every build context of a
# checkout (app/src-tauri/build/shared_output.rs). A check of binaries/ answers
# for one instant; what a bundle runs is what was COPIED afterwards, and an
# editor's `cargo check`, a release build or a coverage run may write there in
# between. So: check the shared outputs, copy, and check the finished bundle
# again before anything opens it.
#
# REPO is the repository root, TAURI the app/src-tauri directory, APP the
# bundle. The triple is the one this repository builds for.

DECK_BUNDLE_TRIPLE=aarch64-apple-darwin
DECK_BUNDLE_SIDECARS="deck-status-helper deck-lifecycle-helper deck-mcp deck-mcp-runner"
# The Pi observer's files (app/src-tauri/resources/pi, the same closed list as
# tauri.conf.json `bundle.resources` and agent_pi/config.rs RESOURCES): text
# the Pi process loads from the bundle, never an executable of deck's own.
DECK_BUNDLE_PI_RESOURCES="observer.ts signal.ts lifecycle.ts both.ts"

# The shared outputs are this checkout's debug build: the sidecars, the
# executable and the staged frontend.
deck_bundle_precheck() { # REPO TAURI
  python3 "$1/scripts/sidecar_provenance.py" check --profile debug --binaries --tauri "$2" \
    --main-product "$2/target/debug/deck-app" \
    && python3 "$1/scripts/sidecar_provenance.py" staged --profile debug --tauri "$2"
}

# The executable (as `deck`), the bundled tmux and the four built sidecars
# into APP/Contents/MacOS, and the Pi observer's files into
# APP/Contents/Resources/pi.
deck_bundle_copy() { # TAURI APP
  mkdir -p "$2/Contents/MacOS" || return 1
  cp "$1/target/debug/deck-app" "$2/Contents/MacOS/deck" || return 1
  cp "$1/binaries/tmux-$DECK_BUNDLE_TRIPLE" "$2/Contents/MacOS/tmux" || return 1
  for deck_bundle_name in $DECK_BUNDLE_SIDECARS; do
    cp "$1/binaries/$deck_bundle_name-$DECK_BUNDLE_TRIPLE" "$2/Contents/MacOS/$deck_bundle_name" || return 1
  done
  deck_bundle_pi_copy "$1" "$2"
}

# The Pi observer's files, exactly the closed list, into APP.
deck_bundle_pi_copy() { # TAURI APP
  rm -rf "$2/Contents/Resources/pi" || return 1
  mkdir -p "$2/Contents/Resources/pi" || return 1
  for deck_bundle_name in $DECK_BUNDLE_PI_RESOURCES; do
    cp "$1/resources/pi/$deck_bundle_name" "$2/Contents/Resources/pi/$deck_bundle_name" || return 1
  done
}

# APP holds the checkout's Pi observer files, byte for byte, and nothing else
# in that directory.
deck_bundle_pi_check() { # TAURI APP
  for deck_bundle_name in $DECK_BUNDLE_PI_RESOURCES; do
    if ! cmp -s "$1/resources/pi/$deck_bundle_name" "$2/Contents/Resources/pi/$deck_bundle_name"; then
      echo "$2 does not hold this checkout's resources/pi/$deck_bundle_name" >&2
      return 1
    fi
  done
  deck_bundle_listed=$(cd "$2/Contents/Resources/pi" && ls -A | LC_ALL=C sort | tr '\n' ' ')
  deck_bundle_wanted=$(printf '%s\n' $DECK_BUNDLE_PI_RESOURCES | LC_ALL=C sort | tr '\n' ' ')
  if [ "$deck_bundle_listed" != "$deck_bundle_wanted" ]; then
    echo "$2/Contents/Resources/pi holds something other than the Pi observer's files" >&2
    return 1
  fi
}

# What APP will actually run is that same debug build: every sidecar, tmux
# and the executable its Info.plist names. Signing does not change the answer.
deck_bundle_postcheck() { # REPO TAURI APP
  python3 "$1/scripts/sidecar_provenance.py" check --profile debug --bundle "$3" --tauri "$2" \
    --main-product "$2/target/debug/deck-app" \
    && deck_bundle_pi_check "$2" "$3"
}
