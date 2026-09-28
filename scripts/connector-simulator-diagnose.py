#!/usr/bin/env python3
"""Run isolated Connector regressions on disposable iPhone simulators."""

import base64
import hashlib
import json
import os
import pathlib
import plistlib
import re
import shutil
import signal
import socket
import stat
import subprocess
import sys
import tempfile
import time
import urllib.parse

ROOT = pathlib.Path(__file__).resolve().parent.parent
XCODE = pathlib.Path(os.environ.get("DECK_DIAG_XCODE", "/Volumes/Yotta/Applications/Xcode.app"))
DEVELOPER = XCODE / "Contents/Developer"
ENV = dict(os.environ, DEVELOPER_DIR=str(DEVELOPER))
SUITE = os.environ.get("DECK_DIAG_SUITE", "all")
if SUITE not in ("all", "ui", "appmodel"):
    raise SystemExit("DECK_DIAG_SUITE must be all, ui or appmodel")
RUN = pathlib.Path(tempfile.mkdtemp(prefix="deck-connector-diagnose-", dir="/tmp"))
os.chmod(RUN, 0o700)
DATA = RUN / "host-data"
SOCKET = "deck-smoke-" + RUN.name.removeprefix("deck-connector-diagnose-").replace("_", "")
MANIFEST = RUN / "manifest.json"
REPORT = RUN / "report.json"
state = {"runDir": str(RUN), "developerDir": str(DEVELOPER), "simulatorUDID": None,
         "hostPID": None, "hostBundle": None, "tmuxSocket": SOCKET, "cleanupCompleted": False}
result = {"environmentReady": False, "isolationVerified": False,
          "mode": "fixed", "reproductionConfirmed": None,
          "historicalReproductionReport": "/tmp/deck-connector-diagnose-owuvifyn/report.json",
          "productAssertionsPassed": False,
          "requiredTestsExecuted": False, "cleanupCompleted": False, "humanInterventions": 0,
          "xcodePath": str(XCODE), "developerDir": str(DEVELOPER),
          "tests": {},
          "cases": {name: "UNRUN" for name in (
              "liveSavedAgentOutput", "ordinaryShellVisibility", "shellWithForegroundAgent",
              "savedCommandVariants", "stoppedSavedAgentOutput", "agentExitedToShell",
              "foregroundCycle", "appRelaunchCredentialRecovery", "refreshFailureRecovery",
              "saveFailure", "backupRecovery")}}


def save():
    MANIFEST.write_text(json.dumps(state, indent=2) + "\n")
    REPORT.write_text(json.dumps(result, indent=2) + "\n")


def run(args, *, log=None, timeout=300, env=ENV):
    with open(log, "w") if log else open(os.devnull, "w") as output:
        p = subprocess.run(args, cwd=ROOT, env=env, stdout=output, stderr=subprocess.STDOUT,
                           timeout=timeout, check=False)
    if p.returncode:
        raise RuntimeError(f"exit {p.returncode}: {args[0]} (log: {log})")


def capture(args, timeout=30):
    return subprocess.check_output(args, cwd=ROOT, env=ENV, text=True,
                                   stderr=subprocess.DEVNULL, timeout=timeout).strip()


def process_matches(pid, executable):
    try:
        command = capture(["ps", "-p", str(pid), "-o", "command="], 5)
        return os.path.realpath(command.split(" ", 1)[0]) == os.path.realpath(executable)
    except (subprocess.CalledProcessError, subprocess.TimeoutExpired):
        return False


def source_fingerprint():
    diff = subprocess.check_output(["git", "diff", "--binary", "HEAD", "--",
                                    "app/src-tauri", "app/ui", "connector/ios"], cwd=ROOT,
                                   env=ENV, timeout=30)
    return hashlib.sha256(diff).hexdigest()


