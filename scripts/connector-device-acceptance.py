#!/usr/bin/env python3
"""Physical-iPhone Connector acceptance against an isolated Deck host on the LAN.

Resumable subcommands share one experiment root (state.json inside it):

  prepare          root + registry, isolated host build/bundle, new and published
                   (e41bf49) iOS sources built as the side-by-side test variant
  host             launch the isolated host (connector-device carrier), register
                   it, enable the LAN listener and decode the displayed pairing QR
  agents           type into the isolated shells: a marker, then `claude` by hand
  test <name> [K=V ...]  run one ConnectorUITests method on the phone; DECKSTEP
                   markers in the live log trigger host steps (grant, fault, ...)
  step <word>      perform one host step now (snap-settings, grant, ...)
  tmux <session-title> <text>|--enter|--capture   isolated pane helpers
  cleanup          registry cleanup of host/tmux, uninstall only the test apps

The phone pairs only through the app's paste field with the code decoded from
the host's rendered QR image; no Keychain value, snapshot or capability is
injected. USB is used for XCTest control only; the app reaches the host over
the private Wi-Fi address. Test variant: io.c9r.deck.connector.qa ("Deck QA"),
its own application identifier, container and Keychain access group, so the
installed io.c9r.deck.connector app, its pairing and drafts are never touched.
Nothing here writes ~/.deck, the production tmux server or production settings.
Evidence written to the root is private (0700); copy only redacted summaries.
"""

import hashlib
import json
import os
import pathlib
import plistlib
import re
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.parse
import base64

ROOT = pathlib.Path(__file__).resolve().parent.parent
XCODE = pathlib.Path(os.environ.get("DECK_DEVICE_XCODE", "/Volumes/Yotta/Applications/Xcode.app"))
ENV = dict(os.environ, DEVELOPER_DIR=str(XCODE / "Contents/Developer"))
QA_BUNDLE = "io.c9r.deck.connector.qa"
RUNNER_BUNDLE = "io.c9r.deck.connector.uitests.xctrunner"
PUBLISHED_IOS = "e41bf49631d858b6838b63c2efb91bc2287ab3af"  # TestFlight 1.0 (3)
TITLES = {"agent": "qa manual agent", "shell": "qa shell", "stopped": "qa stopped", "saved": "qa saved claude"}
STATE_POINTER = pathlib.Path(tempfile.gettempdir()) / f"deck-device-acceptance-{os.getuid()}.root"


def sh(args, *, log=None, timeout=600, env=None, cwd=ROOT, check=True):
    with open(log, "a") if log else open(os.devnull, "w") as out:
        if log:
            out.write(f"\n$ {' '.join(map(str, args))[:300]}\n"); out.flush()
        p = subprocess.run(list(map(str, args)), cwd=cwd, env=env or ENV, stdout=out,
                           stderr=subprocess.STDOUT, timeout=timeout)
    if check and p.returncode:
        raise SystemExit(f"exit {p.returncode}: {args[0]} {args[1] if len(args) > 1 else ''} (log {log})")
    return p.returncode


def out(args, timeout=60, env=None):
    return subprocess.check_output(list(map(str, args)), cwd=ROOT, env=env or ENV, text=True,
                                   stderr=subprocess.DEVNULL, timeout=timeout).strip()


class State(dict):
    def __init__(self, root):
        self.root = pathlib.Path(root)
        path = self.root / "state.json"
        super().__init__(json.loads(path.read_text()) if path.exists() else {})

    def save(self):
        (self.root / "state.json").write_text(json.dumps(self, indent=2, sort_keys=True) + "\n")

    def event(self, name, **fields):
        self.setdefault("events", []).append({"t": round(time.time(), 3), "event": name, **fields})
        self.save()


def load():
    if not STATE_POINTER.exists():
        raise SystemExit("run `prepare` first")
    return State(STATE_POINTER.read_text().strip())


def registry(state, *args, check=True):
    return sh([sys.executable, ROOT / "scripts/edr_runtime.py", "experiment", *args, "--root", state.root],
              log=state.root / "registry.log", check=check)


