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
MODES = ("translation", "translation-native")
# Iteration aid only: a subset of modes can never satisfy the manifest, so
# such a run is reported FAIL (missing evidence), never PASS.
RUN_MODES = tuple(m for m in os.environ.get("DECK_TL_MODES", ",".join(MODES)).split(",") if m in MODES)
AWAY_TEXT = "Deck harmless text copied while Deck was away."
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
    return sorted(pid for pid, command in processes() if command.startswith("/Applications/deck.app/"))


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
    gate_log = EVIDENCE / "ui-tests.log"
    report["tests"]["uiGateExit"] = run_logged(["sh", "scripts/ui-tests"], gate_log, 900)
    return results, rust


def baseline():
    spec = MANIFEST["baseline"]
    tree = RUN / "baseline"
    tree.mkdir(mode=0o700)
    archive = subprocess.run(["git", "archive", spec["commit"], "app/ui"], cwd=ROOT, capture_output=True, timeout=120)
    subprocess.run(["tar", "-x", "-C", str(tree)], input=archive.stdout, check=True, timeout=120)
    shutil.copy2(ROOT / spec["file"], tree / spec["file"])
    log = EVIDENCE / "baseline-reproduction.tap"
    run_logged(["node", "--test", "--test-reporter=tap", spec["file"]], log, 300, cwd=tree)
    failed = set()
    for line in log.read_text().splitlines():
        match = re.match(r"^not ok \d+ - (.*)$", line.strip())
        if match:
            failed.update(re.findall(r"\[([A-E]\d\d)\]", match.group(1)))
    expected = set(spec["expectedFailures"])
    report["baselineReproduction"] = {
        "kind": "expected-failure negative control (NOT product evidence)", "commit": spec["commit"],
        "test": spec["file"], "expectedFailures": sorted(expected), "observedFailures": sorted(failed),
        "confirmed": expected <= failed, "log": str(log)}
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


def run_mode(index, mode, bundle, cache, pack_id, assets):
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
              "isolation": {}, "network": [], "timeout": False}
    owned["sockets"].append({"socket": socket, "tmux": str(bundle / "Contents/MacOS/tmux")})
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
    owned["processes"].append({"pid": pid, "executable": str(executable), "data": str(data)})
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
        names = [name for name, _, _ in smoke_checks(log_path)]
        if "done" in names:
            done = True
            break
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
                wait = time.monotonic() + 5
                while time.monotonic() < wait and front_pid() == pid:
                    time.sleep(0.1)
                away = front_pid() != pid
                subprocess.run(["pbcopy"], input=AWAY_TEXT.encode(), check=True, timeout=10)
                time.sleep(0.5)
                capture(["open", str(bundle)], 30)
                record["actions"].append({"at": name, "action": "pbcopy synthetic away text, then activate",
                                          "deckWasAway": away})
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
    stop_process(pid, executable, data)
    stop_socket(socket, bundle)
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


def cleanup():
    audit = {"processesRemaining": [], "socketsRemaining": [], "caffeinateReleased": None}
    for item in owned["processes"]:
        pid = item["pid"]
        stop_process(pid, Path(item["executable"]), Path(item["data"]))
        if our_pid(Path(item["executable"]), Path(item["data"])):
            audit["processesRemaining"].append(pid)
    for item in owned["sockets"]:
        bundle = Path(item["tmux"]).parent.parent.parent
        stop_socket(item["socket"], bundle)
        if any(f"-L {item['socket']} " in command + " " for _, command in processes()):
            audit["socketsRemaining"].append(item["socket"])
        inventory = subprocess.run([sys.executable, str(ROOT / "scripts/edr_runtime.py"), "--json", "--socket",
                                    item["socket"]], cwd=ROOT, capture_output=True, text=True, timeout=60)
        audit.setdefault("edrRuntimeInventoryExit", []).append(inventory.returncode)
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
        for layer, mode in (("l2", "translation"), ("l3", "translation-native")):
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
        "no-network": len(native_runs) == RUNS and all(not r["network"] for r in native_runs),
        "no-download": len(native_runs) == RUNS and all(not any(".staging" in e or "staging" in e for e in r.get("packParentEntries", []))
                                                          for r in native_runs),
        "screenshots": all(r["screenshots"] for r in runs) and len(runs) == 2 * RUNS,
        "isolation": report["isolationVerified"],
        "cleanup": None,
    }


