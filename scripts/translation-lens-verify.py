#!/usr/bin/env python3
"""Unattended Local Translation acceptance (one command, no human steps).

    python3 scripts/translation-lens-verify.py

preflight -> isolated run root -> verified model pack -> build + bundle
identity -> L1 (node + cargo) -> baseline negative control (separate) ->
N independent GUI runs of the real isolated WKWebView (`translation` L2 with
a controlled provider, `translation-native` L3 on the production path) ->
smoke-verdict -> evidence -> cleanup (finally) -> machine report + exit code.

Exit codes: 0 PASS, 1 FAIL, 2 BLOCKED. The report is written even when a
step fails. Environment: DECK_TL_RUNS (default 3), DECK_TL_MODEL_CACHE (a
directory holding the four pinned assets; verified by SHA-256 before use).

Isolation: a fresh 0700 run root under /tmp, a bundle with its own path and
identifier, a unique deck-smoke-* tmux socket, and a separate smoke data
root per mode and run. Only processes this script launched are terminated,
by recorded PID and verified executable path. The general pasteboard is a
shared OS resource: the app-hosted guard (native/SmokeBridge.swift) keeps
the original items in Deck's memory only and restores them only while the
change is test-owned; this script only ever writes synthetic text with
pbcopy inside that guarded window and never reads the pasteboard.
"""

import datetime
import gzip
import hashlib
import json
import os
import re
import shutil
import signal
import statistics
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TAURI = ROOT / "app/src-tauri"
MANIFEST = json.loads((ROOT / "scripts/translation-lens-verify.manifest.json").read_text())
SMOKE_MANIFEST = json.loads((ROOT / "app/ui/test/fixtures/smoke-manifest.json").read_text())
RUNS = int(os.environ.get("DECK_TL_RUNS", "3"))
MODES = ("translation-guard", "translation", "translation-native")
# Iteration aid only: a subset of modes can never satisfy the manifest, so
# such a run is reported FAIL (missing evidence), never PASS.
RUN_MODES = tuple(m for m in os.environ.get("DECK_TL_MODES", ",".join(MODES)).split(",") if m in MODES)
AWAY_TEXT = "Deck harmless text copied while Deck was away."
# The driver's own general-pasteboard write (C04): compare-and-write against
# the version the guard permitted; prints the clearContents() receipt or
# "refused". NSPasteboard has no cross-process CAS, so a narrow window remains.
JXA_WRITE = """function run(argv) {
  ObjC.import('AppKit');
  const board = $.NSPasteboard.generalPasteboard;
  if (Number(board.changeCount) !== Number(argv[1])) return 'refused';
  const receipt = board.clearContents;
  board.setStringForType($(argv[0]), $.NSPasteboardTypeString);
  return String(receipt);
}"""
JXA_RELEASE = """function run(argv) { ObjC.import('AppKit'); $.NSPasteboard.pasteboardWithName(argv[0]).releaseGlobally; return 'ok'; }"""
RESULT_NAMES = {10: "not-written", 11: "restored", 12: "external-kept", 13: "restore-failed", 15: "begin-refused",
                0: "active-unsettled"}
RUN = Path(tempfile.mkdtemp(prefix="deck-tl-verify-", dir="/tmp")).resolve()
os.chmod(RUN, 0o700)
RUN_ID = RUN.name.replace("deck-tl-verify-", "").replace("_", "").lower()
EVIDENCE = RUN / "evidence"
EVIDENCE.mkdir(mode=0o700)
REPORT_PATH = RUN / "report.json"
HOME_DECK = Path.home() / ".deck"

report = {
    "schemaVersion": 1, "runId": RUN_ID, "startedAt": datetime.datetime.now().isoformat(timespec="seconds"),
    "command": "python3 scripts/translation-lens-verify.py",
    "sourceCommit": None, "branch": None, "dirtyPatchDigest": None, "testedSourceDigest": None,
    "bundlePath": None, "binaryHash": None, "toolchain": {},
    "environmentReady": False, "isolationVerified": False,
    "requiredTestsExecuted": False, "productAssertionsPassed": False,
    "cleanupCompleted": False, "humanInterventions": 0,
    "requiredCount": len(MANIFEST["required"]), "executedCount": 0, "passedCount": 0,
    "failedCount": 0, "skippedRequired": [],
    "evidenceLayers": {
        "L1": "node:test model/cadence (fake clock) and coordinator (fake DOM + controlled backend); cargo unit tests of the native gate, bounds and protected content",
        "L2": "isolated debug bundle, real WKWebView + xterm + tmux + Lens UI/IPC, native AppKit input into Deck's own window, CONTROLLED provider and clipboard gate",
        "L3": "isolated debug bundle, production translation path: real Bergamot model from the isolated pack, real AppKit pasteboard gate/focus checks, real terminal content from the /copy CLI fixture",
    },
    "mockBoundaries": {
        "fakeProvider": "L2 `translation` mode only (installTranslationSmokeBackend): translate/cancel/arm/poll/pack are controlled for fault and late-response injection",
        "cliFixture": "scripts/translation_lens_fixture.py replaces only the content provider (fixed synthetic English answer); /copy really writes the general pasteboard with pbcopy. Controlled /copy workflow integration, not a vendor Agent certification",
        "namedPasteboard": "L3 boundary/race cases (C03, C06, D05, D06) point the SAME native gate at a test-owned named NSPasteboard; the general pasteboard is exercised by C01, C02, C04, C05 and B05 (Cmd+C)",
        "domDriven": "L2 tl-b06-no-rewrite dispatches deck-terminal-changed from JS; tl-e01-font/themes/locales call the product's theme, font-scale and locale functions; settings clicks use element.click(). All other input is native AppKit events",
        "native": "clicks, drags, wheel, keys (incl. Cmd+C and the Lens shortcut) and NSApp.hide are AppKit events handed to Deck's own window; focus returns via LaunchServices (`open <bundle>`) or NSApp.activate",
    },
    "timingSamples": {}, "ownedResources": [], "sharedClipboardHandling": {},
    "baselineReproduction": None, "failures": [], "evidencePaths": {"runRoot": str(RUN), "report": str(REPORT_PATH)},
    "tests": {}, "runs": [], "verdict": "BLOCKED",
    "functionalAssertionsPassed": False, "harnessSafetyPassed": False, "processCleanupCompleted": False,
    "sharedResourceSafetyPassed": False, "clipboardGuardResults": [], "baselineRegressionDetected": None,
    "incidents": [{
        "when": "2026-09-29 ~09:51 JST, first debug L2 run of the previous round (9a2e8b1 development)",
        "what": "the old guard refused to restore after a text-equality claim failed; the user's original general "
                "pasteboard content from before that run was overwritten by a synthetic Cmd+C and could not be recovered",
        "status": "recorded; not re-created; this round removes the failure path (hard gates before any write)"}],
}
owned = {"processes": [], "sockets": [], "caffeinate": None}


def save():
    REPORT_PATH.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n")
    os.chmod(REPORT_PATH, 0o600)


def fail(message):
    report["failures"].append(message)
    save()


def capture(args, timeout=60, cwd=ROOT, check=True):
    done = subprocess.run(args, cwd=cwd, capture_output=True, text=True, timeout=timeout)
    if check and done.returncode:
        raise RuntimeError(f"{args[0]} exited {done.returncode}")
    return done.stdout.strip()


def run_logged(args, log, timeout, cwd=ROOT, env=None):
    with open(log, "w") as out:
        done = subprocess.run(args, cwd=cwd, stdout=out, stderr=subprocess.STDOUT, timeout=timeout,
                              env=env or os.environ.copy())
    return done.returncode