def device_id():
    explicit = os.environ.get("DECK_DEVICE_ID")
    if explicit:
        return explicit
    listing = pathlib.Path(tempfile.mkstemp(prefix="dpa-devices-", suffix=".json")[1])
    try:
        out(["xcrun", "devicectl", "list", "devices", "--json-output", listing], 60)
        devices = json.loads(listing.read_text())["result"]["devices"]
    finally:
        listing.unlink(missing_ok=True)
    phones = [d for d in devices if d.get("hardwareProperties", {}).get("reality") == "physical"
              and d.get("hardwareProperties", {}).get("deviceType") == "iPhone"
              and d.get("connectionProperties", {}).get("tunnelState") == "connected"]
    if len(phones) != 1:
        raise SystemExit(f"expected exactly one connected physical iPhone, found {len(phones)}")
    return phones[0]["hardwareProperties"]["udid"]


def device_apps(state):
    path = state.root / "apps.json"
    out(["xcrun", "devicectl", "device", "info", "apps", "--device", state["device"], "--json-output", path], 120)
    return {a["bundleIdentifier"]: a.get("bundleVersion") for a in json.loads(path.read_text())["result"]["apps"]
            if "c9r" in a["bundleIdentifier"]}


def source_fingerprint():
    diff = subprocess.check_output(["git", "diff", "--binary", "HEAD", "--", "app/src-tauri", "app/ui", "connector/ios"],
                                   cwd=ROOT, timeout=60)
    digest = hashlib.sha256(diff)
    untracked = subprocess.check_output(["git", "ls-files", "--others", "--exclude-standard", "-z", "--",
                                         "app/src-tauri", "app/ui", "connector/ios"], cwd=ROOT, timeout=60)
    for rel in sorted(p for p in untracked.split(b"\0") if p):
        data = (ROOT / os.fsdecode(rel)).read_bytes()
        digest.update(rel + b"\0" + str(len(data)).encode() + b"\0" + data)
    return digest.hexdigest()


def cmd_prepare(_args):
    root = pathlib.Path(tempfile.mkdtemp(prefix="dpa-", dir="/tmp"))
    os.chmod(root, 0o700)
    (root / "t").mkdir(mode=0o700)
    STATE_POINTER.write_text(str(root) + "\n")
    os.chmod(STATE_POINTER, 0o600)
    state = State(root)
    state.update({"root": str(root), "head": out(["git", "rev-parse", "HEAD"]),
                  "gitStatusStart": out(["git", "status", "--short"]),
                  "sourceDiffSHA256": source_fingerprint(), "device": device_id(),
                  "nonce": hashlib.sha256(os.urandom(16)).hexdigest()[:10],
                  "socket": "deck-smoke-" + root.name.removeprefix("dpa-").replace("_", "").lower()[:8],
                  "xcode": out(["xcodebuild", "-version"]).replace("\n", " ")})
    state.save()
    registry(state, "init")
    apps = device_apps(state)
    state["appsBefore"] = apps
    if QA_BUNDLE in apps:
        raise SystemExit("a test variant is already installed; inspect it before continuing")
    state.save()

    build_host(state)
    build_ios(state)
    state.event("prepared")
    print(root)


def build_host(state):
    """Isolated host: debug build in the root, same bundle layout as the simulator driver."""
    root = state.root
    target = pathlib.Path(os.environ["DECK_DEVICE_REUSE_HOST_TARGET"]) if os.environ.get("DECK_DEVICE_REUSE_HOST_TARGET") else root / "rust-target"
    if not os.environ.get("DECK_DEVICE_REUSE_HOST_TARGET"):
        host_env = dict(ENV, CARGO_TARGET_DIR=str(target))
        sh(["cargo", "build", "--manifest-path", "app/src-tauri/Cargo.toml"], log=root / "host-build.log",
           timeout=3600, env=host_env)
    bundle = root / "deck-qa.app"
    macos = bundle / "Contents/MacOS"; macos.mkdir(parents=True); (bundle / "Contents/Resources").mkdir()
    shutil.copy2(target / "debug/deck-app", macos / "deck")
    for name in ("tmux", "deck-status-helper", "deck-lifecycle-helper", "deck-mcp", "deck-mcp-runner"):
        found = list((ROOT / "app/src-tauri/binaries").glob(name + "-aarch64-apple-darwin"))
        if found:
            shutil.copy2(found[0], macos / name)
    shutil.copy2(ROOT / "app/src-tauri/icons/icon.icns", bundle / "Contents/Resources/deck.icns")
    version = json.loads((ROOT / "app/src-tauri/tauri.conf.json").read_text())["version"]
    with open(bundle / "Contents/Info.plist", "wb") as f:
        plistlib.dump({"CFBundleExecutable": "deck", "CFBundleIdentifier": "io.c9r.deck.smoke.dpa" + state["socket"].removeprefix("deck-smoke-"),
                       "CFBundleName": "deck qa", "CFBundleVersion": version, "CFBundleShortVersionString": version,
                       "CFBundlePackageType": "APPL"}, f)
    sh([sys.executable, ROOT / "scripts/sidecar_provenance.py", "check", "--profile", "debug", "--bundle", bundle,
        "--main-product", target / "debug/deck-app"], log=root / "sidecar-provenance.log")
    sh(["codesign", "--force", "--sign", "-", "--entitlements", ROOT / "app/src-tauri/Entitlements.plist", bundle],
       log=root / "host-sign.log")
    state["hostBundle"] = str(bundle)
    state["hostBinarySHA256"] = hashlib.sha256((macos / "deck").read_bytes()).hexdigest()
    state.save()


