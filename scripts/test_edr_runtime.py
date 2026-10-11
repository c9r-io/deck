import importlib.util
import json
import os
import pathlib
import platform
import pwd
import re
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock


PATH = pathlib.Path(__file__).with_name("edr_runtime.py")
SPEC = importlib.util.spec_from_file_location("edr_runtime", PATH)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


class EdrRuntimeTests(unittest.TestCase):
    def test_process_and_smoke_tmux_are_parsed_without_metadata_payload(self):
        executable = MODULE.DEBUG_BUNDLES / "deck-smoke.app/Contents/MacOS/tmux"
        process = MODULE.parse_ps_line(
            f'240 1 09-00:14:24 {executable} '
            '-f /tmp/conf -L deck-smoke-safe start-server ; set-option -g '
            '@deck-server-metadata {"source":"smoke"}'
        )
        self.assertIsNotNone(process)
        self.assertEqual(
            MODULE.tmux_candidate(process),
            (
                str(executable),
                "deck-smoke-safe",
            ),
        )

    def test_production_and_arbitrary_sockets_are_never_candidates(self):
        for socket in ["deck", "default", "deck-smoke", "deck-dev-other"]:
            process = MODULE.parse_ps_line(
                f"10 1 00:01 /Applications/deck.app/Contents/MacOS/tmux -L {socket} start-server"
            )
            self.assertIsNone(MODULE.tmux_candidate(process), socket)

    def test_cleanup_requires_matching_closed_metadata(self):
        self.assertTrue(MODULE.cleanup_allowed("deck-smoke-one", "smoke"))
        self.assertTrue(MODULE.cleanup_allowed("deck-dev", "development"))
        self.assertFalse(MODULE.cleanup_allowed("deck", "installed"))
        self.assertFalse(MODULE.cleanup_allowed("deck-smoke-one", "development"))
        self.assertFalse(MODULE.cleanup_allowed("deck-dev", "smoke"))

    def test_app_ownership_is_exact_to_the_smoke_socket(self):
        executable = MODULE.DEBUG_BUNDLES / "deck-smoke.app/Contents/MacOS/deck"
        process = MODULE.parse_ps_line(
            f"99 1 00:01 {executable} --smoke-data-dir /tmp/root "
            "--smoke-tmux-socket deck-smoke-one"
        )
        app = MODULE.managed_app(process)
        self.assertIsNotNone(app)
        self.assertEqual(app.socket, "deck-smoke-one")

    def test_the_channel_carrier_owns_exactly_its_own_socket(self):
        # app/run.sh: deck-channel-smoke-<suffix>.app <-> deck-smoke-channel-<suffix>
        bundle = MODULE.DEBUG_BUNDLES / "deck-channel-smoke-17-42.app/Contents/MacOS"
        tmux = MODULE.parse_ps_line(
            f"10 1 00:01 {bundle}/tmux -f /tmp/conf -L deck-smoke-channel-17-42 start-server"
        )
        self.assertEqual(
            MODULE.tmux_candidate(tmux), (str(bundle / "tmux"), "deck-smoke-channel-17-42")
        )
        app = MODULE.managed_app(
            MODULE.parse_ps_line(
                f"11 1 00:01 {bundle}/deck --smoke-data-dir /tmp/deck-channel-x "
                "--smoke-tmux-socket deck-smoke-channel-17-42 --smoke-wkwebview channel-first-send"
            )
        )
        self.assertIsNotNone(app)
        self.assertEqual(app.socket, "deck-smoke-channel-17-42")
        # another run's socket, an ordinary smoke socket, or no suffix: not this carrier's
        for socket in ["deck-smoke-channel-17-43", "deck-smoke-one", "deck-smoke-channel-"]:
            other = MODULE.parse_ps_line(
                f"12 1 00:01 {bundle}/tmux -L {socket} start-server"
            )
            self.assertIsNone(MODULE.tmux_candidate(other), socket)
            other_app = MODULE.parse_ps_line(
                f"13 1 00:01 {bundle}/deck --smoke-tmux-socket {socket}"
            )
            self.assertIsNone(MODULE.managed_app(other_app), socket)
        empty = MODULE.DEBUG_BUNDLES / "deck-channel-smoke-.app/Contents/MacOS/tmux"
        self.assertIsNone(
            MODULE.tmux_candidate(
                MODULE.parse_ps_line(f"14 1 00:01 {empty} -L deck-smoke-channel- start-server")
            )
        )

    def test_production_app_is_not_managed(self):
        process = MODULE.parse_ps_line(
            "99 1 00:01 /Applications/deck.app/Contents/MacOS/deck-app"
        )
        self.assertIsNone(MODULE.managed_app(process))

    def test_lookalike_bundle_outside_the_repository_is_not_executed(self):
        process = MODULE.parse_ps_line(
            "10 1 00:01 /tmp/deck-smoke.app/Contents/MacOS/tmux "
            "-L deck-smoke-lookalike start-server"
        )
        self.assertIsNone(MODULE.tmux_candidate(process))


