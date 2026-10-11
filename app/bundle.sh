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

# The shared outputs are this checkout's debug build: the sidecars, the
# executable and the staged frontend.
deck_bundle_precheck() { # REPO TAURI
  python3 "$1/scripts/sidecar_provenance.py" check --profile debug --binaries --tauri "$2" \
    --main-product "$2/target/debug/deck-app" \
    && python3 "$1/scripts/sidecar_provenance.py" staged --profile debug --tauri "$2"
}

# The executable (as `deck`), the bundled tmux and the four built sidecars
# into APP/Contents/MacOS.
deck_bundle_copy() { # TAURI APP
  mkdir -p "$2/Contents/MacOS" || return 1
  cp "$1/target/debug/deck-app" "$2/Contents/MacOS/deck" || return 1
  cp "$1/binaries/tmux-$DECK_BUNDLE_TRIPLE" "$2/Contents/MacOS/tmux" || return 1
  for deck_bundle_name in $DECK_BUNDLE_SIDECARS; do
    cp "$1/binaries/$deck_bundle_name-$DECK_BUNDLE_TRIPLE" "$2/Contents/MacOS/$deck_bundle_name" || return 1
  done
}

# What APP will actually run is that same debug build: every sidecar, tmux
# and the executable its Info.plist names. Signing does not change the answer.
deck_bundle_postcheck() { # REPO TAURI APP
  python3 "$1/scripts/sidecar_provenance.py" check --profile debug --bundle "$3" --tauri "$2" \
    --main-product "$2/target/debug/deck-app"
}