def build_ios(state):
    build_new_ios(state)
    build_old_ios(state)


def build_new_ios(state):
    root = state.root
    # New iOS source as the test variant.
    derived = root / "ios-new"
    sh(["xcodebuild", "-project", ROOT / "connector/ios/DeckConnector.xcodeproj", "-scheme", "DeckConnectorUITests",
        "-configuration", "Debug", "-destination", f"id={state['device']}", "-derivedDataPath", derived,
        f"DECK_CONNECTOR_BUNDLE_ID={QA_BUNDLE}", "DECK_CONNECTOR_DISPLAY_NAME=Deck QA", "build-for-testing"],
       log=root / "ios-new-build.log", timeout=3600)
    app = derived / "Build/Products/Debug-iphoneos/DeckConnector.app"
    state["iosNewBinarySHA256"] = hashlib.sha256((app / "DeckConnector").read_bytes()).hexdigest()
    state["iosNewBuild"] = plistlib.load(open(app / "Info.plist", "rb"))["CFBundleVersion"]



def build_old_ios(state):
    root = state.root
    # Published TestFlight source (1.0 (3)), extracted (no worktree), same test identity.
    old = root / "ios-old-src"
    old.mkdir()
    archive = subprocess.run(["git", "archive", PUBLISHED_IOS, "connector/ios"], cwd=ROOT, stdout=subprocess.PIPE, check=True).stdout
    subprocess.run(["tar", "-x", "-C", old], input=archive, check=True)
    plist = old / "connector/ios/DeckConnectorApp/Info.plist"
    text = plist.read_text()
    text = text.replace("<string>io.c9r.deck.connector</string>", "<string>$(PRODUCT_BUNDLE_IDENTIFIER)</string>")
    text = text.replace("<key>CFBundleDisplayName</key><string>Deck Connector</string>", "<key>CFBundleDisplayName</key><string>Deck QA</string>")
    plist.write_text(text)
    sh(["xcodebuild", "-project", old / "connector/ios/DeckConnector.xcodeproj", "-scheme", "DeckConnector",
        "-configuration", "Debug", "-destination", f"id={state['device']}", "-derivedDataPath", root / "ios-old",
        f"PRODUCT_BUNDLE_IDENTIFIER={QA_BUNDLE}", "build"], log=root / "ios-old-build.log", timeout=3600)
    old_app = root / "ios-old/Build/Products/Debug-iphoneos/DeckConnector.app"
    state["iosOldApp"] = str(old_app)
    state["iosOldBuild"] = plistlib.load(open(old_app / "Info.plist", "rb"))["CFBundleVersion"]
    state["iosOldBinarySHA256"] = hashlib.sha256((old_app / "DeckConnector").read_bytes()).hexdigest()
    state.save()


def cmd_rebuild_ios(_args):
    state = load()
    shutil.move(str(state.root / "ios-new"), str(state.root / f"ios-new-attempt{int(time.time())}"))
    state["sourceDiffSHA256"] = source_fingerprint()
    build_new_ios(state)
    state.event("ios-rebuilt")


def cmd_rebuild_host(_args):
    """Rebuild only the host after a carrier fix; the failed attempt's data stays as evidence."""
    state = load()
    attempt = len(list(state.root.glob("host-data-attempt*"))) + 1
    if data_dir(state).exists():
        data_dir(state).rename(state.root / f"host-data-attempt{attempt}")
    shutil.rmtree(state["hostBundle"])
    for key in ("hostPID", "serverRegistered", "tmuxSocket", "stepSeq", "pairing"):
        state.pop(key, None)
    state["socket"] = "deck-smoke-" + state.root.name.removeprefix("dpa-").replace("_", "").lower()[:8]
    state["sourceDiffSHA256Host"] = source_fingerprint()
    os.environ["DECK_DEVICE_REUSE_HOST_TARGET"] = ""
    build_host(state)
    state.event("host-rebuilt", attempt=attempt)