REPO = PATH.parent.parent
BUNDLED_TMUX = REPO / "app/src-tauri/binaries/tmux-aarch64-apple-darwin"
HAS_TMUX = sys.platform == "darwin" and platform.machine() == "arm64" and os.access(BUNDLED_TMUX, os.X_OK)
UID = os.getuid()
ACCOUNT_HOME = pathlib.Path(os.path.realpath(pwd.getpwuid(UID).pw_dir))


class Fixture(unittest.TestCase):
    """Real, harmless tmux servers in a directory of the test's own.

    Every server here is started and asked by its FULL socket path, so nothing a
    test does can reach a server outside its directory: not by a socket name, and
    not through the `TMUX` of a shell that itself runs inside an installed Deck.
    """

    def setUp(self):
        self.base = pathlib.Path(tempfile.mkdtemp(prefix="deck-exp-", dir="/private/tmp"))
        self.started = []  # (tmux, socket) of every server this test started
        self.naps = []
        self.addCleanup(self.teardown)

    def teardown(self):
        for tmux, socket in self.started:
            subprocess.run([str(tmux), "-S", str(socket), "kill-server"], capture_output=True, timeout=10, env=self.quiet())
        for nap in self.naps:
            if nap.poll() is None:
                nap.kill()
            nap.wait(timeout=5)
        shutil.rmtree(self.base, ignore_errors=True)

    @staticmethod
    def quiet():
        return {name: value for name, value in os.environ.items() if name not in ("TMUX", "TMUX_PANE", "TMUX_TMPDIR")}

    def tmux_at(self, directory):
        directory.mkdir(parents=True, exist_ok=True)
        shutil.copy2(BUNDLED_TMUX, directory / "tmux")  # a carrier brings its own tmux
        return directory / "tmux"

    def serve(self, tmux, socket):
        subprocess.run([str(tmux), "-S", str(socket), "-f", "/dev/null", "new-session", "-d", "-s", "s", "sleep 600"],
                       check=True, capture_output=True, timeout=10, env=self.quiet())
        self.started.append((tmux, socket))
        pid = MODULE.answering_pid(str(tmux), str(socket))
        self.assertIsNotNone(pid)
        return pid

    def alive(self, tmux, socket, pid):
        return MODULE.answering_pid(str(tmux), str(socket)) == pid

    def gone(self, tmux, socket, pid):
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            if MODULE.answering_pid(str(tmux), str(socket)) is None and MODULE.identity(pid) is None:
                return True
            time.sleep(0.1)
        return False