def probe_live_agent(tmux):
    board = DATA / "deck.json"
    deadline = time.monotonic() + 45
    live = None
    while time.monotonic() < deadline:
        if board.is_file():
            cards = json.loads(board.read_text()).get("data", {}).get("cards", [])
            live = next((card for card in cards if card.get("title") == "connector live codex"), None)
            if live:
                break
        time.sleep(0.25)
    if not live:
        result["realAgentProbe"] = "BLOCKED: isolated live card was not created"
        return
    session = live["session"]
    state["liveSession"] = session
    result["liveCardIdHash"] = hashlib.sha256(live["id"].encode()).hexdigest()[:16]
    agent_ready = False
    deadline = time.monotonic() + 45
    while time.monotonic() < deadline:
        try:
            command = capture([str(tmux), "-L", SOCKET, "display-message", "-p", "-t", session,
                               "#{pane_current_command}"], 5)
            if command == "codex":
                agent_ready = True
                break
        except (subprocess.CalledProcessError, subprocess.TimeoutExpired):
            pass
        time.sleep(0.25)
    if not agent_ready:
        result["realAgentProbe"] = "BLOCKED: real Codex did not become the foreground command"
        return
    choices = []
    stable_ready = 0
    deadline = time.monotonic() + 45
    while time.monotonic() < deadline:
        screen = capture([str(tmux), "-L", SOCKET, "capture-pane", "-p", "-t", session], 5)
        if "› 1. Update now" in screen and "Skip" in screen:
            run([str(tmux), "-L", SOCKET, "send-keys", "-t", session, "Escape"], timeout=10)
            choices.append("skip-update")
            stable_ready = 0
        elif "Trust this folder?" in screen:
            agent_dir = DATA / "agent-work"
            if not agent_dir.is_dir() or list(agent_dir.iterdir()):
                result["realAgentProbe"] = "BLOCKED: Agent trust prompt directory is not empty"
                return
            run([str(tmux), "-L", SOCKET, "send-keys", "-t", session, "Enter"], timeout=10)
            choices.append("trust-empty-test-directory")
            stable_ready = 0
        elif "OpenAI Codex" in screen and "› " in screen:
            stable_ready += 1
            if stable_ready >= 5:
                break
        else:
            stable_ready = 0
        time.sleep(0.25)
    else:
        result["realAgentProbe"] = "BLOCKED: real Codex did not reach its input prompt"
        return
    result["agentStartupChoices"] = choices
    marker = "deckprobe" + RUN.name.removeprefix("deck-connector-diagnose-").replace("_", "")
    prompt = ("Reply with only the concatenation of these two chunks, without spaces: "
              f"deckprobe and {marker.removeprefix('deckprobe')}.")
    run([str(tmux), "-L", SOCKET, "send-keys", "-l", "-t", session, prompt], timeout=10)
    run([str(tmux), "-L", SOCKET, "send-keys", "-t", session, "Enter"], timeout=10)
    deadline = time.monotonic() + 90
    submit_again = False
    pending_since = time.monotonic()
    while time.monotonic() < deadline:
        try:
            output = capture([str(tmux), "-L", SOCKET, "capture-pane", "-p", "-t", session,
                              "-S", "-200"], 5)
            if marker in output:
                result["realAgentProbe"] = "PASS"
                result["realAgentMarker"] = marker
                return
            if "› 1. Update now" in output and "Skip" in output:
                run([str(tmux), "-L", SOCKET, "send-keys", "-t", session, "Escape"], timeout=10)
                pending_since = time.monotonic()
            elif "Trust this folder?" in output:
                agent_dir = DATA / "agent-work"
                if not agent_dir.is_dir() or list(agent_dir.iterdir()):
                    result["realAgentProbe"] = "BLOCKED: Agent trust prompt directory is not empty"
                    return
                run([str(tmux), "-L", SOCKET, "send-keys", "-t", session, "Enter"], timeout=10)
                pending_since = time.monotonic()
            elif (not submit_again and "› Reply with only the concatenation" in output
                  and "GPT-6-Sol" in output and time.monotonic() - pending_since > 2):
                # The exact prompt is still in Codex's composer after its startup dialog.
                run([str(tmux), "-L", SOCKET, "send-keys", "-t", session, "Enter"], timeout=10)
                submit_again = True
                result["agentComposerSubmit"] = "visible-pending-input"
        except (subprocess.CalledProcessError, subprocess.TimeoutExpired):
            pass
        time.sleep(0.5)
    result["realAgentProbe"] = "BLOCKED: Codex did not return the nonsecret marker within 90 seconds"


def probe_agent_exit_to_shell(tmux):
    if result.get("realAgentProbe") != "PASS" or not state.get("liveSession"):
        result["shellReturnProbe"] = "BLOCKED: live Agent output did not pass"
        return False
    session = state["liveSession"]
    run([str(tmux), "-L", SOCKET, "send-keys", "-l", "-t", session, "/exit"], timeout=10)
    run([str(tmux), "-L", SOCKET, "send-keys", "-t", session, "Enter"], timeout=10)
    sent_extra_enter = False
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        try:
            command = capture([str(tmux), "-L", SOCKET, "display-message", "-p", "-t", session,
                               "#{pane_current_command}"], 5)
            if command in ("zsh", "bash", "fish", "sh"):
                break
            screen = capture([str(tmux), "-L", SOCKET, "capture-pane", "-p", "-t", session], 5)
            if not sent_extra_enter and "› /exit" in screen:
                run([str(tmux), "-L", SOCKET, "send-keys", "-t", session, "Enter"], timeout=10)
                sent_extra_enter = True
        except (subprocess.CalledProcessError, subprocess.TimeoutExpired):
            result["shellReturnProbe"] = "BLOCKED: test-owned session disappeared"
            return False
        time.sleep(0.25)
    else:
        result["shellReturnProbe"] = "BLOCKED: Codex did not return to a live shell"
        return False
    suffix = RUN.name.removeprefix("deck-connector-diagnose-").replace("_", "")
    marker = "shellmarker" + suffix
    command = "printf '%s%s\\n' 'shellmarker' '" + suffix + "'"
    run([str(tmux), "-L", SOCKET, "send-keys", "-l", "-t", session, command], timeout=10)
    run([str(tmux), "-L", SOCKET, "send-keys", "-t", session, "Enter"], timeout=10)
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        output = capture([str(tmux), "-L", SOCKET, "capture-pane", "-p", "-t", session,
                          "-S", "-200"], 5)
        if marker in output:
            result["shellReturnProbe"] = "PASS"
            result["shellReturnMarker"] = marker
            return True
        time.sleep(0.25)
    result["shellReturnProbe"] = "BLOCKED: shell marker was not produced"
    return False