def data_dir(state):
    return state.root / "host-data"


def app_log(state):
    path = data_dir(state) / "app.log"
    return path.read_text(errors="replace") if path.exists() else ""


def wait_log(state, pattern, timeout):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        found = re.findall(pattern, app_log(state))
        if found:
            return found
        time.sleep(0.2)
    return None


def scenario(state, word):
    path = data_dir(state) / "translation-fixture" / "scenario"
    path.parent.mkdir(mode=0o700, exist_ok=True)
    path.write_text(word + "\n")
    os.chmod(path, 0o600)


def host_step(state, step, timeout=60):
    seq = state.get("stepSeq", 0) + 1
    state["stepSeq"] = seq; state.save()
    started = time.monotonic()
    scenario(state, f"{seq}:{step}")
    found = wait_log(state, rf"smoke-check connector-device-step a=(-?{seq}) b=(\d+)", timeout)
    ok = bool(found) and int(found[-1][0]) > 0
    state.event("host-step", step=step, seq=seq, ok=ok, seconds=round(time.monotonic() - started, 2))
    if not ok:
        raise SystemExit(f"host step {step} failed")
    return seq


def register_host(state):
    executable = pathlib.Path(state["hostBundle"]) / "Contents/MacOS/deck"
    deadline = time.monotonic() + 60
    while time.monotonic() < deadline:
        rows = out(["ps", "-axo", "pid=,comm="], 10).splitlines()
        pids = [int(r.split(None, 1)[0]) for r in rows if len(r.split(None, 1)) == 2
                and os.path.realpath(r.split(None, 1)[1]) == os.path.realpath(executable)]
        if len(pids) == 1:
            if state.get("hostPID") != pids[0]:
                registry(state, "add-process", "--pid", pids[0])
                state["hostPID"] = pids[0]; state.save()
            return
        time.sleep(0.25)
    raise SystemExit("isolated host process not found")


def register_server(state):
    socket = state.root / "t" / f"tmux-{os.getuid()}" / state["socket"]
    deadline = time.monotonic() + 30
    while not socket.exists() and time.monotonic() < deadline:
        time.sleep(0.25)
    if not socket.exists():
        raise SystemExit("isolated tmux socket not created")
    if not state.get("serverRegistered"):
        registry(state, "add-server", "--socket", socket, "--tmux", pathlib.Path(state["hostBundle"]) / "Contents/MacOS/tmux")
        state["serverRegistered"] = True; state["tmuxSocket"] = str(socket); state.save()


QR_DECODER = r'''
import AppKit
import Vision
let url = URL(fileURLWithPath: CommandLine.arguments[1])
guard let ns = NSImage(contentsOf: url), var cg = ns.cgImage(forProposedRect: nil, context: nil, hints: nil) else { exit(2) }
func decode(_ image: CGImage) -> [String] {
    let request = VNDetectBarcodesRequest(); request.symbologies = [.qr]
    try? VNImageRequestHandler(cgImage: image).perform([request])
    return (request.results ?? []).compactMap { $0.payloadStringValue }
}
var codes = Set(decode(cg))
if codes.count != 1 {
    // Crop the largest bright square region (the QR card) and retry at 2x.
    let w = cg.width, h = cg.height
    if let crop = cg.cropping(to: CGRect(x: w/3, y: h/4, width: w/3, height: h*2/3)) { cg = crop; codes = Set(decode(cg)) }
}
guard codes.count == 1 else { FileHandle.standardError.write("found \(codes.count)\n".data(using: .utf8)!); exit(3) }
print(codes.first!)
'''