@unittest.skipUnless(HAS_TMUX, "needs the bundled arm64 tmux")
class ExperimentTests(Fixture):
    """The registered-experiment cleanup. Beside the experiment runs a SENTINEL
    server that no registry names: whatever a test does, it must still be there.
    Nothing here goes near an installed Deck: where a test needs something that
    looks like one, it is a path in a registry entry and is never contacted."""

    def setUp(self):
        super().setUp()
        self.root, self.beside = self.base / "root", self.base / "beside"
        self.tmux = self.tmux_at(self.root)
        self.socket = self.root / "exp.sock"
        self.server = self.serve(self.tmux, self.socket)
        self.sentinel_socket = self.beside / "sentinel.sock"
        self.sentinel = self.serve(self.tmux_at(self.beside), self.sentinel_socket)

    def nap(self):
        process = subprocess.Popen(["/bin/sleep", "600"])
        self.naps.append(process)
        return process.pid

    def tool(self, *arguments):
        return subprocess.run([sys.executable, str(PATH), "experiment", *arguments, "--root", str(self.root)],
                              capture_output=True, text=True, timeout=60)

    def sentinel_alive(self):
        return self.alive(self.beside / "tmux", self.sentinel_socket, self.sentinel)

    def register(self):
        self.assertEqual(self.tool("init").returncode, 0)
        added = self.tool("add-server", "--socket", str(self.socket), "--tmux", str(self.tmux))
        self.assertEqual(added.returncode, 0, added.stderr)
        return json.loads((self.root / "experiment.json").read_text())

    def rewrite(self, change):
        document = json.loads((self.root / "experiment.json").read_text())
        change(document)
        (self.root / "experiment.json").write_text(json.dumps(document))

    def refused(self, saying):
        result = self.tool("cleanup")
        self.assertEqual(result.returncode, 3, result.stdout + result.stderr)
        self.assertIn("REFUSED, nothing was stopped", result.stderr)
        self.assertIn(saying, result.stderr)
        self.assertNotIn("stopped tmux-server", result.stdout)
        self.assertTrue(self.alive(self.tmux, self.socket, self.server), "a refused cleanup stopped the experiment's own server")
        self.assertTrue(self.sentinel_alive(), "the sentinel was touched")

    def test_what_is_registered_is_stopped_and_the_server_beside_it_is_not(self):
        document = self.register()
        napper = self.nap()
        self.assertEqual(self.tool("add-process", "--pid", str(napper)).returncode, 0)
        entry = document["resources"][0]
        self.assertEqual((entry["kind"], entry["pid"], entry["socket"], entry["executable"]),
                         ("tmux-server", self.server, str(self.socket), str(self.tmux)))
        shown = self.tool("show")
        self.assertEqual(shown.returncode, 0, shown.stderr)
        self.assertEqual(shown.stdout.count(": running"), 2)
        self.assertTrue(self.alive(self.tmux, self.socket, self.server), "show stopped something")
        done = self.tool("cleanup")
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertTrue(self.gone(self.tmux, self.socket, self.server))
        self.assertIsNotNone(self.naps[0].wait(timeout=5))
        self.assertFalse(self.socket.exists(), "the stale socket file of the stopped server is removed")
        self.assertTrue(self.sentinel_alive(), "the sentinel was touched")
        # a second cleanup finds everything gone and says so instead of guessing
        again = self.tool("cleanup")
        self.assertEqual(again.returncode, 0)
        self.assertEqual(again.stdout.count(": gone"), 2)
        self.assertNotIn("stopped", again.stdout)

    def test_one_registered_resource_is_stopped_and_the_rest_keeps_running(self):
        self.register()
        napper = self.nap()
        self.assertEqual(self.tool("add-process", "--pid", str(napper)).returncode, 0)
        # a pid nobody registered: the sentinel's
        other = self.tool("stop", "--pid", str(self.sentinel))
        self.assertEqual(other.returncode, 3, other.stdout + other.stderr)
        self.assertIn("is registered 0 times", other.stderr)
        self.assertTrue(self.sentinel_alive(), "the sentinel was touched")
        # a registered pid that is another process now
        self.rewrite(lambda document: document["resources"][1].update(started="Mon Jan  1 00:00:00 2001"))
        reused = self.tool("stop", "--pid", str(napper))
        self.assertEqual(reused.returncode, 3)
        self.assertIn("is another process now", reused.stderr)
        self.assertIsNone(self.naps[0].poll(), "a refused stop signalled the process")
        true_start = MODULE.identity(napper)[0]
        self.rewrite(lambda document: document["resources"][1].update(started=true_start))
        done = self.tool("stop", "--pid", str(napper))
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertIn(f"stopped process pid {napper}", done.stdout)
        self.assertIsNotNone(self.naps[0].wait(timeout=5))
        self.assertTrue(self.alive(self.tmux, self.socket, self.server), "stopping one resource stopped the server too")
        self.assertTrue(self.sentinel_alive())

    def test_a_process_started_by_its_bare_name_is_registered_from_any_directory(self):
        # found in use: a keep-awake started as `caffeinate` shows no path, and the tool, run from a
        # person's home, resolved the bare name against that home and refused its own experiment's process
        self.register()
        process = subprocess.Popen(["sleep", "600"])
        self.naps.append(process)
        self.assertEqual(MODULE.identity(process.pid)[1], "sleep")
        added = subprocess.run([sys.executable, str(PATH), "experiment", "add-process", "--pid", str(process.pid),
                                "--root", str(self.root)], capture_output=True, text=True, timeout=60, cwd=str(ACCOUNT_HOME))
        self.assertEqual(added.returncode, 0, added.stdout + added.stderr)
        done = self.tool("stop", "--pid", str(process.pid))
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertIsNotNone(process.wait(timeout=5))
        # a path is still judged: the same name under an installed location is not an experiment's
        with self.assertRaisesRegex(MODULE.Refused, "is inside /Applications"):
            MODULE.not_formal_process("/Applications/deck.app/Contents/MacOS/sleep", "sleep 600")
        MODULE.not_formal_process("sleep", "sleep 600")

    def test_what_runs_from_the_root_unregistered_is_reported_and_left_running(self):
        self.register()
        other_socket = self.root / "other.sock"
        other = self.serve(self.tmux, other_socket)
        done = self.tool("cleanup")
        self.assertEqual(done.returncode, 1, done.stdout + done.stderr)
        self.assertIn(f"STILL RUNNING from the root and never registered: pid {other} {self.tmux}", done.stderr)
        self.assertTrue(self.gone(self.tmux, self.socket, self.server), "the registered server was not stopped")
        self.assertTrue(self.alive(self.tmux, other_socket, other), "an unregistered server was stopped")
        self.assertTrue(self.sentinel_alive(), "the sentinel was touched")

    def test_a_socket_outside_the_root_is_neither_registered_nor_stopped(self):
        self.register()
        added = self.tool("add-server", "--socket", str(self.sentinel_socket), "--tmux", str(self.tmux))
        self.assertEqual(added.returncode, 3)
        self.assertIn("leaves the experiment root", added.stderr)
        # written into the registry by hand, with the sentinel's true identity
        started, executable, _ = MODULE.identity(self.sentinel)
        self.rewrite(lambda document: document["resources"].append({
            "kind": "tmux-server", "socket": str(self.sentinel_socket), "tmux": str(self.tmux),
            "pid": self.sentinel, "started": started, "executable": executable}))
        self.refused("leaves the experiment root")

    def test_a_link_inside_the_root_does_not_carry_a_socket_out_of_it(self):
        self.register()
        link = self.root / "inside.sock"
        link.symlink_to(self.sentinel_socket)
        started, executable, _ = MODULE.identity(self.sentinel)
        self.rewrite(lambda document: document["resources"].append({
            "kind": "tmux-server", "socket": str(link), "tmux": str(self.tmux),
            "pid": self.sentinel, "started": started, "executable": executable}))
        self.refused("leaves the experiment root")

    def test_a_pid_that_is_another_process_now_is_refused(self):
        self.register()
        # the pid is right and running; the process registered under it started at another time
        self.rewrite(lambda document: document["resources"][0].update(started="Mon Jan  1 00:00:00 2001"))
        self.refused("is another process now")
        # and a registered program that is not the one running under the pid
        true_start = MODULE.identity(self.server)[0]
        self.rewrite(lambda document: document["resources"][0].update(
            started=true_start, executable=str(self.root / "another-tmux")))
        self.refused("is another process now")

    def test_a_socket_that_answers_as_another_pid_is_refused(self):
        self.register()
        napper = self.nap()
        started, executable, _ = MODULE.identity(napper)
        # the experiment's socket, claimed for a process that is not its server
        self.rewrite(lambda document: document["resources"].__setitem__(0, {
            "kind": "tmux-server", "socket": str(self.socket), "tmux": str(self.tmux),
            "pid": napper, "started": started, "executable": executable}))
        self.refused(f"pid {self.server} answers on")
        self.assertIsNone(self.naps[0].poll(), "a process of a refused cleanup was stopped")

    def test_one_bad_entry_refuses_the_good_ones_too(self):
        self.register()
        napper = self.nap()
        self.assertEqual(self.tool("add-process", "--pid", str(napper)).returncode, 0)
        self.rewrite(lambda document: document["resources"][1].update(started="Mon Jan  1 00:00:00 2001"))
        self.refused("is another process now")  # the server entry is good, and still is not stopped
        self.assertIsNone(self.naps[0].poll())

    def test_a_registry_that_is_incomplete_or_not_this_roots_is_refused(self):
        self.register()
        original = (self.root / "experiment.json").read_text()
        for change, saying in (
            (lambda document: document["resources"][0].pop("started"), "has no started"),
            (lambda document: document["resources"][0].update(pid=True), "has no pid"),
            (lambda document: document["resources"][0].update(kind="server"), "no known kind"),
            (lambda document: document["resources"][0].pop("socket"), "the socket is not an absolute path"),
            (lambda document: document["resources"][0].update(tmux=str(self.root / "absent")), "names no tmux program"),
            (lambda document: document.update(root=str(self.beside)), "is not this root's registry"),
        ):
            with self.subTest(saying=saying):
                (self.root / "experiment.json").write_text(original)
                self.rewrite(change)
                self.refused(saying)
        (self.root / "experiment.json").write_text("{ not json")
        self.refused("cannot be read")
        (self.root / "experiment.json").unlink()
        self.refused("cannot be read")

    def test_what_looks_like_an_installed_deck_is_refused_and_never_contacted(self):
        self.register()
        asked = []
        real = MODULE.run
        self.addCleanup(setattr, MODULE, "run", real)
        MODULE.run = lambda arguments: asked.append(list(arguments)) or real(arguments)
        formal_socket = f"/private/tmp/tmux-{UID}/deck"
        formal_tmux = "/Applications/deck.app/Contents/MacOS/tmux"
        root = MODULE.experiment_root(str(self.root))
        entry = {"kind": "tmux-server", "socket": formal_socket, "tmux": str(self.tmux),
                 "pid": self.server, "started": MODULE.identity(self.server)[0], "executable": str(self.tmux)}
        asked.clear()
        with self.assertRaisesRegex(MODULE.Refused, "leaves the experiment root"):
            MODULE.checked(root, entry)
        self.assertEqual(asked, [], "a refused entry made the tool run something")
        for place in (formal_socket, formal_tmux, str(ACCOUNT_HOME / ".deck/tmux.conf"),
                      str(ACCOUNT_HOME / "Applications/deck.app/Contents/MacOS/tmux")):
            self.assertIsNotNone(MODULE.formal(pathlib.Path(place)), place)
        self.assertIsNone(MODULE.formal(self.tmux))
        for place in ("/Applications", str(ACCOUNT_HOME), str(ACCOUNT_HOME / ".deck"), "/private/tmp",
                      f"/private/tmp/tmux-{UID}", "relative/root", str(self.base / "absent")):
            with self.assertRaises(MODULE.Refused, msg=place):
                MODULE.experiment_root(place)
        with self.assertRaisesRegex(MODULE.Refused, "account's own ~/.deck"):
            MODULE.not_formal_process(str(self.tmux), f"{self.tmux} -f {ACCOUNT_HOME}/.deck/tmux.conf start-server")
        with self.assertRaisesRegex(MODULE.Refused, "is inside /Applications"):
            MODULE.not_formal_process(formal_tmux, f"{formal_tmux} start-server")
        self.assertEqual(asked, [], "judging a path made the tool run something")
        self.assertTrue(self.sentinel_alive())

    def test_a_root_is_a_directory_of_its_own_under_a_temporary_directory(self):
        self.assertEqual(MODULE.experiment_root(str(self.root)), self.root)
        self.assertEqual(MODULE.experiment_root(f"/tmp/{self.base.name}/root"), self.root)  # through the /tmp link
        self.assertIn(pathlib.Path("/private/tmp"), MODULE.temporary_parents())
        # the same directory, were it somewhere that is not a temporary place
        with mock.patch.object(MODULE, "temporary_parents", lambda: [self.beside]):
            with self.assertRaisesRegex(MODULE.Refused, "under a temporary directory"):
                MODULE.experiment_root(str(self.root))
        # and the temporary place itself is nobody's experiment
        with mock.patch.object(MODULE, "temporary_parents", lambda: [self.root]):
            with self.assertRaisesRegex(MODULE.Refused, "under a temporary directory"):
                MODULE.experiment_root(str(self.root))

    def test_a_server_that_is_already_gone_is_reported_not_guessed(self):
        self.register()
        subprocess.run([str(self.tmux), "-S", str(self.socket), "kill-server"], check=True, capture_output=True,
                       timeout=10, env=self.quiet())
        self.assertTrue(self.gone(self.tmux, self.socket, self.server))
        shown = self.tool("show")
        self.assertEqual(shown.returncode, 0, shown.stderr)
        self.assertIn(": gone", shown.stdout)
        # another server, never registered, now answers on the registered path: not ours to stop
        other = self.serve(self.tmux, self.socket)
        result = self.tool("cleanup")
        self.assertEqual(result.returncode, 3)
        self.assertIn(f"pid {other}, which was never registered, answers on", result.stderr)
        self.assertTrue(self.alive(self.tmux, self.socket, other))

    def test_a_server_the_experiment_replaced_on_its_socket_is_registered_again(self):
        # a restart under test: the first server ends, a second one listens on the same path
        self.register()
        subprocess.run([str(self.tmux), "-S", str(self.socket), "kill-server"], check=True, capture_output=True,
                       timeout=10, env=self.quiet())
        self.assertTrue(self.gone(self.tmux, self.socket, self.server))
        second = self.serve(self.tmux, self.socket)
        added = self.tool("add-server", "--socket", str(self.socket), "--tmux", str(self.tmux))
        self.assertEqual(added.returncode, 0, added.stderr)
        again = self.tool("add-server", "--socket", str(self.socket), "--tmux", str(self.tmux))
        self.assertEqual(again.returncode, 3)
        self.assertIn("is registered already", again.stderr)
        done = self.tool("cleanup")
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertIn(f"pid {self.server} ", done.stdout.splitlines()[0])
        self.assertIn(": gone", done.stdout.splitlines()[0])
        self.assertIn(f"stopped tmux-server pid {second}", done.stdout)
        self.assertTrue(self.gone(self.tmux, self.socket, second))
        self.assertTrue(self.sentinel_alive())

    def test_the_tool_runs_from_standard_input_the_way_another_machine_gets_it(self):
        self.register()
        with open(PATH) as program:
            shown = subprocess.run([sys.executable, "-", "experiment", "show", "--root", str(self.root)],
                                   stdin=program, capture_output=True, text=True, timeout=60)
        self.assertEqual(shown.returncode, 0, shown.stderr)
        self.assertIn(f"tmux-server pid {self.server}", shown.stdout)

    def test_a_named_socket_is_resolved_to_its_path_and_bound_to_its_pid(self):
        # the reviewed development/smoke inventory knows a server by NAME: the name is
        # resolved to where `tmux -L` would look from this environment, and from then
        # on the server is that path and the pid that answers on it
        directory = self.base / "named"
        (directory / f"tmux-{UID}").mkdir(parents=True)
        name = "deck-smoke-edr-test"
        path = f"{directory}/tmux-{UID}/{name}"
        pid = self.serve(self.tmux, path)
        with mock.patch.dict(os.environ, {"TMUX_TMPDIR": str(directory)}):
            self.assertEqual(MODULE.default_socket(name), path)
            self.assertEqual(MODULE.bound_socket(MODULE.Process(pid, 1, "0:01", ()), str(self.tmux), name), path)
            # the same name, held for a process that is not the one that answers there
            wrong = MODULE.Server(self.sentinel, 1, "0:01", str(self.tmux), name, "smoke", ())
            with self.assertRaisesRegex(RuntimeError, "is not the server that answers on"):
                MODULE.cleanup(wrong, False)
            self.assertTrue(self.alive(self.tmux, path, pid))
            self.assertTrue(self.sentinel_alive())
            MODULE.cleanup(MODULE.Server(pid, 1, "0:01", str(self.tmux), name, "smoke", ()), False)
        self.assertTrue(self.gone(self.tmux, path, pid))
        self.assertTrue(self.sentinel_alive())