def main():
    save()
    if not DEVELOPER.is_dir():
        raise RuntimeError("specified Xcode is unavailable")
    result["xcodeVersion"] = capture(["xcodebuild", "-version"])
    result["simctl"] = capture(["xcrun", "--find", "simctl"])
    result["sdks"] = capture(["xcodebuild", "-showsdks"])
    result["projectSchemes"] = capture(["xcodebuild", "-list", "-project",
                                        str(ROOT / "connector/ios/DeckConnector.xcodeproj")])
    runtimes = json.loads(capture(["xcrun", "simctl", "list", "runtimes", "-j"]))
    device_types_inventory = json.loads(capture(["xcrun", "simctl", "list", "devicetypes", "-j"]))
    devices_inventory = json.loads(capture(["xcrun", "simctl", "list", "devices", "available", "-j"]))
    result["installedDeviceTypeCount"] = len(device_types_inventory.get("devicetypes", []))
    result["availableDeviceCount"] = sum(len(value) for value in devices_inventory.get("devices", {}).values())
    available = [x for x in runtimes["runtimes"] if x.get("isAvailable") and
                 x["identifier"].startswith("com.apple.CoreSimulator.SimRuntime.iOS-")]
    if not available:
        raise RuntimeError("iOS runtime unavailable")
    runtime = max(available, key=lambda x: tuple(int(v) for v in x["version"].split(".")))
    device_types = runtime.get("supportedDeviceTypes", [])
    iphones = [x for x in device_types if x.get("productFamily") == "iPhone"]
    if not iphones:
        raise RuntimeError("iPhone device type unavailable")
    device_type = next((x["identifier"] for x in iphones if x["name"] == "iPhone 17"),
                       iphones[0]["identifier"])
    result["selectedRuntime"] = runtime["identifier"]
    result["selectedDeviceType"] = device_type
    result["head"] = capture(["git", "rev-parse", "HEAD"])
    result["initialGitStatus"] = capture(["git", "status", "--short"])
    result["sourceDiffSHA256"] = source_fingerprint()
    result["freeBytes"] = shutil.disk_usage(RUN).free
    result["environmentReady"] = True
    # This test uses the existing debug smoke path, whose credential store is process local.
    keychain = (ROOT / "app/src-tauri/src/keychain.rs").read_text()
    if "if crate::smoke_faults::enabled()" not in keychain or "cache_put(slot, Some(value.to_string()))" not in keychain:
        raise RuntimeError("smoke Keychain isolation proof changed")
    DATA.mkdir(mode=0o700)
    state["hostData"] = str(DATA)
    state["simulatorUDID"] = capture(["xcrun", "simctl", "create", RUN.name, device_type, runtime["identifier"]])
    save()
    run(["xcrun", "simctl", "boot", state["simulatorUDID"]], log=RUN / "sim-boot.log", timeout=60)
    run(["xcrun", "simctl", "bootstatus", state["simulatorUDID"], "-b"], log=RUN / "sim-ready.log", timeout=180)
    state["simulatorRuntime"] = runtime["identifier"]
    save()

    reused = os.environ.get("DECK_DIAG_REUSE_HOST_TARGET")
    target = pathlib.Path(reused) if reused else RUN / "rust-target"
    if reused:
        if not target.is_dir() or "deck-connector-diagnose" not in str(target.resolve()):
            raise RuntimeError("reuse target must be an existing isolated diagnostic build")
        result["hostBuildReusedFrom"] = str(target)
    else:
        host_env = dict(ENV, CARGO_TARGET_DIR=str(target))
        run(["cargo", "build", "--manifest-path", "app/src-tauri/Cargo.toml"],
            log=RUN / "host-build.log", timeout=1800, env=host_env)
    bundle = RUN / "deck-smoke.app"
    macos = bundle / "Contents/MacOS"
    resources = bundle / "Contents/Resources"
    macos.mkdir(parents=True)
    resources.mkdir()
    if not os.access(target / "debug/deck-app", os.X_OK):
        raise RuntimeError("isolated host binary is not executable")
    shutil.copy2(target / "debug/deck-app", macos / "deck")
    result["hostBinarySHA256"] = hashlib.sha256((macos / "deck").read_bytes()).hexdigest()
    binaries = ROOT / "app/src-tauri/binaries"
    for name in ("tmux", "deck-status-helper", "deck-mcp", "deck-mcp-runner"):
        matches = list(binaries.glob(name + "-aarch64-apple-darwin"))
        if matches:
            shutil.copy2(matches[0], macos / name)
    shutil.copy2(ROOT / "app/src-tauri/icons/icon.icns", resources / "deck.icns")
    version = json.loads((ROOT / "app/src-tauri/tauri.conf.json").read_text())["version"]
    with open(bundle / "Contents/Info.plist", "wb") as file:
        plistlib.dump({"CFBundleExecutable": "deck",
                       "CFBundleIdentifier": "io.c9r.deck.smoke." + RUN.name.rsplit("-", 1)[-1].replace("_", ""),
                       "CFBundleName": "deck smoke", "CFBundleVersion": version,
                       "CFBundleShortVersionString": version, "CFBundlePackageType": "APPL"}, file)
    run(["codesign", "--force", "--sign", "-", "--entitlements",
         str(ROOT / "app/src-tauri/Entitlements.plist"), str(bundle)],
        log=RUN / "host-sign.log", timeout=60)
    executable = macos / "deck"
    state["hostBundle"] = str(bundle)
    save()
    udid = state["simulatorUDID"]
    derived = RUN / "DerivedData"
    project = str(ROOT / "connector/ios/DeckConnector.xcodeproj")
    destination = f"platform=iOS Simulator,id={udid}"
    scheme = "DeckConnectorUITests" if SUITE == "ui" else "DeckConnector"
    run(["xcodebuild", "-project", project, "-scheme", scheme,
         "-configuration", "Debug", "-destination", destination,
         "-derivedDataPath", str(derived), "build-for-testing"],
        log=RUN / "ios-build.log", timeout=1200)
    products = derived / "Build/Products"
    app = products / "Debug-iphonesimulator/DeckConnector.app"
    result["iosBinarySHA256"] = hashlib.sha256((app / "DeckConnector").read_bytes()).hexdigest()
    if source_fingerprint() != result["sourceDiffSHA256"]:
        raise RuntimeError("build inputs changed during this diagnostic run")
    run(["xcrun", "simctl", "install", udid, str(app)], log=RUN / "sim-install.log", timeout=90)
    open_args = ["open", "-n", str(bundle), "--args", "--smoke-data-dir", str(DATA),
                 "--smoke-tmux-socket", SOCKET, "--smoke-wkwebview", "connector-transport"]
    for attempt in (1, 2):
        open_log = RUN / f"host-open-{attempt}.log"
        try:
            run(open_args, log=open_log, timeout=30)
            break
        except RuntimeError:
            failure = open_log.read_text()
            known_transient = ("RBSRequestErrorDomain Code=5" in failure
                               and "Launchd job spawn failed" in failure)
            active = any(os.path.realpath(line.strip().split(" ", 1)[1].split(" ", 1)[0])
                         == os.path.realpath(executable)
                         for line in capture(["ps", "-axo", "pid=,command="], 10).splitlines()
                         if len(line.strip().split(" ", 1)) == 2)
            if attempt == 2 or not known_transient or active:
                raise
            result["hostLaunchTransientRetries"] = attempt
    fixture = DATA / "connector-smoke-transport.json"
    deadline = time.monotonic() + 90
    while time.monotonic() < deadline and not fixture.is_file():
        ps = capture(["ps", "-axo", "pid=,command="], 10)
        matches = [int(fields[0]) for line in ps.splitlines()
                   if len(fields := line.strip().split(" ", 1)) == 2 and fields[0].isdigit()
                   and os.path.realpath(fields[1].split(" ", 1)[0]) == os.path.realpath(executable)]
        if len(matches) == 1 and process_matches(matches[0], executable):
            state["hostPID"] = matches[0]
            save()
        time.sleep(0.25)
    if not fixture.is_file() or not state["hostPID"]:
        raise RuntimeError("isolated host fixture was not ready")
    if fixture.stat().st_mode & 0o077:
        raise RuntimeError("pairing fixture is not private")
    private = json.loads(fixture.read_text())
    result["caseCorrelation"] = {
        "case": "stoppedSavedAgentOutput",
        "cardIdHash": hashlib.sha256(private["cardId"].encode()).hexdigest()[:16],
        "shellCardIdHash": hashlib.sha256(private["shellCardId"].encode()).hexdigest()[:16],
        "request": "GET /v1/cards/<fixture-card>/output",
    }
    uri = urllib.parse.urlparse(private["pairingURI"])
    encoded = urllib.parse.parse_qs(uri.query)["data"][0]
    descriptor = json.loads(base64.urlsafe_b64decode(encoded + "=" * (-len(encoded) % 4)))
    origin = urllib.parse.urlparse(descriptor["origin"])
    if origin.scheme != "https" or origin.hostname != "127.0.0.1":
        raise RuntimeError("smoke pairing origin is not loopback HTTPS")
    listeners = capture(["lsof", "-nP", "-a", "-p", str(state["hostPID"]), "-iTCP", "-sTCP:LISTEN"], 10)
    if f"127.0.0.1:{origin.port}" not in listeners:
        raise RuntimeError("smoke listener does not belong to the identified host PID")
    result["hostIdHash"] = __import__("hashlib").sha256(descriptor["hostId"].encode()).hexdigest()[:16]
    result["listenerPort"] = origin.port
    result["isolationVerified"] = True
    if SUITE == "ui":
        probe_live_agent(macos / "tmux")
    checkpoint = "smoke-check connector-transport-ready"
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        log_path = DATA / "app.log"
        if log_path.is_file() and checkpoint in log_path.read_text():
            break
        time.sleep(0.1)
    else:
        raise RuntimeError("isolated host smoke checkpoint did not arrive")
    run([str(ROOT / "scripts/smoke-verdict"), str(DATA), "connector-transport"],
        log=RUN / "host-verdict.log", timeout=30)
    result["tests"]["hostSmoke"] = "PASS"

    container = pathlib.Path(capture(["xcrun", "simctl", "get_app_container", udid,
                                      "io.c9r.deck.connector", "data"], 30))
    sim_fixture = container / "Documents/connector-diagnose-fixture.json"
    shutil.copy2(fixture, sim_fixture)
    os.chmod(sim_fixture, 0o600)
    state["simulatorFixture"] = str(sim_fixture)
    save()
    plans = list(products.glob(("*DeckConnectorUITests*" if SUITE == "ui" else "DeckConnector_*") + ".xctestrun"))
    if len(plans) != 1:
        raise RuntimeError("expected one UI test plan")
    plan = products / ("DeckConnectorUITests-diagnose.xctestrun" if SUITE == "ui" else "DeckConnector-diagnose.xctestrun")
    shutil.copy2(plans[0], plan)
    with open(plan, "rb") as file:
        contents = plistlib.load(file)
    targets = ([target for config in contents["TestConfigurations"] for target in config["TestTargets"]]
               if "TestConfigurations" in contents else
               [target for target in contents.values() if isinstance(target, dict)])
    match = [target for target in targets if target.get("BlueprintName") ==
             ("DeckConnectorUITests" if SUITE == "ui" else "DeckConnectorTests")]
    if len(match) != 1:
        raise RuntimeError("UI test target missing from xctestrun")
    if SUITE == "appmodel":
        # App-hosted XCTest receives TestingEnvironmentVariables on Xcode 27.
        match[0].setdefault("TestingEnvironmentVariables", {})["DECK_CONNECTOR_SMOKE_FIXTURE"] = str(sim_fixture)
        match[0].setdefault("EnvironmentVariables", {})["DECK_CONNECTOR_SMOKE_FIXTURE"] = str(sim_fixture)
    elif result.get("realAgentMarker"):
        match[0].setdefault("TestingEnvironmentVariables", {})["DECK_DIAG_EXPECTED_MARKER"] = result["realAgentMarker"]
        match[0].setdefault("EnvironmentVariables", {})["DECK_DIAG_EXPECTED_MARKER"] = result["realAgentMarker"]
    if SUITE == "ui":
        shell_marker = "shellmarker" + RUN.name.removeprefix("deck-connector-diagnose-").replace("_", "")
        match[0].setdefault("TestingEnvironmentVariables", {})["DECK_DIAG_EXITED_MARKER"] = shell_marker
        match[0].setdefault("EnvironmentVariables", {})["DECK_DIAG_EXITED_MARKER"] = shell_marker
    with open(plan, "wb") as file:
        plistlib.dump(contents, file)
    test_command = ["xcodebuild", "-xctestrun", str(plan), "-destination", destination,
                    "-parallel-testing-enabled", "NO",
                    "-resultBundlePath", str(RUN / ("ui.xcresult" if SUITE == "ui" else "appmodel.xcresult")),
                    "test-without-building"]
    selected = (["DeckConnectorUITests/ConnectorUITests/testIsolatedStoppedAgentVisibilityAndOutput",
                 "DeckConnectorUITests/ConnectorUITests/testIsolatedLiveCodexOutput"]
                if SUITE == "ui" else
                ["DeckConnectorTests/AppModelTests/testOptInRealHostPairingBufferCASAndCredentialLifecycle",
                 "DeckConnectorTests/AppModelTests/testOutputIssueRecoversWhenAReadableOutputArrives"])
    test_command += ["-only-testing:" + name for name in selected]
    test_failure = None
    if SUITE == "ui":
        try:
            run(test_command, log=RUN / "ui-test.log", timeout=600)
        except Exception as error:
            test_failure = str(error)
    else:
        with open(RUN / "appmodel-test.log", "w") as output:
            process = subprocess.Popen(test_command, cwd=ROOT, env=ENV, stdout=output,
                                       stderr=subprocess.STDOUT)
            deadline = time.monotonic() + 600
            try:
                while process.poll() is None and time.monotonic() < deadline:
                    if not sim_fixture.is_file():
                        try:
                            sim_fixture.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
                            shutil.copy2(fixture, sim_fixture)
                            os.chmod(sim_fixture, 0o600)
                        except FileNotFoundError:
                            # Xcode can replace the same test app container mid-copy.
                            pass
                    time.sleep(0.1)
                if process.poll() is None:
                    raise TimeoutError("app-hosted XCTest exceeded 600 seconds")
                if process.returncode:
                    raise RuntimeError("app-hosted XCTest failed; see appmodel-test.log")
            finally:
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait(timeout=5)
    bundle = RUN / ("ui.xcresult" if SUITE == "ui" else "appmodel.xcresult")
    summary = json.loads(capture(["xcrun", "xcresulttool", "get", "test-results", "summary",
                                  "--path", str(bundle)], 30))
    result["testCounts"] = {key: summary.get(key) for key in
                            ("passedTests", "failedTests", "skippedTests", "totalTestCount")}
    nodes = json.loads(capture(["xcrun", "xcresulttool", "get", "test-results", "tests",
                                "--path", str(bundle)], 30))
    def test_cases(node):
        if isinstance(node, dict):
            if node.get("nodeType") == "Test Case":
                yield node
            for value in node.values():
                yield from test_cases(value)
        elif isinstance(node, list):
            for item in node:
                yield from test_cases(item)
    observed = {node["name"].removesuffix("()"): node.get("result", "Unknown")
                for node in test_cases(nodes)}
    result["testResults"] = observed
    if len(observed) != len(selected):
        raise RuntimeError("selected XCTest cases did not all execute")
    if SUITE == "ui":
        run(["xcrun", "xcresulttool", "export", "attachments", "--path", str(bundle),
             "--output-path", str(RUN / "screenshots"), "--filter", "Stopped agent round*"],
            log=RUN / "screenshot-export.log", timeout=60)
    result["tests"]["simulatorUI" if SUITE == "ui" else "appModelRealHost"] = (
        "PASS" if not test_failure and all(value == "Passed" for value in observed.values()) else "FAIL")
    if SUITE == "ui":
        stopped = observed.get("testIsolatedStoppedAgentVisibilityAndOutput")
        live = observed.get("testIsolatedLiveCodexOutput")
        if stopped:
            result["cases"]["stoppedSavedAgentOutput"] = "PASS" if stopped == "Passed" else "FAIL"
            result["cases"]["foregroundCycle"] = "PASS" if stopped == "Passed" else "FAIL"
            result["cases"]["appRelaunchCredentialRecovery"] = "PASS" if stopped == "Passed" else "FAIL"
        if live:
            result["cases"]["liveSavedAgentOutput"] = (
                "PASS" if live == "Passed" else "BLOCKED" if live == "Skipped" else "FAIL")
        if probe_agent_exit_to_shell(macos / "tmux"):
            shell_bundle = RUN / "shell-return.xcresult"
            shell_command = [value for value in test_command if not value.startswith("-only-testing:")]
            shell_command[shell_command.index("-resultBundlePath") + 1] = str(shell_bundle)
            shell_command.append("-only-testing:DeckConnectorUITests/ConnectorUITests/testIsolatedCodexExitToShellRejectsOutputAndSend")
            shell_failure = None
            try:
                run(shell_command, log=RUN / "shell-return-test.log", timeout=300)
            except Exception as error:
                shell_failure = str(error)
            shell_summary = json.loads(capture(["xcrun", "xcresulttool", "get", "test-results", "summary",
                                                "--path", str(shell_bundle)], 30))
            result["shellReturnTestCounts"] = {key: shell_summary.get(key) for key in
                                               ("passedTests", "failedTests", "skippedTests", "totalTestCount")}
            shell_nodes = json.loads(capture(["xcrun", "xcresulttool", "get", "test-results", "tests",
                                              "--path", str(shell_bundle)], 30))
            shell_observed = {node["name"].removesuffix("()"): node.get("result", "Unknown")
                              for node in test_cases(shell_nodes)}
            result["shellReturnTestResults"] = shell_observed
            shell_status = shell_observed.get("testIsolatedCodexExitToShellRejectsOutputAndSend")
            result["cases"]["agentExitedToShell"] = "PASS" if shell_status == "Passed" else "FAIL"
            result["tests"]["shellReturnUI"] = "PASS" if not shell_failure and shell_status == "Passed" else "FAIL"
            if shell_failure:
                raise RuntimeError(shell_failure)
        else:
            result["cases"]["agentExitedToShell"] = "BLOCKED"
    else:
        result["cases"]["ordinaryShellVisibility"] = (
            "PASS" if observed.get("testOptInRealHostPairingBufferCASAndCredentialLifecycle") == "Passed" else "FAIL")
    selected_cases = ("stoppedSavedAgentOutput", "liveSavedAgentOutput", "agentExitedToShell") if SUITE == "ui" else ("ordinaryShellVisibility",)
    result["requiredTestsExecuted"] = all(result["cases"][name] in ("PASS", "FAIL") for name in selected_cases)
    result["productAssertionsPassed"] = result["requiredTestsExecuted"] and all(result["cases"][name] == "PASS" for name in selected_cases)
    if test_failure:
        raise RuntimeError(test_failure)