def sha256_file(path):
    digest = hashlib.sha256()
    with open(path, "rb") as file:
        for chunk in iter(lambda: file.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def source_digest():
    files = capture(["git", "ls-files", "-co", "--exclude-standard", "--", "app", "scripts", "docs",
                     "CLAUDE.md", "rust-toolchain.toml"]).splitlines()
    digest = hashlib.sha256()
    for name in sorted(files):
        path = ROOT / name
        if path.is_file() and "/target/" not in name and "/ui-dist/" not in name:
            digest.update(name.encode() + b"\0" + sha256_file(path).encode() + b"\n")
    return digest.hexdigest()


def dirty_digest():
    diff = subprocess.run(["git", "diff", "--binary", "HEAD"], cwd=ROOT, capture_output=True, timeout=60).stdout
    digest = hashlib.sha256(diff)
    for name in sorted(capture(["git", "ls-files", "--others", "--exclude-standard"]).splitlines()):
        path = ROOT / name
        if path.is_file():
            digest.update(name.encode() + b"\0" + sha256_file(path).encode())
    return digest.hexdigest()


def screen_locked():
    text = capture(["ioreg", "-n", "Root", "-d1"], 20)
    return '"CGSSessionScreenIsLocked"=Yes' in text.replace(" ", "")


def pack_assets():
    source = (TAURI / "src/intelligence/pack.rs").read_text()
    base = re.search(r'const BASE: &str = "([^"]+)";', source).group(1)
    pack_id = re.search(r'const PACK_ID: &str = "([^"]+)";', source).group(1)
    assets = [{"name": m.group(1), "compressed": int(m.group(2).replace("_", "")),
               "bytes": int(m.group(3).replace("_", "")), "sha256": m.group(4)}
              for m in re.finditer(r'name: "([^"]+)",\s*compressed_bytes: ([0-9_]+),\s*bytes: ([0-9_]+),\s*sha256: "([0-9a-f]+)"', source)]
    if len(assets) != 4:
        raise RuntimeError("pack manifest parse failed")
    return base, pack_id, assets


def verified_dir(directory, assets):
    return all((directory / a["name"]).is_file() and (directory / a["name"]).stat().st_size == a["bytes"]
               and sha256_file(directory / a["name"]) == a["sha256"] for a in assets)


def prepare_model():
    base, pack_id, assets = pack_assets()
    candidates = [os.environ.get("DECK_TL_MODEL_CACHE"), "/private/tmp/deck-translation-quality-demo/model"]
    for candidate in filter(None, candidates):
        if verified_dir(Path(candidate), assets):
            report["modelSource"] = {"kind": "verified read-only cache", "path": candidate}
            return Path(candidate), pack_id, assets
    cache = RUN / "model-cache"
    cache.mkdir(mode=0o700)
    for asset in assets:
        with urllib.request.urlopen(base + asset["name"] + ".gz", timeout=120) as response:
            compressed = response.read(asset["compressed"] + 1)
        if len(compressed) != asset["compressed"]:
            raise RuntimeError("model download size mismatch")
        (cache / asset["name"]).write_bytes(gzip.decompress(compressed))
    if not verified_dir(cache, assets):
        raise RuntimeError("downloaded model failed verification")
    report["modelSource"] = {"kind": "downloaded from the pinned manifest into the run root", "path": str(cache)}
    return cache, pack_id, assets


def install_pack(data, cache, pack_id, assets):
    target = data / "models/translation" / pack_id
    for directory in (data / "models", data / "models/translation", target):
        directory.mkdir(mode=0o700, exist_ok=True)
        os.chmod(directory, 0o700)
    for asset in assets:
        shutil.copyfile(cache / asset["name"], target / asset["name"])
        os.chmod(target / asset["name"], 0o600)
    return target


def build_bundle():
    log = EVIDENCE / "build.log"
    if run_logged(["cargo", "build", "--manifest-path", str(TAURI / "Cargo.toml")], log, 3600) != 0:
        raise RuntimeError("cargo build failed (evidence/build.log)")
    dist = TAURI / "ui-dist"
    mismatched = []
    for path in (ROOT / "app/ui").rglob("*"):
        if path.is_file():
            staged = dist / path.relative_to(ROOT / "app/ui")
            if not staged.is_file() or sha256_file(staged) != sha256_file(path):
                mismatched.append(str(path.relative_to(ROOT)))
    if mismatched:
        raise RuntimeError("embedded frontend does not match app/ui")
    bundle = RUN / "deck-smoke-tl.app"
    macos, resources = bundle / "Contents/MacOS", bundle / "Contents/Resources"
    macos.mkdir(parents=True)
    resources.mkdir()
    shutil.copy2(TAURI / "target/debug/deck-app", macos / "deck")
    for name in ("tmux", "deck-status-helper"):
        shutil.copy2(TAURI / f"binaries/{name}-aarch64-apple-darwin", macos / name)
    shutil.copy2(TAURI / "icons/icon.icns", resources / "deck.icns")
    version = json.loads((TAURI / "tauri.conf.json").read_text())["version"]
    import plistlib
    with open(bundle / "Contents/Info.plist", "wb") as file:
        plistlib.dump({"CFBundleExecutable": "deck", "CFBundleIdentifier": f"io.c9r.deck.smoke.tl{RUN_ID}",
                       "CFBundleName": "deck smoke", "CFBundleVersion": version,
                       "CFBundleShortVersionString": version, "CFBundlePackageType": "APPL",
                       "NSHighResolutionCapable": True, "LSMinimumSystemVersion": "11.0"}, file)
    if run_logged(["codesign", "--force", "--sign", "-", "--entitlements", str(TAURI / "Entitlements.plist"),
                   str(bundle)], EVIDENCE / "codesign.log", 120) != 0:
        raise RuntimeError("codesign failed")
    report["bundlePath"] = str(bundle)
    report["bundleIdentifier"] = f"io.c9r.deck.smoke.tl{RUN_ID}"
    report["binaryHash"] = sha256_file(macos / "deck")
    report["frontendEmbeddedMatchesSource"] = True
    save()
    return bundle


def processes():
    out = capture(["ps", "-axo", "pid=,command="], 20)
    rows = []
    for line in out.splitlines():
        parts = line.strip().split(" ", 1)
        if len(parts) == 2 and parts[0].isdigit():
            rows.append((int(parts[0]), parts[1]))
    return rows


def production_pids():
    """The production app process itself. Its tmux attach clients come and go
    as the user works and are not a signal of this test's behaviour."""
    return sorted(pid for pid, command in processes()
                  if command.split(" ", 1)[0] == "/Applications/deck.app/Contents/MacOS/deck-app")


def l1():
    results, log = {}, EVIDENCE / "l1-node.tap"
    code = run_logged(["node", "--test", "--test-reporter=tap", *MANIFEST["l1Files"]], log, 600)
    for line in log.read_text().splitlines():
        match = re.match(r"^(not ok|ok) \d+ - (.*)$", line.strip())
        if match:
            results[match.group(2)] = match.group(1) == "ok"
    report["tests"]["l1NodeExit"] = code
    rust_log = EVIDENCE / "l1-cargo.log"
    rust_code = run_logged(["cargo", "test", "--manifest-path", str(TAURI / "Cargo.toml"), "--bin", "deck-app",
                            "--", *MANIFEST["rustFilters"]], rust_log, 3600)
    rust = {}
    for line in rust_log.read_text().splitlines():
        match = re.match(r"^test (\S+) \.\.\. (ok|FAILED|ignored)$", line.strip())
        if match:
            rust[match.group(1)] = match.group(2) == "ok"
    report["tests"]["l1CargoExit"] = rust_code
    # the whole Rust workspace gate (privacy, EDR, IPC and admission censuses included)
    report["tests"]["cargoWorkspaceExit"] = run_logged(
        ["cargo", "test", "--workspace", "--locked", "--manifest-path", str(TAURI / "Cargo.toml")],
        EVIDENCE / "cargo-workspace.log", 3600)
    report["tests"]["clippyExit"] = run_logged(
        ["cargo", "clippy", "--workspace", "--all-targets", "--all-features", "--locked", "--manifest-path",
         str(TAURI / "Cargo.toml"), "--", "-D", "warnings"], EVIDENCE / "clippy.log", 3600)
    harness_log = EVIDENCE / "native-guard-harness.log"
    report["tests"]["nativeGuardHarnessExit"] = run_logged([str(ROOT / MANIFEST["nativeHarness"])], harness_log, 900)
    report["tests"]["nativeGuardMutantsCaught"] = sorted(re.findall(r"mutant (\S+) caught", harness_log.read_text()))
    gate_log = EVIDENCE / "ui-tests.log"
    report["tests"]["uiGateExit"] = run_logged(["sh", "scripts/ui-tests"], gate_log, 900)
    return results, rust


def baseline():
    """Negative controls, kept apart from product evidence: the new G tests on
    the start version's sources, the F01/F02/F05 tests on the start version's
    guard control flow (compat carrier), and the native mutants (L1 above)."""
    spec = MANIFEST["baseline"]
    tree = RUN / "baseline"
    tree.mkdir(mode=0o700)
    archive = subprocess.run(["git", "archive", spec["commit"], "app/ui"], cwd=ROOT, capture_output=True, timeout=120)
    subprocess.run(["tar", "-x", "-C", str(tree)], input=archive.stdout, check=True, timeout=120)
    for name in spec["files"]:
        shutil.copy2(ROOT / name, tree / name)
    shutil.copy2(ROOT / "app/ui/test/fixtures/dom-fixture.mjs", tree / "app/ui/test/fixtures/dom-fixture.mjs")

    def failed_ids(log):
        found = set()
        for line in log.read_text().splitlines():
            match = re.match(r"^not ok \d+ - (.*)$", line.strip())
            if match:
                found.update(re.findall(r"\[([A-G]\d\d)\]", match.group(1)))
        return found
    log = EVIDENCE / "baseline-g.tap"
    run_logged(["node", "--test", "--test-reporter=tap", *spec["files"]], log, 300, cwd=tree)
    g_failed = failed_ids(log)
    legacy = spec["legacyGuardFlow"]
    legacy_log = EVIDENCE / "baseline-f-legacy-flow.tap"
    run_logged(["node", "--test", "--test-reporter=tap", legacy["file"]], legacy_log, 120,
               env=dict(os.environ, TL_GUARD_FLOW="legacy"))
    f_failed = failed_ids(legacy_log)
    mutants = report["tests"].get("nativeGuardMutantsCaught", [])
    report["baselineReproduction"] = {
        "kind": "expected-failure negative controls (NOT product evidence)",
        "startCommit": spec["commit"],
        "g": {"files": spec["files"], "expected": sorted(spec["expectedFailures"]), "observed": sorted(g_failed),
              "confirmed": set(spec["expectedFailures"]) <= g_failed, "log": str(log)},
        "fCompatCarrier": {"carrier": "app/ui/test/fixtures/legacy-guard-flow.mjs (control flow of the start version)",
                           "expected": sorted(legacy["expectedFailures"]), "observed": sorted(f_failed),
                           "confirmed": set(legacy["expectedFailures"]) <= f_failed, "log": str(legacy_log)},
        "fNativeMutants": {"expected": sorted(spec["nativeMutants"]), "caught": mutants,
                           "confirmed": sorted(spec["nativeMutants"]) == mutants},
    }
    report["baselineRegressionDetected"] = all(report["baselineReproduction"][k]["confirmed"]
                                               for k in ("g", "fCompatCarrier", "fNativeMutants"))
    save()


def smoke_checks(log_path):
    checks = []
    if log_path.is_file():
        for line in log_path.read_text(errors="replace").splitlines():
            match = re.search(r"\[ui\] smoke-check (\S+) a=(-?\d+) b=(-?\d+)", line)
            if match:
                checks.append((match.group(1), int(match.group(2)), int(match.group(3))))
    return checks


def our_pid(executable, data):
    for pid, command in processes():
        if command.startswith(str(executable) + " ") and f"--smoke-data-dir {data}" in command:
            return pid
    return None


def front_pid():
    front = capture(["lsappinfo", "front"], 10, check=False)
    info = capture(["lsappinfo", "info", "-only", "pid", front], 10, check=False)
    match = re.search(r'"pid"=(\d+)|pid\s*=\s*(\d+)', info)
    return int(next(g for g in match.groups() if g)) if match else None


def open_files_outside(pid, data):
    """Open paths of our process under the production ~/.deck (must be none)."""
    listing = capture(["lsof", "-nP", "-p", str(pid)], 20, check=False)
    home = str(HOME_DECK)
    return sorted({line.split()[-1] for line in listing.splitlines()
                   if home + "/" in line or line.rstrip().endswith(home)})


def network(pid):
    listing = capture(["lsof", "-nP", "-a", "-p", str(pid), "-i"], 20, check=False)
    return [line for line in listing.splitlines()[1:] if line.strip()]


def settle_guard(data, pid, executable, timeout=15):
    """Ask the live app to settle its pasteboard guard (it disarms Copied-text
    observation first) and return (code, audit rows). The reply must carry this
    request's nonce, so an older reply never stands in for it. None =
    unconfirmed (no reply within `timeout`, or the process is gone)."""
    fixture = data / "translation-fixture"
    result = fixture / "settle-result"
    if our_pid(executable, data) != pid:
        return None
    nonce = os.urandom(8).hex()
    staged = fixture / "settle-request.tmp"
    staged.write_text(nonce)
    os.replace(staged, fixture / "settle-request")
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if result.is_file():
            got, _, rest = result.read_text().partition("|")
            if got == nonce:
                code, _, audit = rest.partition("|")
                return int(code), parse_audit(audit)
        time.sleep(0.1)
    return None


def parse_audit(audit):
    rows = []
    for row in audit.split(";"):
        fields = row.split(",")
        if len(fields) == 7 and all(f.lstrip("-").isdigit() for f in fields):
            item = dict(zip(("id", "board", "writes", "rejects", "reason", "result", "modified"), map(int, fields)))
            item["resultName"] = RESULT_NAMES.get(item["result"], str(item["result"]))
            item["boardName"] = "general" if item["board"] == 0 else "named-test"
            item["modified"] = bool(item["modified"])
            rows.append(item)
    return rows


# Settle budget: a failed or unconfirmed settlement is retried while the only
# process holding the backup is alive; the process is terminated only after a
# confirmed final result or when the budget is spent (recorded as such).
SETTLE_ATTEMPT_TIMEOUTS = (3, 5, 8, 10)
DRIVER_MUTANT = os.environ.get("DECK_TL_DRIVER_MUTANT", "")  # negative controls only


def finish_instance(item, entry):
    """The one resource-finishing sequence for a test instance: stop new test
    writes and settle the guard through the live process (bounded retries) →
    record → terminate the process → clean its socket. Idempotent: a second
    call (the outer finally after a normal tail) changes nothing and keeps
    the first record."""
    record = item["record"]
    if item.get("finished"):
        record.setdefault("finishCalls", []).append({"entry": entry, "result": "already-finished"})
        return record["guardSettle"]
    executable, data = Path(item["executable"]), Path(item["data"])
    pid = item.get("pid") or our_pid(executable, data)
    item["pid"] = pid
    events = record.setdefault("finishEvents", [])
    t0 = time.monotonic()
    attempts, settled = [], None
    skip = DRIVER_MUTANT == "skip-cleanup-settle" and entry == "outer-cleanup"
    budget = SETTLE_ATTEMPT_TIMEOUTS[:1] if DRIVER_MUTANT == "no-settle-retry" else SETTLE_ATTEMPT_TIMEOUTS
    alive = bool(pid) and our_pid(executable, data) == pid
    if alive and not skip:
        for timeout in budget:
            if our_pid(executable, data) != pid:
                attempts.append({"t": round(time.monotonic() - t0, 2), "outcome": "process-gone"})
                break
            reply = settle_guard(data, pid, executable, timeout)
            if reply is None:
                attempts.append({"t": round(time.monotonic() - t0, 2), "outcome": "unconfirmed"})
                continue
            code, rows = reply
            pending = [r for r in rows if r["result"] in (0, 13)]
            attempts.append({"t": round(time.monotonic() - t0, 2), "outcome": "reply", "code": code,
                             "pendingGuards": len(pending)})
            settled = reply
            if not pending:
                break
    events.append({"event": "settle", "entry": entry, "attempts": attempts, "skipped": skip or not alive})
    rows = settled[1] if settled else None
    final_pending = [r for r in (rows or []) if r["result"] in (0, 13)]
    outcome = ("unconfirmed" if settled is None else "restore-failed" if final_pending else "settled")
    record["guardSettle"] = {"entry": entry, "confirmed": settled is not None and not final_pending,
                             "code": settled[0] if settled else None, "rows": rows, "outcome": outcome,
                             "attempts": len(attempts)}
    events.append({"event": "terminate", "t": round(time.monotonic() - t0, 2)})
    stop_process(pid, executable, data)
    item["finished"] = True
    record.setdefault("finishCalls", []).append({"entry": entry, "result": outcome})
    if outcome != "settled" and not record.get("killedGuard"):
        fail(f"run {record['run']} {record['mode']}: guard settlement {outcome} after {len(attempts)} attempt(s) "
             f"({entry}{', process already gone' if not alive else ''})")
    try:
        if os.environ.get("DECK_TL_FAULT_SOCKET") == "1" and not item.get("socketFaulted"):
            item["socketFaulted"] = True
            raise RuntimeError("injected socket cleanup fault")
        stop_socket(item["socket"], Path(item["bundle"]))
    except Exception as error:  # one resource's failure never stops the others
        fail(f"cleanup {item['socket']}: {type(error).__name__}: {error}")
    return record["guardSettle"]


def release_named_board(pid):
    subprocess.run(["osascript", "-l", "JavaScript", "-e", JXA_RELEASE, f"io.c9r.deck.smoke.translation.{pid}"],
                   capture_output=True, timeout=20)


RUN_RECORDS = []


def run_mode(index, mode, bundle, cache, pack_id, assets, scenario="", probe=None):
    data = (RUN / f"run-{index}" / mode / "data")
    data.mkdir(parents=True, mode=0o700)
    for parent in (data.parent, data.parent.parent):
        os.chmod(parent, 0o700)
    data = data.resolve()
    if str(data).startswith(str(HOME_DECK)) or not str(data).startswith(str(RUN)):
        raise RuntimeError("data root is not inside the run root")
    fixture_dir = data / "translation-fixture"
    fixture_dir.mkdir(mode=0o700)
    shutil.copyfile(ROOT / "scripts/translation_lens_fixture.py", fixture_dir / "copy_agent.py")
    os.chmod(fixture_dir / "copy_agent.py", 0o600)
    pack_dir = install_pack(data, cache, pack_id, assets) if mode == "translation-native" else None
    socket = f"deck-smoke-tl{RUN_ID}{index}{'n' if mode == 'translation-native' else 'u'}"
    executable = bundle / "Contents/MacOS/deck"
    record = {"run": index, "mode": mode, "dataRoot": str(data), "socket": socket, "actions": [],
              "isolation": {}, "network": [], "timeout": False, "checks": {}, "verdictExit": None,
              "screenshots": [], "complete": False, "scenario": scenario}
    if scenario:
        (fixture_dir / "scenario").write_text(scenario)
    # Registered BEFORE any side effect: an exception anywhere below still
    # leaves this record in the report and this instance to the outer cleanup.
    RUN_RECORDS.append(record)
    item = {"pid": None, "executable": str(bundle / "Contents/MacOS/deck"), "data": str(data), "socket": socket,
            "bundle": str(bundle), "record": record}
    owned["processes"].append(item)
    report["ownedResources"].append({"kind": "tmux-socket", "name": socket})
    busy = [command for _, command in processes() if re.search(r"(^|/)(rustc|cargo) ", command + " ")]
    record["concurrentBuildProcesses"] = len(busy)
    record["loadAverage"] = os.getloadavg()
    started = time.monotonic()
    capture(["open", "-n", str(bundle), "--args", "--smoke-data-dir", str(data), "--smoke-tmux-socket", socket,
             "--smoke-wkwebview", mode], 60)
    pid = None
    deadline = time.monotonic() + 60
    while time.monotonic() < deadline and not pid:
        pid = our_pid(executable, data)
        time.sleep(0.2)
    if not pid:
        raise RuntimeError(f"{mode}: isolated app did not start")
    item["pid"] = pid
    # A bundle launched from a background process may not be activated; the
    # Lens (correctly) does nothing unfocused. Bring OUR bundle forward.
    front_deadline = time.monotonic() + 20
    while time.monotonic() < front_deadline and front_pid() != pid:
        capture(["open", str(bundle)], 30, check=False)
        time.sleep(1.5)
    record["frontAtStart"] = front_pid() == pid
    report["ownedResources"].append({"kind": "process", "pid": pid, "executable": str(executable), "mode": mode, "run": index})
    save()
    log_path = data / "app.log"
    handled, done = set(), False
    limit = MANIFEST["fixedThresholds"]["guiModeTimeoutS"]
    samples = 0
    while time.monotonic() - started < limit:
        time.sleep(0.2)
        if not our_pid(executable, data):
            break
        entries = smoke_checks(log_path)
        names = [name for name, _, _ in entries]
        if mode == "translation-guard" and "tl-f06-await-kill" in names:
            # F06 kill: SIGKILL the process that alone holds the named guard's
            # backup. The only honest classification is "unconfirmed".
            if our_pid(executable, data) == pid:
                os.kill(pid, signal.SIGKILL)
                time.sleep(1)
            kill_id = next(b for n, _, b in entries if n == "tl-f06-await-kill")
            result = data / "translation-fixture/settle-result"
            settled_rows = result.read_text().partition("|")[2] if result.is_file() else ""
            confirmed = any(row.split(",")[0] == str(kill_id) and row.split(",")[-1] not in ("0", "")
                            for row in settled_rows.split(";") if row.count(",") == 5)
            record["killedGuard"] = {"id": kill_id, "board": "named-test",
                                     "classification": "confirmed" if confirmed else "unconfirmed",
                                     "countedAsSuccess": False}
            release_named_board(pid)
            done = "done" in names
            break
        if mode != "translation-guard" and "done" in names:
            done = True
            break
        if samples % 5 == 0:
            current = front_pid()
            if current and current != pid:
                label = f"pid {current}: " + " ".join(capture(["ps", "-o", "comm=", "-p", str(current)], 10,
                                                              check=False).split("/")[-1:])
                last = entries[-1][0] if entries else ""
                record.setdefault("foreignFront", []).append({"after": last, "app": label})
        if samples % 25 == 0:
            outside = open_files_outside(pid, data)
            if outside:
                record["isolation"]["openFilesUnderHomeDeck"] = outside
            if mode == "translation-native":
                record["network"].extend(network(pid))
        samples += 1
        for name in names:
            if name in handled:
                continue
            if name in ("tl-a06-await-return", "tl-n-a06-await-return"):
                handled.add(name)
                time.sleep(2.5)
                if front_pid() != pid:
                    capture(["open", str(bundle)], 30)
                    record["actions"].append({"at": name, "action": "LaunchServices activate"})
                else:
                    record["actions"].append({"at": name, "action": "returned via NSApp.activate"})
            elif name == "tl-n-c04-await-away":
                handled.add(name)
                version = next(b for n, _, b in entries if n == name)
                wait = time.monotonic() + 5
                while time.monotonic() < wait and front_pid() == pid:
                    time.sleep(0.1)
                away = front_pid() != pid
                wrote = "refused"
                if away and version > 0:
                    wrote = capture(["osascript", "-l", "JavaScript", "-e", JXA_WRITE, AWAY_TEXT, str(version)],
                                    20, check=False)
                    if wrote.isdigit():
                        receipt = data / "translation-fixture/copy-receipt"
                        receipt.write_text(wrote)
                time.sleep(0.5)
                capture(["open", str(bundle)], 30)
                record["actions"].append({"at": name, "action": "compare-and-write of synthetic away text, then activate",
                                          "deckWasAway": away, "written": wrote.isdigit()})
            elif name == "tl-f06-hold" and probe:
                # A REAL driver fault inside run_mode: the instance is left to the
                # outer except/finally, which must settle before terminating.
                handled.add(name)
                record["actions"].append({"at": name, "action": f"inject driver fault: {probe}"})
                save()
                if probe == "timeout":
                    capture(["sleep", "5"], timeout=1)
                if probe == "cancel":
                    os.kill(os.getpid(), signal.SIGINT)
                    time.sleep(5)
                raise RuntimeError(f"injected driver fault ({probe})")
            elif name == "tl-f06-await-cancel":
                handled.add(name)
                settled = settle_guard(data, pid, executable)
                record["cancelSettle"] = {"confirmed": settled is not None,
                                          "code": settled[0] if settled else None,
                                          "rows": settled[1] if settled else None,
                                          "beforeTeardown": our_pid(executable, data) == pid}
                capture(["open", str(bundle)], 30, check=False)
                record["actions"].append({"at": name, "action": "driver cancel: settle-request while hidden, then activate"})
            elif name == "tl-n-d07-await-corrupt" and pack_dir:
                handled.add(name)
                target = pack_dir / "model.enzh.intgemm.alphas.bin"
                with open(target, "r+b") as file:
                    file.seek(4096)
                    byte = file.read(1)
                    file.seek(4096)
                    file.write(bytes([byte[0] ^ 0xFF]))
                record["actions"].append({"at": name, "action": "flipped one byte of the isolated pack copy"})
            elif name == "tl-n-d07-await-missing" and pack_dir:
                handled.add(name)
                shutil.rmtree(pack_dir)
                record["actions"].append({"at": name, "action": "removed the isolated pack copy"})
            elif name == "tl-n-d07-await-restore" and pack_dir:
                handled.add(name)
                install_pack(data, cache, pack_id, assets)
                record["actions"].append({"at": name, "action": "restored the verified pack copy"})
    record["timeout"] = not done
    record["seconds"] = round(time.monotonic() - started, 1)
    if mode == "translation-native" and our_pid(executable, data):
        record["network"].extend(network(pid))
    record["network"] = sorted(set(record["network"]))
    if pack_dir is not None:
        record["packParentEntries"] = sorted(p.name for p in pack_dir.parent.iterdir())
    # Settle BEFORE teardown: the backup lives only in this process.
    finish_instance(item, "normal")
    if mode == "translation-guard" and record.get("killedGuard"):
        record["guardSettle"]["expectedUnconfirmed"] = "process killed on purpose (F06); named board only"
    checks = smoke_checks(log_path)
    mode_evidence = EVIDENCE / f"run-{index}" / mode
    mode_evidence.mkdir(parents=True, mode=0o700)
    (mode_evidence / "smoke-checks.log").write_text("".join(f"{n} a={a} b={b}\n" for n, a, b in checks))
    for image in (data / "evidence").glob("*.png") if (data / "evidence").is_dir() else []:
        shutil.copy2(image, mode_evidence / image.name)
    record["screenshots"] = sorted(p.name for p in mode_evidence.glob("*.png"))
    verdict_log = mode_evidence / "smoke-verdict.log"
    record["verdictExit"] = run_logged([str(ROOT / "scripts/smoke-verdict"), str(data), mode], verdict_log, 60)
    record["checks"] = {}
    metrics = {name for name, rule in SMOKE_MANIFEST["modes"][mode]["checks"].items() if rule.get("metric")}
    for name, a, b in checks:
        slot = record["checks"].setdefault(name, {"passed": True, "values": []})
        # A metric is a measurement (0 ms is valid); a check passes only with a > 0.
        slot["passed"] = slot["passed"] and (a >= 0 if name in metrics else a > 0)
        slot["values"].append([a, b])
    record["exception"] = record["checks"].get("tl-exception", {}).get("values")
    record["complete"] = True
    # times another app took focus mid-run and the smoke window re-activated itself
    record["frontRegains"] = sum(v[0] for v in record["checks"].get("tl-front-regains", {}).get("values", []))
    return record


def stop_process(pid, executable, data):
    if our_pid(executable, data) != pid:
        return
    os.kill(pid, signal.SIGTERM)
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline and our_pid(executable, data) == pid:
        time.sleep(0.2)
    if our_pid(executable, data) == pid:
        os.kill(pid, signal.SIGKILL)
        time.sleep(0.5)


def stop_socket(socket, bundle):
    tmux = bundle / "Contents/MacOS/tmux"
    subprocess.run([str(tmux), "-L", socket, "kill-server"], capture_output=True, timeout=20)
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline and any(f"-L {socket} " in command + " " for _, command in processes()):
        time.sleep(0.2)
    # A server whose control client was orphaned by a killed app can stop
    # answering kill-server: terminate exactly this bundle's tmux processes
    # for this socket, by PID (identity: executable path + socket argument).
    for signum in (signal.SIGTERM, signal.SIGKILL):
        left = [pid for pid, command in processes()
                if command.split(" ", 1)[0] == str(tmux) and f"-L {socket} " in command + " "]
        for pid in left:
            os.kill(pid, signum)
        if left:
            time.sleep(1)


def cleanup():
    audit = {"processesRemaining": [], "socketsRemaining": [], "caffeinateReleased": None}
    for item in owned["processes"]:
        # Same sequence as the normal tail: settle through the live process,
        # THEN terminate. A no-op for instances the normal tail finished.
        try:
            finish_instance(item, "outer-cleanup")
        except Exception as error:  # keep going: the other resources still need finishing
            fail(f"cleanup {item['socket']}: {type(error).__name__}: {error}")
        pid = item.get("pid") or our_pid(Path(item["executable"]), Path(item["data"]))
        if pid:
            release_named_board(pid)
        if our_pid(Path(item["executable"]), Path(item["data"])):
            audit["processesRemaining"].append(pid)
        try:
            stop_socket(item["socket"], Path(item["bundle"]))
        except Exception as error:
            fail(f"cleanup {item['socket']}: {type(error).__name__}: {error}")
        if any(f"-L {item['socket']} " in command + " " for _, command in processes()):
            audit["socketsRemaining"].append(item["socket"])
        inventory = subprocess.run([sys.executable, str(ROOT / "scripts/edr_runtime.py"), "--json", "--socket",
                                    item["socket"]], cwd=ROOT, capture_output=True, text=True, timeout=60)
        audit.setdefault("edrRuntimeInventoryExit", []).append(inventory.returncode)
    # settlement outcome of every instance, kept apart from process cleanup
    audit["guardSettlements"] = [{"run": i["record"]["run"], "mode": i["record"]["mode"],
                                  **{k: v for k, v in (i["record"].get("guardSettle") or {}).items() if k != "rows"}}
                                 for i in owned["processes"]]
    if owned["caffeinate"]:
        owned["caffeinate"].terminate()
        try:
            owned["caffeinate"].wait(10)
        except subprocess.TimeoutExpired:
            owned["caffeinate"].kill()
        audit["caffeinateReleased"] = owned["caffeinate"].poll() is not None
    report["cleanupAudit"] = audit
    report["cleanupCompleted"] = (not audit["processesRemaining"] and not audit["socketsRemaining"]
                                  and audit["caffeinateReleased"] is not False)


def evaluate(l1_results, rust_results, runs):
    executed = passed = 0
    per_id = {}
    for test_id, spec in MANIFEST["required"].items():
        items = []
        for tag in spec.get("l1", []):
            hits = [ok for title, ok in l1_results.items() if f"[{tag}]" in title]
            items.append({"layer": "L1", "ref": tag, "executed": bool(hits), "passed": bool(hits) and all(hits)})
        for name in spec.get("rust", []):
            items.append({"layer": "L1-rust", "ref": name, "executed": name in rust_results,
                          "passed": rust_results.get(name) is True})
        for name in spec.get("native", []):
            ok = report["tests"].get("nativeGuardHarnessExit") == 0
            items.append({"layer": "L1-native", "ref": f"scripts/test-smoke-guard {name}", "executed": "nativeGuardHarnessExit" in report["tests"],
                          "passed": ok})
        for layer, mode in (("l2", "translation"), ("l3", "translation-native"), ("lg", "translation-guard")):
            for name in spec.get(layer, []):
                states = [run["checks"].get(name) for run in runs if run["mode"] == mode]
                ran = len(states) == RUNS and all(states)
                items.append({"layer": layer.upper(), "ref": name, "executed": ran,
                              "passed": ran and all(s["passed"] for s in states)})
        for name in spec.get("driver", []):
            ok = report["driverChecks"].get(name)
            items.append({"layer": "driver", "ref": name, "executed": ok is not None, "passed": ok is True})
        ran = all(i["executed"] for i in items)
        ok = ran and all(i["passed"] for i in items)
        executed += ran
        passed += ok
        per_id[test_id] = {"executed": ran, "passed": ok, "evidence": items}
        if not ran:
            report["skippedRequired"].append(test_id)
    report["tests"]["required"] = per_id
    report["executedCount"], report["passedCount"] = executed, passed
    report["failedCount"] = report["requiredCount"] - passed


# Driver fault probes: the REAL driver is run as a child against a held,
# already-modified NAMED guard and fails inside run_mode (exception, command
# timeout, SIGINT). kind -> (JS scenario, driver fault, expected final guard
# result). The parent judges the child's own report.
PROBES = {
    "exception": ("hold", "exception", "restored"),
    "timeout": ("hold", "timeout", "restored"),
    "cancel": ("hold", "cancel", "restored"),
    "restore-retry": ("hold:restore-fail-1", "exception", "restored"),
    "late-reply": ("hold:late-reply", "exception", "restored"),
    "exhausted": ("hold:restore-fail-99", "exception", "restore-failed"),
    "fill-fail": ("hold:fill-fail", "exception", "restored"),
    "fill-fail-empty": ("hold:fill-fail:empty", "exception", "restored"),
    "fill-fail-external": ("hold:fill-fail:external", "exception", "external-kept"),
    "cleanup-error": ("hold", "exception", "restored"),
}


def run_probe(kind, bundle, parent_dir, mutant=""):
    """Run one child driver fault probe; return the judged evidence."""
    env = dict(os.environ, DECK_TL_DRIVER_MUTANT=mutant)
    env.pop("DECK_TL_FAULT_SOCKET", None)
    if kind == "cleanup-error":
        env["DECK_TL_FAULT_SOCKET"] = "1"
    log = parent_dir / f"probe-{kind}{'-' + mutant if mutant else ''}.log"
    done = subprocess.run([sys.executable, str(Path(__file__).resolve()), "--fault-probe", kind, "--bundle", str(bundle)],
                          cwd=ROOT, env=env, capture_output=True, text=True, timeout=300)
    log.write_text(done.stdout + done.stderr)
    evidence = {"kind": kind, "mutant": mutant or None, "childExit": done.returncode, "passed": False}
    try:
        child = json.loads(Path(json.loads(done.stdout[done.stdout.index("{"):])["report"]).read_text())
    except (ValueError, KeyError, OSError):
        evidence["reason"] = "no child report"
        return evidence
    runs = child.get("runs") or []
    record = runs[0] if runs else {}
    settle = record.get("guardSettle") or {}
    events = record.get("finishEvents") or []
    rows = settle.get("rows") or []
    held = rows[-1] if rows else {}
    attempts = (events[0].get("attempts") if events else []) or []
    order_ok = [e["event"] for e in events[:2]] == ["settle", "terminate"] and (
        not attempts or events[1]["t"] >= attempts[-1]["t"])
    scenario, fault, expected = PROBES[kind]
    final = held.get("resultName") if settle.get("outcome") != "unconfirmed" else "unconfirmed"
    evidence.update({
        "childReport": child.get("evidencePaths", {}).get("report"), "entry": settle.get("entry"),
        "attempts": attempts, "final": final, "modified": held.get("modified"), "writes": held.get("writes"),
        "settleBeforeTerminate": order_ok, "childVerdict": child.get("verdict"),
        "firstFailure": (child.get("failures") or [""])[0][:120],
        "processesRemaining": child.get("cleanupAudit", {}).get("processesRemaining"),
    })
    checks = [
        settle.get("entry") == "outer-cleanup", order_ok, final == expected, held.get("modified") is True,
        child.get("verdict") != "PASS", done.returncode != 0,
        not child.get("cleanupAudit", {}).get("processesRemaining"),
    ]
    if kind == "restore-retry":
        checks.append(len(attempts) >= 2 and attempts[0].get("pendingGuards") == 1)
    if kind == "late-reply":
        checks.append(attempts and attempts[0]["outcome"] == "unconfirmed")
    if kind == "exhausted":
        checks += [settle.get("outcome") == "restore-failed", len(attempts) == len(SETTLE_ATTEMPT_TIMEOUTS),
                   any("restore-failed" in f for f in child.get("failures", []))]
    if kind.startswith("fill-fail"):
        checks.append(held.get("writes") == 0)
    if kind == "cleanup-error":
        failures = child.get("failures", [])
        checks += [failures and "injected driver fault" in failures[0],
                   any("injected socket cleanup fault" in f for f in failures),
                   not child.get("cleanupAudit", {}).get("socketsRemaining")]
    if kind == "cancel":
        checks.append(child.get("cancelled") is True)
    evidence["passed"] = all(bool(c) for c in checks)
    return evidence


def guard_results(runs):
    """Per guard, content-free: board, writes, refusals, reason, final result."""
    out = []
    for run in runs:
        rows = (run.get("guardSettle") or {}).get("rows") or (run.get("cancelSettle") or {}).get("rows") or []
        if run["mode"] == "translation-guard" and run.get("cancelSettle", {}).get("rows"):
            rows = run["cancelSettle"]["rows"]
        for row in rows:
            out.append({"run": run["run"], "mode": run["mode"], **row})
        if run.get("killedGuard"):
            out.append({"run": run["run"], "mode": run["mode"], "id": None, "boardName": "named-test",
                        "resultName": run["killedGuard"]["classification"], "expectedNegative": "F06 kill"})
        if not run.get("guardSettle", {}).get("confirmed") and not run.get("killedGuard"):
            out.append({"run": run["run"], "mode": run["mode"], "id": None, "boardName": "unknown",
                        "resultName": "unconfirmed"})
    return out


def driver_checks(runs):
    removed = subprocess.run(["git", "grep", "-n", "-E",
                              "translation_clipboard_current|clipboardCurrent|translation-use-current|translation-resume|translation\\.useCurrent|translation\\.paused",
                              "--", "app/ui/js", "app/ui/index.html", "app/src-tauri/src"],
                             cwd=ROOT, capture_output=True, text=True)
    old_names = {"translation-pause", "translation-frozen", "translation-resume", "translation-explicit-current",
                 "translation-native-explicit", "translation-native-await-away"}
    modes = SMOKE_MANIFEST["modes"]
    native_runs = [r for r in runs if r["mode"] == "translation-native"]
    report["driverChecks"] = {
        "removed-entry-points": removed.returncode == 1 and not removed.stdout.strip(),
        "old-contract-removed": not any(n in modes[m]["checks"] for m in MODES for n in old_names),
        "verdict-translation": len([r for r in runs if r["mode"] == "translation" and r["verdictExit"] == 0]) == RUNS,
        "verdict-translation-native": len([r for r in native_runs if r["verdictExit"] == 0]) == RUNS,
        "verdict-translation-guard": len([r for r in runs if r["mode"] == "translation-guard" and r["verdictExit"] == 0]) == RUNS,
        "no-network": len(native_runs) == RUNS and all(not r["network"] for r in native_runs),
        "no-download": len(native_runs) == RUNS and all(not any(".staging" in e or "staging" in e for e in r.get("packParentEntries", []))
                                                          for r in native_runs),
        "screenshots": all(r["screenshots"] for r in runs if r["mode"] != "translation-guard") and len(runs) == len(MODES) * RUNS,
        "isolation": report["isolationVerified"],
        "cleanup": None,
    }
    results = guard_results(runs)
    report["clipboardGuardResults"] = results
    general = [r for r in results if r["boardName"] == "general"]
    guard_runs = [r for r in runs if r["mode"] == "translation-guard"]
    report["driverChecks"].update({
        # every general guard settled to "restored" or "not written", confirmed by the live process
        "shared-resource-safety": bool(general) and all(r["resultName"] in ("restored", "not-written") for r in general)
            and all(r.get("guardSettle", {}).get("confirmed") for r in runs if r["mode"] != "translation-guard")
            and not any(r["resultName"] == "unconfirmed" and r["boardName"] != "named-test" for r in results),
        "general-writes-only-in-native-mode": all(r["mode"] == "translation-native" for r in general),
        "f06-cancel-settled-before-teardown": len(guard_runs) == RUNS and all(
            (r.get("cancelSettle") or {}).get("confirmed") and r["cancelSettle"]["code"] == 11
            and r["cancelSettle"]["beforeTeardown"] for r in guard_runs),
        "f06-kill-reported-unconfirmed": len(guard_runs) == RUNS and all(
            (r.get("killedGuard") or {}).get("classification") == "unconfirmed"
            and not r["killedGuard"]["countedAsSuccess"] for r in guard_runs),
    })
    report["externalInterference"] = [r for r in general if r["resultName"] == "external-kept"]
    probes = report.get("probes", [])
    for kind in PROBES:
        mine = [p for p in probes if p["kind"] == kind]
        report["driverChecks"][f"probe-{kind}"] = len(mine) == RUNS and all(p["passed"] for p in mine)
    controls = report.get("driverMutantControls", [])
    report["driverChecks"]["driver-mutants-caught"] = len(controls) == 2 and all(c["caught"] for c in controls)


def timings(runs):
    samples = {}
    for run in runs:
        for name, slot in run["checks"].items():
            if name.startswith("tl-t-") or name in ("tl-b07-requests", "tl-b07-dom", "tl-b02-final", "tl-n-b02-final",
                                                     "tl-n-c02-near-arm", "tl-b07-responsive"):
                samples.setdefault(name, []).extend(v[0] for v in slot["values"])
    report["timingSamples"] = {name: {"n": len(v), "raw": v, "min": min(v), "median": statistics.median(v), "max": max(v)}
                               for name, v in sorted(samples.items()) if v}


def probe_main(kind, bundle):
    """Child of run_probe: one guard-mode instance, a real fault inside run_mode."""
    report["probe"] = kind
    report["sourceCommit"] = capture(["git", "rev-parse", "HEAD"])
    if screen_locked():
        fail("BLOCKED: the macOS GUI session is locked")
        return 2
    report["environmentReady"] = True
    scenario, fault, _ = PROBES[kind]
    report["runs"] = RUN_RECORDS
    run_mode(1, "translation-guard", Path(bundle), None, None, None, scenario=scenario, probe=fault)
    fail("probe finished without its injected fault")
    return None


def main():
    save()
    if "--fault-probe" in sys.argv:
        return probe_main(sys.argv[sys.argv.index("--fault-probe") + 1], sys.argv[sys.argv.index("--bundle") + 1])
    report["sourceCommit"] = capture(["git", "rev-parse", "HEAD"])
    report["branch"] = capture(["git", "branch", "--show-current"])
    report["gitStatusAtStart"] = capture(["git", "status", "--short"]).splitlines()
    report["dirtyPatchDigest"] = dirty_digest()
    report["testedSourceDigest"] = source_digest()
    report["toolchain"] = {
        "rustc": capture(["rustc", "-V"]), "cargo": capture(["cargo", "-V"]), "node": capture(["node", "-v"]),
        "python": sys.version.split()[0], "swiftc": capture(["xcrun", "swiftc", "--version"]).splitlines()[0],
        "macOS": capture(["sw_vers", "-productVersion"]),
        "inputSource": capture(["defaults", "read", str(Path.home() / "Library/Preferences/com.apple.HIToolbox"),
                                "AppleCurrentKeyboardLayoutInputSourceID"], check=False),
    }
    production_before = production_pids()
    report["productionDeckPidsBefore"] = production_before
    save()
    if screen_locked():
        fail("BLOCKED: the macOS GUI session is locked; the isolated WKWebView runs need an unlocked session")
        report["environmentReady"] = False
        return 2
    # The smoke window restricts its own input context to Roman sources
    # (SmokeBridge deck_smoke_roman_input); the user's selection is recorded only.
    cache, pack_id, assets = prepare_model()
    bundle = build_bundle()
    if source_digest() != report["testedSourceDigest"]:
        raise RuntimeError("sources changed during the build")
    report["environmentReady"] = True
    save()
    # DECK_TL_QUICK=1 (iteration only) skips L1 and the negative controls; such
    # a run leaves required evidence unexecuted and can never report PASS.
    if os.environ.get("DECK_TL_QUICK") != "1":
        l1_results, rust_results = l1()
        report["tests"]["l1"] = l1_results
        report["tests"]["rust"] = rust_results
        save()
        baseline()
    else:
        report["tests"]["l1"], report["tests"]["rust"] = {}, {}
        fail("iteration run: L1 and negative controls skipped (DECK_TL_QUICK=1)")
    owned["caffeinate"] = subprocess.Popen(["caffeinate", "-d", "-i", "-w", str(os.getpid())])
    report["ownedResources"].append({"kind": "caffeinate", "pid": owned["caffeinate"].pid})
    runs = RUN_RECORDS
    report["runs"] = runs
    # Negative controls for the driver fixes (named board only): with the
    # driver mutants the probe assertions must FAIL.
    controls = []
    for kind, mutant in (("exception", "skip-cleanup-settle"), ("restore-retry", "no-settle-retry")):
        evidence = run_probe(kind, bundle, EVIDENCE, mutant)
        controls.append({**evidence, "caught": not evidence["passed"]})
    report["driverMutantControls"] = controls
    report["probes"] = []
    for index in range(1, RUNS + 1):
        for mode in RUN_MODES:
            if screen_locked():
                fail(f"BLOCKED: GUI session locked before run {index} {mode}")
                report["runs"] = runs
                return 2
            if mode == "translation-native" and any(
                    r["run"] == index and r["mode"] == "translation-guard"
                    and (r["verdictExit"] or any(not c["passed"] for c in r["checks"].values())) for r in runs):
                fail(f"run {index}: named-board harness safety failed; the general-pasteboard mode was NOT run")
                report["runs"] = runs
                return None
            if mode == "translation" and any(r["run"] == index and r["mode"] == "translation-guard" for r in runs):
                # driver fault probes on the named board, right after this run's safety mode
                for kind in PROBES:
                    evidence = run_probe(kind, bundle, EVIDENCE / f"run-{index}")
                    evidence["run"] = index
                    report["probes"].append(evidence)
                    save()
                if not all(p["passed"] for p in report["probes"] if p["run"] == index):
                    fail(f"run {index}: a driver fault probe failed; the general-pasteboard mode will not run")
            if mode == "translation-native" and any(not p["passed"] for p in report["probes"] if p["run"] == index):
                return None
            (EVIDENCE / f"run-{index}").mkdir(parents=True, exist_ok=True, mode=0o700)
            record = run_mode(index, mode, bundle, cache, pack_id, assets)
            report["runs"] = runs
            save()
    report["isolationChecks"] = {
        "bundleIdentifierDiffersFromProduction": report["bundleIdentifier"].startswith("io.c9r.deck.smoke.tl"),
        "bundleOutsideApplications": not report["bundlePath"].startswith("/Applications/"),
        "socketsArePrivateSmoke": all(r["socket"].startswith("deck-smoke-") for r in runs),
        "dataRootsInsideRunRoot": all(r["dataRoot"].startswith(str(RUN) + "/") for r in runs),
        "noOpenFilesUnderHomeDeck": all(not r["isolation"] for r in runs),
        "productionDeckPidsUnchanged": production_pids() == production_before,
        "appLogWrittenToIsolatedRoot": all(Path(r["dataRoot"], "app.log").is_file() for r in runs),
    }
    report["isolationVerified"] = all(report["isolationChecks"].values())
    report["productionDeckPidsAfter"] = production_pids()
    driver_checks(runs)
    timings(runs)
    for run in runs:
        failures = sorted(name for name, slot in run["checks"].items() if not slot["passed"])
        if failures or run["timeout"] or run["verdictExit"]:
            fail(f"run {run['run']} {run['mode']}: failed checks {failures}, timeout={run['timeout']}, verdictExit={run['verdictExit']}, exception={run['exception']}")
    return None


if __name__ == "__main__":
    code = None
    try:
        code = main()
    except BaseException as error:  # the report is always written; SIGINT cancels land here too
        fail(f"{type(error).__name__}: {error}")
        if isinstance(error, KeyboardInterrupt):
            report["cancelled"] = True
    finally:
        try:
            cleanup()
        except Exception as error:
            fail(f"cleanup: {type(error).__name__}: {error}")
        report.setdefault("driverChecks", {})
        report["driverChecks"]["cleanup"] = report["cleanupCompleted"]
        # a normal tail finished each instance once; the outer cleanup found it done
        finished = [r for r in RUN_RECORDS if r.get("complete")]
        report["driverChecks"]["idempotent-cleanup"] = bool(finished) and all(
            [c["entry"] for c in r.get("finishCalls", [])] == ["normal", "outer-cleanup"]
            and r["finishCalls"][1]["result"] == "already-finished" for r in finished)
        report["runs"] = RUN_RECORDS
        if "runs" in report and report["runs"]:
            evaluate(report["tests"].get("l1", {}), report["tests"].get("rust", {}), report["runs"])
        report["sharedClipboardHandling"] = {
            "isolation": "the general pasteboard is shared with the whole login session; it is NOT isolated by the separate bundle",
            "gates": "every test write is refused BEFORE touching a board unless an active guard holds a stable backup and the "
                     "board is still at the guard's owned version; non-guard writers (fixture /copy, the Lens writer, this "
                     "driver) need a permit and are adopted only by their own clearContents() receipt, never by text",
            "residualRace": "NSPasteboard has no cross-process compare-and-swap: between a version check and clearContents() "
                            "another process can still write; the window is narrowed, not closed",
            "restore": "settle disarms Copied-text observation first; restores only while the owned version is current; a failed "
                       "restore keeps the backup; the driver settles through the live process before terminating it",
            "faultInjection": "only on the test-owned named pasteboard (translation-guard mode, scripts/test-smoke-guard)",
            "writesByThisScript": "one compare-and-write of a synthetic sentence during C04, inside a permitted guard",
            "results": report.get("clipboardGuardResults", []),
        }
        report["sourceDigestAfter"] = source_digest()
        report["testedTreeUnchanged"] = report["sourceDigestAfter"] == report["testedSourceDigest"]
        report["requiredTestsExecuted"] = (report["executedCount"] == report["requiredCount"]
                                           and not report["skippedRequired"])
        req = report["tests"].get("required", {})
        harness_ids = set(MANIFEST["harnessSafetyIds"])
        report["functionalAssertionsPassed"] = bool(req) and all(v["passed"] for k, v in req.items() if k not in harness_ids)
        report["baselineRegressionDetected"] = (report.get("baselineRegressionDetected") is True
                                                and report.get("driverChecks", {}).get("driver-mutants-caught") is True)
        report["harnessSafetyPassed"] = (bool(req) and all(req[k]["passed"] for k in harness_ids if k in req)
                                         and report["tests"].get("nativeGuardHarnessExit") == 0
                                         and report.get("baselineRegressionDetected") is True)
        report["processCleanupCompleted"] = report["cleanupCompleted"]
        report["sharedResourceSafetyPassed"] = report.get("driverChecks", {}).get("shared-resource-safety") is True
        report["productAssertionsPassed"] = (report["passedCount"] == report["requiredCount"] and not report["failures"])
        passing = (report["environmentReady"] and report["isolationVerified"] and report["requiredTestsExecuted"]
                   and report["productAssertionsPassed"] and report["cleanupCompleted"]
                   and report["functionalAssertionsPassed"] and report["harnessSafetyPassed"]
                   and report["sharedResourceSafetyPassed"]
                   and report["humanInterventions"] == 0 and report["testedTreeUnchanged"]
                   and report["tests"].get("l1NodeExit") == 0 and report["tests"].get("l1CargoExit") == 0
                   and report["tests"].get("uiGateExit") == 0
                   and report["tests"].get("cargoWorkspaceExit") == 0 and report["tests"].get("clippyExit") == 0
                   and report["tests"].get("nativeGuardHarnessExit") == 0)
        if code == 2 or not report["environmentReady"] or report.get("externalInterference"):
            report["verdict"] = "BLOCKED"
        else:
            report["verdict"] = "PASS" if passing else "FAIL"
        report["finishedAt"] = datetime.datetime.now().isoformat(timespec="seconds")
        save()
        print(json.dumps({"verdict": report["verdict"], "report": str(REPORT_PATH),
                          "passed": report["passedCount"], "required": report["requiredCount"],
                          "failures": report["failures"][:10]}, indent=2, ensure_ascii=False))
    sys.exit({"PASS": 0, "FAIL": 1}.get(report["verdict"], 2))