@unittest.skipUnless(HAS_TMUX, "needs the bundled arm64 tmux")
class CompatCandidateTests(Fixture):
    """scripts/test-compat-candidate launches a RELEASE build, whose shell service is
    the socket `deck` in the default tmux directory. Its cleanup function is run here
    as the script has it, against a directory standing for that default one."""

    def setUp(self):
        super().setUp()
        script = (REPO / "scripts/test-compat-candidate").read_text()
        start = script.index("stop_candidate_server() {\n")
        self.function = script[start:script.index("\n}\n", start) + 3]
        self.candidate = self.tmux_at(self.base / "work/deck-compat.app/Contents/MacOS")
        self.installed = self.tmux_at(self.base / "installed")  # stands for an installed Deck's tmux
        self.sockets = self.base / "sockets"
        (self.sockets / f"tmux-{UID}").mkdir(parents=True)
        self.socket = self.sockets / f"tmux-{UID}/deck"

    def stop(self):
        environment = {"PATH": "/usr/bin:/bin", "TMUX_TMPDIR": str(self.sockets)}
        return subprocess.run(["/bin/sh", "-euc", self.function + 'stop_candidate_server "$1"\n', "sh", str(self.candidate)],
                              capture_output=True, text=True, timeout=30, env=environment)

    def test_the_candidates_own_server_is_stopped(self):
        pid = self.serve(self.candidate, self.socket)
        result = self.stop()
        self.assertEqual((result.returncode, result.stderr), (0, ""))
        self.assertTrue(self.gone(self.candidate, self.socket, pid))

    def test_another_server_on_the_same_socket_is_left_running(self):
        pid = self.serve(self.installed, self.socket)
        result = self.stop()
        self.assertEqual(result.returncode, 0)
        self.assertIn("is not this candidate's; left running", result.stderr)
        self.assertTrue(self.alive(self.installed, self.socket, pid))

    def test_nothing_on_the_socket_is_nothing_to_do(self):
        result = self.stop()
        self.assertEqual((result.returncode, result.stderr), (0, ""))