def cleanup():
    ok = True
    pid = state.get("hostPID")
    executable = pathlib.Path(state["hostBundle"]) / "Contents/MacOS/deck" if state.get("hostBundle") else None
    if not pid and executable:
        for line in capture(["ps", "-axo", "pid=,command="], 10).splitlines():
            fields = line.strip().split(" ", 1)
            if fields[0].isdigit() and len(fields) == 2 and \
                    os.path.realpath(fields[1].split(" ", 1)[0]) == os.path.realpath(executable):
                pid = int(fields[0])
                state["hostPID"] = pid
                break
    if pid and executable and process_matches(pid, executable):
        os.kill(pid, signal.SIGTERM)
        for _ in range(40):
            if not process_matches(pid, executable):
                break
            time.sleep(0.25)
        ok &= not process_matches(pid, executable)
    if state.get("hostBundle"):
        tmux = pathlib.Path(state["hostBundle"]) / "Contents/MacOS/tmux"
        if tmux.is_file():
            try:
                metadata = json.loads(capture([str(tmux), "-L", SOCKET, "show-option", "-gv",
                                               "@deck-server-metadata"], 10))
                if metadata.get("source") != "smoke" or metadata.get("buildIdentifier") != result.get("head"):
                    raise RuntimeError("tmux metadata does not match this run")
                run([str(tmux), "-L", SOCKET, "kill-server"], log=RUN / "tmux-cleanup.log", timeout=20)
            except subprocess.CalledProcessError:
                pass  # This run never created a tmux server.
            except Exception:
                ok = False
    socket_path = pathlib.Path(f"/private/tmp/tmux-{os.getuid()}") / SOCKET
    if socket_path.exists():
        mode = socket_path.stat().st_mode
        if not stat.S_ISSOCK(mode) or socket_path.stat().st_uid != os.getuid():
            ok = False
        else:
            connected = False
            for _ in range(20):
                probe = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                probe.settimeout(1)
                try:
                    connected = probe.connect_ex(str(socket_path)) == 0
                finally:
                    probe.close()
                if not connected:
                    break
                time.sleep(0.25)
            if connected:
                ok = False
            else:
                try:
                    socket_path.unlink()
                except OSError:
                    ok = False
    if state.get("simulatorFixture"):
        pathlib.Path(state["simulatorFixture"]).unlink(missing_ok=True)
    if state.get("simulatorUDID"):
        udid = state["simulatorUDID"]
        try:
            inventory = json.loads(capture(["xcrun", "simctl", "list", "devices", "-j"], 20))
            device = next((item for devices in inventory["devices"].values() for item in devices
                           if item.get("udid") == udid), None)
            if device and device.get("state") != "Shutdown":
                run(["xcrun", "simctl", "shutdown", udid], log=RUN / "sim-shutdown.log", timeout=90)
            if device:
                run(["xcrun", "simctl", "delete", udid], log=RUN / "sim-delete.log", timeout=90)
        except Exception:
            ok = False
    if DATA.is_dir() and not (pid and executable and process_matches(pid, executable)):
        if (DATA / "app.log").is_file():
            shutil.copy2(DATA / "app.log", RUN / "host-app.log")
        shutil.rmtree(DATA)
    # Codex may remember that the isolated empty work directory was trusted.
    # Remove only this run's exact section after its Agent and host are gone.
    config = pathlib.Path.home() / ".codex/config.toml"
    if config.is_file() and not (pid and executable and process_matches(pid, executable)):
        private_agent_dir = pathlib.Path(os.path.realpath(DATA / "agent-work"))
        heading = f'[projects."{private_agent_dir}"]'
        try:
            original = config.read_text()
            pattern = r"(?m)^" + re.escape(heading) + r'\ntrust_level = "trusted"\n\n'
            cleaned, matches = re.subn(pattern, "", original)
        except OSError:
            ok = False
        else:
            if matches == 1:
                temporary = config.with_name(config.name + "." + RUN.name + ".tmp")
                try:
                    fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
                    with os.fdopen(fd, "w") as file:
                        file.write(cleaned)
                    os.replace(temporary, config)
                    result["agentTrustEntryRemoved"] = True
                except OSError:
                    temporary.unlink(missing_ok=True)
                    ok = False
            elif matches > 1:
                ok = False
    if not (pid and executable and process_matches(pid, executable)):
        disposable_builds = [RUN / "DerivedData", RUN / "deck-smoke.app"]
        if os.environ.get("DECK_DIAG_PRESERVE_BUILD") != "1":
            disposable_builds.append(RUN / "rust-target")
        for path in disposable_builds:
            if path.is_dir():
                try:
                    shutil.rmtree(path)
                except OSError:
                    ok = False
    result["cleanupCompleted"] = ok
    result["finalGitStatus"] = capture(["git", "status", "--short"])
    state["cleanupCompleted"] = ok
    save()