def cmd_host(_args):
    state = load()
    address = os.environ.get("DECK_DEVICE_HOST_ADDRESS") or out(["ipconfig", "getifaddr", "en0"])
    port = int(os.environ.get("DECK_DEVICE_HOST_PORT", "47731"))
    data = data_dir(state); data.mkdir(mode=0o700, exist_ok=True)
    env = dict(ENV, TMUX_TMPDIR=str(state.root / "t"))
    sh(["open", "-g", "-n", state["hostBundle"], "--env", f"TMUX_TMPDIR={state.root / 't'}", "--args",
        "--smoke-data-dir", data, "--smoke-tmux-socket", state["socket"], "--smoke-wkwebview", "connector-device"],
       log=state.root / "host-open.log", env=env, timeout=60)
    register_host(state)
    scenario(state, f"0:setup-{address.replace('.', '-')}-{port}")
    found = wait_log(state, r"smoke-check connector-device-ready a=(-?\d+) b=(\d+)", 120)
    register_server(state)
    if not found or int(found[-1][0]) < 0:
        raise SystemExit(f"host setup failed: {found}")
    listeners = out(["lsof", "-nP", "-a", "-p", state["hostPID"], "-iTCP", "-sTCP:LISTEN"], 10)
    if f"{address}:{port}" not in listeners:
        raise SystemExit("listener does not belong to the isolated host")
    decode_pairing(state, address, port)


def decode_pairing(state, address, port):
    data = data_dir(state)
    decoder = state.root / "qr.swift"; decoder.write_text(QR_DECODER)
    uri = out(["xcrun", "swift", decoder, data / "evidence/pairing-qr.png"], 180)
    parsed = urllib.parse.urlparse(uri)
    encoded = urllib.parse.parse_qs(parsed.query)["data"][0]
    descriptor = json.loads(base64.urlsafe_b64decode(encoded + "=" * (-len(encoded) % 4)))
    origin = urllib.parse.urlparse(descriptor["origin"])
    if parsed.scheme != "deck-connector" or origin.scheme != "https" or origin.hostname != address or origin.port != port:
        raise SystemExit("decoded pairing origin does not match the isolated listener")
    # SAN/pin cross-check from the Mac side: the served leaf hashes to the descriptor pin.
    leaf = subprocess.run(["openssl", "s_client", "-connect", f"{address}:{port}", "-servername", address, "-showcerts"],
                          input=b"", capture_output=True, timeout=20).stdout
    pem = re.search(rb"-----BEGIN CERTIFICATE-----.+?-----END CERTIFICATE-----", leaf, re.S).group(0)
    der = subprocess.run(["openssl", "x509", "-outform", "DER"], input=pem, capture_output=True, check=True).stdout
    text = subprocess.run(["openssl", "x509", "-noout", "-ext", "subjectAltName"], input=pem, capture_output=True).stdout.decode()
    state["tls"] = {"pinMatchesDescriptor": hashlib.sha256(der).hexdigest() == descriptor["fingerprint"].lower(),
                    "sanContainsListenerAddress": f"IP Address:{address}" in text}
    state["pairing"] = {"uri": uri, "host": descriptor["hostName"], "hostIdHash": hashlib.sha256(descriptor["hostId"].encode()).hexdigest()[:16],
                        "originIsPrivateLan": True, "port": port}
    state["hostAddress"] = address
    state["pairingDecodedAt"] = time.time()
    state.event("pairing-decoded", tls=state["tls"])
    print(json.dumps(state["tls"]))


def cmd_pair_refresh(_args):
    state = load()
    host_step(state, "pair")
    decode_pairing(state, state.get("hostAddress") or out(["ipconfig", "getifaddr", "en0"]),
                   int(os.environ.get("DECK_DEVICE_HOST_PORT", "47731")))


def tmux(state, *args, timeout=15):
    return out([pathlib.Path(state["hostBundle"]) / "Contents/MacOS/tmux", "-S", state["tmuxSocket"], *args], timeout)


def session_of(state, title):
    board = json.loads((data_dir(state) / "deck.json").read_text())
    cards = board.get("data", board).get("cards", [])
    match = [c for c in cards if c.get("title") == title]
    if len(match) != 1:
        raise SystemExit(f"card {title!r}: {len(match)} matches")
    return match[0]["session"]


def capture(state, title):
    return tmux(state, "capture-pane", "-p", "-J", "-t", session_of(state, title), "-S", "-200")


def type_line(state, title, text, enter=True):
    target = session_of(state, title)
    tmux(state, "send-keys", "-t", target, "-l", text)
    if enter:
        tmux(state, "send-keys", "-t", target, "Enter")