def code_lines(file):
    """(number, line) of what `file` executes: no comment lines, no module docstring."""
    lines = file.read_text(errors="replace").splitlines()
    skip = 0
    if file.suffix == ".py" and '"""' in "\n".join(lines[:3]):
        opening = next(index for index, line in enumerate(lines) if '"""' in line)
        closing = opening if lines[opening].count('"""') > 1 else next(
            index for index, line in enumerate(lines) if index > opening and '"""' in line)
        skip = closing + 1
    return [(number, line) for number, line in enumerate(lines, 1)
            if number > skip and not line.lstrip().startswith("#")]


def scripts():
    files = [file for file in (REPO / "scripts").rglob("*")
             if file.is_file() and "__pycache__" not in file.parts and file.suffix not in (".pyc", ".json")]
    return sorted([*files, REPO / "app/run.sh", REPO / "app/bundle.sh"])


# Every tracked script that stops a tmux server, and how it says WHICH server.
#   path: by the full socket path (`-S`), never by a name
#   run-name: by a name the same file builds for its own run, starting `deck-smoke-`;
#             the text beside it is where the file builds that name
# A script that is not here, or that says it another way, fails the test below.
KILL_SERVER_SITES = {
    "scripts/edr_runtime.py": ("path", None),
    "scripts/ce_parity.py": ("path", 'Tmux(["-f", str(fixture.conf), "-S", str(fixture.tmux_socket)]'),
    "scripts/test-compat-candidate": ("path", None),
    "scripts/translation-lens-verify.py": ("run-name", 'socket = f"deck-smoke-tl{RUN_ID}'),
    "scripts/reminder-verify.py": ("run-name", "self.socket = 'deck-smoke-reminder-' + self.run_id"),
    "scripts/connector-simulator-diagnose.py": ("run-name", 'SOCKET = "deck-smoke-" + RUN.name'),
}
BY_NAME = re.compile(r"""(^|[\s\[(,])["']?-L["']?[\s,]""")
PRODUCT_NAME = re.compile(r"""(^|[\s\[(,])["']?-L["']?[\s,]+["']?deck["']?([\s,\])]|$)""")