def timings(runs):
    samples = {}
    for run in runs:
        for name, slot in run["checks"].items():
            if name.startswith("tl-t-") or name in ("tl-b07-requests", "tl-b07-dom", "tl-b02-final", "tl-n-b02-final",
                                                     "tl-n-c02-near-arm", "tl-b07-responsive"):
                samples.setdefault(name, []).extend(v[0] for v in slot["values"])
    report["timingSamples"] = {name: {"n": len(v), "raw": v, "min": min(v), "median": statistics.median(v), "max": max(v)}
                               for name, v in sorted(samples.items()) if v}


def main():
    save()
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
    if report["toolchain"]["inputSource"] not in ("com.apple.keylayout.ABC", "com.apple.keylayout.US"):
        fail("BLOCKED: native key input needs an ASCII keyboard layout (ABC/US) as the current input source")
        return 2
    cache, pack_id, assets = prepare_model()
    bundle = build_bundle()
    if source_digest() != report["testedSourceDigest"]:
        raise RuntimeError("sources changed during the build")
    report["environmentReady"] = True
    save()
    l1_results, rust_results = l1()
    report["tests"]["l1"] = l1_results
    report["tests"]["rust"] = rust_results
    save()
    baseline()
    owned["caffeinate"] = subprocess.Popen(["caffeinate", "-d", "-i", "-w", str(os.getpid())])
    report["ownedResources"].append({"kind": "caffeinate", "pid": owned["caffeinate"].pid})
    runs = []
    for index in range(1, RUNS + 1):
        for mode in RUN_MODES:
            if screen_locked():
                fail(f"BLOCKED: GUI session locked before run {index} {mode}")
                report["runs"] = runs
                return 2
            record = run_mode(index, mode, bundle, cache, pack_id, assets)
            runs.append(record)
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
    except Exception as error:  # the report is always written
        fail(f"{type(error).__name__}: {error}")
    finally:
        try:
            cleanup()
        except Exception as error:
            fail(f"cleanup: {type(error).__name__}: {error}")
        report.setdefault("driverChecks", {})
        report["driverChecks"]["cleanup"] = report["cleanupCompleted"]
        if "runs" in report and report["runs"]:
            evaluate(report["tests"].get("l1", {}), report["tests"].get("rust", {}), report["runs"])
        report["sharedClipboardHandling"] = {
            "isolation": "the general pasteboard is shared with the whole login session; it is NOT isolated by the separate bundle",
            "guard": "app-hosted (SmokeBridge.swift): items kept in Deck's memory only, never written to evidence; restore only when changeCount equals the last test-owned change",
            "results": [{"run": r["run"], "mode": r["mode"],
                         "guard": (r["checks"].get("tl-n-guard") or r["checks"].get("tl-e04-l2-guard") or {}).get("values"),
                         "restored": (r["checks"].get("tl-n-guard-restored") or r["checks"].get("tl-e04-l2-restored") or {}).get("values")}
                        for r in report.get("runs", [])],
            "writesByThisScript": "pbcopy of one synthetic sentence during C04 while the guard is active",
        }
        report["sourceDigestAfter"] = source_digest()
        report["testedTreeUnchanged"] = report["sourceDigestAfter"] == report["testedSourceDigest"]
        report["requiredTestsExecuted"] = (report["executedCount"] == report["requiredCount"]
                                           and not report["skippedRequired"])
        report["productAssertionsPassed"] = (report["passedCount"] == report["requiredCount"] and not report["failures"])
        passing = (report["environmentReady"] and report["isolationVerified"] and report["requiredTestsExecuted"]
                   and report["productAssertionsPassed"] and report["cleanupCompleted"]
                   and report["humanInterventions"] == 0 and report["testedTreeUnchanged"]
                   and report["tests"].get("l1NodeExit") == 0 and report["tests"].get("l1CargoExit") == 0
                   and report["tests"].get("uiGateExit") == 0)
        if code == 2 or not report["environmentReady"]:
            report["verdict"] = "BLOCKED"
        else:
            report["verdict"] = "PASS" if passing else "FAIL"
        report["finishedAt"] = datetime.datetime.now().isoformat(timespec="seconds")
        save()
        print(json.dumps({"verdict": report["verdict"], "report": str(REPORT_PATH),
                          "passed": report["passedCount"], "required": report["requiredCount"],
                          "failures": report["failures"][:10]}, indent=2, ensure_ascii=False))
    sys.exit({"PASS": 0, "FAIL": 1}.get(report["verdict"], 2))