def cmd_agents(_args):
    state = load()
    n = state["nonce"]
    type_line(state, TITLES["shell"], f"echo DQA-SHELL-{n}")
    work = state.root / "agent-work"; work.mkdir(mode=0o700, exist_ok=True)
    # A user starting Claude by hand in an empty-command card's shell.
    type_line(state, TITLES["agent"], f"cd {work} && echo DQA-AGENT-{n} && claude")
    deadline = time.monotonic() + 90
    trusted = False
    while time.monotonic() < deadline:
        pane = capture(state, TITLES["agent"])
        if not trusted and re.search(r"trust (this|the files in this) folder", pane, re.I):
            tmux(state, "send-keys", "-t", session_of(state, TITLES["agent"]), "Enter")
            trusted = True
            state.event("agent-trust-answered")
        elif re.search(r"\? for shortcuts|bypass permissions|accept edits|Try \"", pane):
            # Claude clears the screen on start; its header shows the run's own work directory.
            state["agentMarker"] = f"{state.root.name}/agent-work"
            state["shellMarker"] = f"DQA-SHELL-{n}"
            state.event("agent-ready", trusted=trusted)
            print("agent ready")
            return
        time.sleep(1)
    (state.root / "agent-pane-timeout.txt").write_text(capture(state, TITLES["agent"]))
    raise SystemExit("Claude did not reach its prompt; pane saved")


# The phone's own device is trusted / no longer trusted through the Mac's
# Settings (per-device trust); the global scope stays "All card summaries".
HOST_REACTIONS = {"ready-trust": "trust", "ready-grant": "grant", "ready-fault": "fault",
                  "ready-downgrade": "untrust", "ready-regrant": "trust", "ready-disable": "disable",
                  "ready-enable": "enable", "ready-revoke": "revoke"}


def plan_for(state, test, extra):
    products = state.root / "ios-new/Build/Products"
    source = [p for p in products.glob("DeckConnectorUITests_*.xctestrun")]
    if len(source) != 1:
        raise SystemExit("expected one generated UI test plan")
    plan = plistlib.load(open(source[0], "rb"))
    target = plan["DeckConnectorUITests"]
    env = target.setdefault("EnvironmentVariables", {})
    n = state["nonce"]
    env.update({"DECK_DEVICE_ACCEPTANCE": "1", "DECK_DEVICE_HOST": state.get("pairing", {}).get("host", ""),
                "DECK_DEVICE_AGENT_TITLE": TITLES["agent"], "DECK_DEVICE_SHELL_TITLE": TITLES["shell"],
                "DECK_DEVICE_STOPPED_TITLE": TITLES["stopped"], "DECK_DEVICE_NONCE": n,
                "DECK_DEVICE_AGENT_MARKER": state.get("agentMarker", ""), "DECK_DEVICE_SHELL_MARKER": state.get("shellMarker", ""),
                "DECK_DEVICE_EXIT_MARKER": f"DQA-EXIT-{n}",
                "DECK_DEVICE_ADDRESS": f"{state.get('hostAddress', '')}:{state.get('pairing', {}).get('port', '')}"})
    if test == "testDeviceR1OldBuildReproducesFieldDenial":
        env["DECK_DEVICE_PAIRING_URI"] = state["pairing"]["uri"]
        target["UITargetAppPath"] = state["iosOldApp"]
        target["DependentProductPaths"] = [p for p in target["DependentProductPaths"] if not p.endswith("/DeckConnector.app")] + [state["iosOldApp"]]
    for item in extra:
        key, value = item.split("=", 1)
        env[key] = value
    path = products / f"DeckConnectorUITests-device-{int(time.time())}.xctestrun"
    plistlib.dump(plan, open(path, "wb"))
    os.chmod(path, 0o600)
    return path