class NamedSocketRuleTests(unittest.TestCase):
    """`tmux -L deck …` reaches whichever server is called deck from the caller's
    environment; from an ordinary shell that is the installed Deck's."""

    def test_the_patterns_see_what_they_are_for(self):
        for line in ('"$tmux" -L deck kill-server', 'run([tmux, "-L", "deck", "kill-server"])', "tmux -L deck"):
            self.assertRegex(line, PRODUCT_NAME)
            self.assertRegex(line, BY_NAME)
        for line in ('run([tmux, "-L", socket, "kill-server"])', "tmux -L deck-smoke-1 ls", 'tmux -L "deck-dev" ls'):
            self.assertNotRegex(line, PRODUCT_NAME)
            self.assertRegex(line, BY_NAME)
        self.assertNotRegex('run([tmux, "-S", path, "kill-server"])', BY_NAME)

    def test_no_tracked_script_names_the_installed_products_socket(self):
        offenders = [f"{file.relative_to(REPO)}:{number}" for file in scripts() if file != pathlib.Path(__file__).resolve()
                     for number, line in code_lines(file) if PRODUCT_NAME.search(line)]
        self.assertEqual(offenders, [])

    def test_every_script_that_stops_a_server_is_registered_with_how_it_names_it(self):
        found = {}
        this = pathlib.Path(__file__).resolve()
        for file in scripts():
            lines = [line for _, line in code_lines(file) if "kill-server" in line]
            if file == this:
                # the guard holds the offending words as samples; what it RUNS is by path
                ran = [line for line in lines if "subprocess.run(" in line]
                self.assertTrue(ran)
                for line in ran:
                    self.assertNotRegex(line, BY_NAME)
                    self.assertIn('"-S"', line)
            elif lines:
                found[str(file.relative_to(REPO))] = lines
        self.assertEqual(sorted(found), sorted(KILL_SERVER_SITES))
        for name, (how, built) in KILL_SERVER_SITES.items():
            text = (REPO / name).read_text()
            if built is not None:
                self.assertIn(built, text, name)
            for line in found[name]:
                if how == "path":
                    self.assertNotRegex(line, BY_NAME, f"{name}: {line.strip()}")
                else:
                    # by name, and the name is a variable: never a name written out
                    self.assertRegex(line, r"""-L["'],\s*(SOCKET|socket|self\.socket)\s*,""", f"{name}: {line.strip()}")

    def test_this_tool_asks_and_stops_servers_by_path_and_without_the_callers_tmux(self):
        code = "\n".join(line for _, line in code_lines(PATH))
        self.assertEqual(re.findall(r'run\(\s*\[[^\]]*"-L"', code), [], "a tmux command addressed by socket name")
        self.assertIn('"-S", path, "kill-server"', code)
        self.assertIn('"-S", socket_path, "kill-server"', code)
        with mock.patch.dict(os.environ, {"TMUX": "/private/tmp/tmux-0/deck,1,0", "TMUX_PANE": "%1"}):
            seen = MODULE.run(["/usr/bin/env"]).stdout
        self.assertNotIn("TMUX=", seen)
        self.assertNotIn("TMUX_PANE=", seen)
        self.assertIn("LC_ALL=C", seen)


if __name__ == "__main__":
    unittest.main()