def run_all():
    save()
    child_reports = {}
    preserved_target = None
    for suite in ("ui", "appmodel"):
        child_env = dict(ENV, DECK_DIAG_SUITE=suite)
        if suite == "ui":
            child_env["DECK_DIAG_PRESERVE_BUILD"] = "1"
        elif preserved_target:
            child_env["DECK_DIAG_REUSE_HOST_TARGET"] = str(preserved_target)
        with open(RUN / f"{suite}-driver.log", "w") as output:
            process = subprocess.run([sys.executable, str(pathlib.Path(__file__).resolve())],
                                     cwd=ROOT, env=child_env, stdout=output,
                                     stderr=subprocess.STDOUT, check=False)
        paths = [line.strip() for line in (RUN / f"{suite}-driver.log").read_text().splitlines()
                 if line.strip().endswith("/report.json")]
        if not paths or not pathlib.Path(paths[-1]).is_file():
            result["tests"][suite] = "BLOCKED"
            result[f"{suite}Failure"] = f"driver exited {process.returncode} without a report"
            continue
        child = json.loads(pathlib.Path(paths[-1]).read_text())
        if suite == "ui":
            candidate = pathlib.Path(paths[-1]).parent / "rust-target"
            if candidate.is_dir() and (candidate / "debug/deck-app").is_file():
                preserved_target = candidate
        child_reports[suite] = paths[-1]
        result["tests"].update(child["tests"])
        result["tests"][f"hostSmoke_{suite}"] = child["tests"].get("hostSmoke", "UNRUN")
        result["cases"].update({key: value for key, value in child["cases"].items()
                                if value != "UNRUN"})
        result[f"{suite}ExitCode"] = process.returncode
        result[f"{suite}TestCounts"] = child.get("testCounts")
        if suite == "ui":
            for key in ("head", "xcodeVersion", "selectedRuntime", "selectedDeviceType",
                        "hostBinarySHA256", "iosBinarySHA256", "sourceDiffSHA256"):
                result[key] = child.get(key)
            result["shellReturnTestCounts"] = child.get("shellReturnTestCounts")
        result[f"{suite}SimulatorUDID"] = json.loads((pathlib.Path(paths[-1]).parent / "manifest.json").read_text()).get("simulatorUDID")
        if child.get("failure"):
            result[f"{suite}Failure"] = child["failure"]
        save()
    result["childReports"] = child_reports
    result["environmentReady"] = len(child_reports) == 2 and all(
        json.loads(pathlib.Path(path).read_text())["environmentReady"] for path in child_reports.values())
    result["isolationVerified"] = len(child_reports) == 2 and all(
        json.loads(pathlib.Path(path).read_text())["isolationVerified"] for path in child_reports.values())
    result["cleanupCompleted"] = len(child_reports) == 2 and all(
        json.loads(pathlib.Path(path).read_text())["cleanupCompleted"] for path in child_reports.values())
    state["cleanupCompleted"] = result["cleanupCompleted"]
    required = ("liveSavedAgentOutput", "ordinaryShellVisibility", "stoppedSavedAgentOutput",
                "agentExitedToShell")
    result["requiredTestsExecuted"] = all(result["cases"][name] in ("PASS", "FAIL") for name in required)
    result["productAssertionsPassed"] = result["requiredTestsExecuted"] and all(
        result["cases"][name] == "PASS" for name in required) and all(
        result["tests"].get(name) == "PASS" for name in ("simulatorUI", "shellReturnUI", "appModelRealHost"))
    result["finalGitStatus"] = capture(["git", "status", "--short"])
    if preserved_target and preserved_target.is_dir():
        shutil.rmtree(preserved_target)
    save()