def cmd_test(args):
    state = load()
    test, extra = args[0], args[1:]
    plan = plan_for(state, test, extra)
    runs = state.root / "runs"; runs.mkdir(mode=0o700, exist_ok=True)
    stamp = time.strftime("%H%M%S")
    log = runs / f"{test}-{stamp}.log"; result = runs / f"{test}-{stamp}.xcresult"
    started = time.monotonic()
    with open(log, "w") as output:
        proc = subprocess.Popen(["xcodebuild", "-xctestrun", plan, "-destination", f"id={state['device']}",
                                 f"-only-testing:DeckConnectorUITests/ConnectorUITests/{test}",
                                 "-resultBundlePath", result, "test-without-building"],
                                cwd=state.root, env=ENV, stdout=output, stderr=subprocess.STDOUT)
    seen = set()
    while proc.poll() is None:
        for step in re.findall(r"DECKSTEP (\S+)", log.read_text(errors="replace")):
            if step in seen:
                continue
            seen.add(step)
            state.event("phone-step", test=test, step=step)
            if step in HOST_REACTIONS:
                host_step(state, HOST_REACTIONS[step])
            if step == "locked":
                time.sleep(3)
                lock = subprocess.run(["xcrun", "devicectl", "device", "info", "lockState", "--device", state["device"]],
                                      env=ENV, capture_output=True, text=True, timeout=60).stdout
                state.event("device-lock-state", passcodeRequired="passcodeRequired: true" in lock,
                            unlockedSinceBoot="unlockedSinceBoot: true" in lock)
        time.sleep(0.3)
    plan.unlink()  # the copied plan may hold the one-use pairing URI
    text = log.read_text(errors="replace")
    executed = re.findall(r"Executed (\d+) tests?, with (\d+) failures?", text)
    skipped = len(re.findall(r"' skipped \(", text))
    record = {"test": test, "exit": proc.returncode, "seconds": round(time.monotonic() - started, 1),
              "executed": int(executed[-1][0]) if executed else 0, "failures": int(executed[-1][1]) if executed else None,
              "skipped": skipped, "log": str(log), "xcresult": str(result), "steps": sorted(seen)}
    record["verdict"] = "PASS" if proc.returncode == 0 and record["executed"] == 1 and record["failures"] == 0 and skipped == 0 else "FAIL"
    state.setdefault("tests", []).append(record); state.save()
    attachments = runs / f"{test}-{stamp}-att"
    sh(["xcrun", "xcresulttool", "export", "attachments", "--path", result, "--output-path", attachments],
       log=runs / "export.log", check=False)
    print(json.dumps(record, indent=1))
    for line in re.findall(r".*(?:error:|XCTAssert|failed).*", text)[:12]:
        print(line)


def cmd_step(args):
    print(host_step(load(), args[0]))


def cmd_tmux(args):
    state = load()
    title, rest = TITLES.get(args[0], args[0]), args[1:]
    if rest == ["--capture"]:
        print(capture(state, title))
    elif rest == ["--enter"]:
        tmux(state, "send-keys", "-t", session_of(state, title), "Enter")
    else:
        type_line(state, title, " ".join(rest))


def cmd_cleanup(_args):
    state = load()
    if data_dir(state).joinpath("app.log").exists():
        scenario(state, f"{state.get('stepSeq', 0) + 1}:done")
    registry(state, "cleanup", check=False)
    registry(state, "show", check=False)
    # iOS keeps Keychain items across uninstall: remove the variant's pairing
    # through its own UI first. If that fails, keep the app installed so the
    # credential can still be removed, and say so.
    # Not gated on an app listing (one listing once omitted the installed
    # variant): any device test in this root may have left a pairing, and
    # running Z1 reinstalls the variant if needed to reach that Keychain item.
    if state.get("tests"):
        cmd_test(["testDeviceZ1RemovePairing"])
        state = load()
        if state["tests"][-1]["verdict"] != "PASS":
            state.event("cleanup-kept-test-app", reason="pairing removal failed")
            raise SystemExit("test variant pairing not removed; app kept for manual removal")
    for bundle in (QA_BUNDLE, RUNNER_BUNDLE):
        if bundle not in state.get("appsBefore", {}):
            sh(["xcrun", "devicectl", "device", "uninstall", "app", "--device", state["device"], bundle],
               log=state.root / "uninstall.log", check=False, timeout=120)
    state["appsAfter"] = device_apps(state)
    state["gitStatusEnd"] = out(["git", "status", "--short"])
    state.event("cleanup")
    print(json.dumps({"appsBefore": state["appsBefore"], "appsAfter": state["appsAfter"]}))


if __name__ == "__main__":
    commands = {"prepare": cmd_prepare, "rebuild-host": cmd_rebuild_host, "rebuild-ios": cmd_rebuild_ios, "pair-refresh": cmd_pair_refresh, "host": cmd_host, "agents": cmd_agents, "test": cmd_test,
                "step": cmd_step, "tmux": cmd_tmux, "cleanup": cmd_cleanup}
    if len(sys.argv) < 2 or sys.argv[1] not in commands:
        raise SystemExit(__doc__)
    commands[sys.argv[1]](sys.argv[2:])