if __name__ == "__main__":
    if SUITE == "all":
        try:
            run_all()
        except Exception as error:
            result["failure"] = str(error)
            save()
            print(str(error), file=sys.stderr)
        print(REPORT)
        sys.exit(0 if result.get("productAssertionsPassed") and result.get("requiredTestsExecuted")
                 and all(result["tests"].get(name) == "PASS" for name in
                         ("simulatorUI", "shellReturnUI", "appModelRealHost"))
                 and result.get("cleanupCompleted") else 1)
    try:
        main()
    except Exception as error:
        result["failure"] = str(error)
        test_log = RUN / ("ui-test.log" if SUITE == "ui" else "appmodel-test.log")
        test_key = "simulatorUI" if SUITE == "ui" else "appModelRealHost"
        if test_log.exists() and result["tests"].get(test_key) != "PASS":
            result["tests"][test_key] = "FAIL"
        print(str(error), file=sys.stderr)
    finally:
        cleanup()
        print(REPORT)
    suite_test = "simulatorUI" if SUITE == "ui" else "appModelRealHost"
    sys.exit(0 if result["tests"].get(suite_test) == "PASS"
             and (SUITE != "ui" or result["tests"].get("shellReturnUI") == "PASS")
             and result["cleanupCompleted"] else 1)
