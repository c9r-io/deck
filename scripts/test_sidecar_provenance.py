import contextlib
import importlib.util
import io
import json
import pathlib
import plistlib
import re
import struct
import subprocess
import sys
import tempfile
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[1]
PATH = pathlib.Path(__file__).with_name("sidecar_provenance.py")
SPEC = importlib.util.spec_from_file_location("sidecar_provenance", PATH)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)

TRIPLE = "aarch64-apple-darwin"
# the Pi observer's files (app/bundle.sh DECK_BUNDLE_PI_RESOURCES)
PI_RESOURCES = ("observer.ts", "signal.ts", "lifecycle.ts", "both.ts")
# Literals, not the tool's own table: a wrong table must fail these tests.
BUILT = {
    "deck-status-helper": "status-helper",
    "deck-lifecycle-helper": "lifecycle-helper",
    "deck-mcp-runner": "mcp-runner",
    "deck-mcp": "mcp-adapter",
}


def image(uuid, sections=(), tail=b""):
    """A minimal 64-bit Mach-O image: LC_UUID and one segment's section names."""
    commands = b""
    count = 0
    if uuid is not None:
        commands += struct.pack("<2I", 0x1B, 24) + bytes.fromhex(uuid)
        count += 1
    if sections:
        body = b"".join(
            name.encode().ljust(16, b"\0") + b"__DATA".ljust(16, b"\0") + b"\0" * 48
            for name in sections
        )
        commands += (
            struct.pack("<2I", 0x19, 72 + len(body))
            + b"__DATA".ljust(16, b"\0")
            + struct.pack("<4Q", 0, 0, 0, 0)
            + struct.pack("<4I", 7, 3, len(sections), 0)
            + body
        )
        count += 1
    return struct.pack("<8I", 0xFEEDFACF, 0x0100000C, 0, 2, count, len(commands), 0, 0) + commands + tail


def universal(*images):
    header = struct.pack(">2I", 0xCAFEBABE, len(images))
    offset = 8 + 20 * len(images)
    table = b""
    for data in images:
        table += struct.pack(">5I", 0x0100000C, 0, offset, len(data), 0)
        offset += len(data)
    return header + table + b"".join(images)


def uuid(seed):
    return f"{seed:032X}"


PLAIN = ("__text", "__const", "__llvm_addrsig")
DEVELOPMENT_ASSETS = b"/test/static.test.mjs\0/test/wk-smoke.mjs\0"
INSTRUMENTED = ("__text", "__llvm_prf_cnts", "__llvm_prf_data", "__llvm_prf_names", "__llvm_covmap")


class Tree:
    """A stand-in app/src-tauri with every context's products and a placement."""

    def __init__(self, root):
        self.tauri = pathlib.Path(root) / "app" / "src-tauri"
        (self.tauri / "binaries").mkdir(parents=True)
        (self.tauri / "tauri.conf.json").write_text("{}")
        self.ids = {}
        seed = 1
        for binary, crate in BUILT.items():
            for context, sections in (("debug", PLAIN), ("release", PLAIN), ("coverage", INSTRUMENTED)):
                self.ids[binary, context] = uuid(seed)
                path = self.product(binary, context)
                path.parent.mkdir(parents=True)
                path.write_bytes(image(uuid(seed), sections))
                seed += 1
        (self.tauri / "binaries" / f"tmux-{TRIPLE}").write_bytes(image(uuid(900), PLAIN))
        # the executable names every embedded frontend file; only a debug
        # profile stages ui/test
        for profile, seed, assets in (("debug", 901, DEVELOPMENT_ASSETS), ("release", 902, b"")):
            main = self.tauri / "target" / profile / "deck-app"
            main.parent.mkdir(parents=True)
            main.write_bytes(image(uuid(seed), PLAIN, tail=b"\0/index.html\0" + assets))

    def product(self, binary, context):
        target = self.tauri / BUILT[binary] / "target"
        if context == "coverage":
            return target / "llvm-cov" / TRIPLE / "debug" / binary
        return target / TRIPLE / context / binary

    def place(self, context, only=None):
        for binary in BUILT:
            if only in (None, binary):
                (self.tauri / "binaries" / f"{binary}-{TRIPLE}").write_bytes(self.product(binary, context).read_bytes())

    def bundle(self, context, executable="deck", signed=False):
        app = self.tauri.parent / f"{context}.app"
        macos = app / "Contents" / "MacOS"
        macos.mkdir(parents=True, exist_ok=True)
        tail = b"\0signature" if signed else b""
        for binary in BUILT:
            (macos / binary).write_bytes(self.product(binary, context).read_bytes() + tail)
        (macos / "tmux").write_bytes((self.tauri / "binaries" / f"tmux-{TRIPLE}").read_bytes() + tail)
        (macos / executable).write_bytes((self.tauri / "target" / context / "deck-app").read_bytes() + tail)
        with open(app / "Contents" / "Info.plist", "wb") as handle:
            plistlib.dump({"CFBundleExecutable": executable}, handle)
        return app


def run(*argv):
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        status = MODULE.main(list(argv))
    return status, out.getvalue(), err.getvalue()


class InspectTests(unittest.TestCase):
    def write(self, data):
        handle = tempfile.NamedTemporaryFile(delete=False, prefix="deck-sidecar-test-")
        self.addCleanup(pathlib.Path(handle.name).unlink)
        handle.write(data)
        handle.close()
        return handle.name

    def test_plain_and_instrumented_images_are_told_apart(self):
        plain = MODULE.inspect(self.write(image(uuid(7), PLAIN)))
        self.assertEqual(plain, [{"uuid": uuid(7), "instrumented": False, "sections": []}])
        instrumented = MODULE.inspect(self.write(image(uuid(8), INSTRUMENTED)))
        self.assertTrue(instrumented[0]["instrumented"])
        # a 16-byte section name has no terminator and is still read whole
        self.assertIn("__llvm_prf_names", instrumented[0]["sections"])
        self.assertNotIn("__llvm_addrsig", instrumented[0]["sections"])

    def test_every_slice_of_a_universal_file_is_read(self):
        path = self.write(universal(image(uuid(1), PLAIN), image(uuid(2), INSTRUMENTED)))
        self.assertEqual(MODULE.identity(path), ((uuid(1), uuid(2)), True))

    def test_a_signature_appended_after_the_image_does_not_change_the_answer(self):
        bare = MODULE.identity(self.write(image(uuid(3), PLAIN)))
        signed = MODULE.identity(self.write(image(uuid(3), PLAIN, tail=b"\0" * 64)))
        self.assertEqual(bare, signed)

    def test_what_cannot_be_read_never_answers(self):
        whole = image(uuid(4), INSTRUMENTED)
        for data in (b"", b"#!/bin/sh\n", whole[:20], whole[:-40], struct.pack(">2I", 0xCAFEBABE, 0)):
            with self.assertRaises(MODULE.Unreadable):
                MODULE.inspect(self.write(data))
        with self.assertRaises(MODULE.Unreadable):
            MODULE.inspect("/nonexistent/deck-sidecar")
        # readable, but nothing to compare provenance with
        with self.assertRaises(MODULE.Unreadable):
            MODULE.identity(self.write(image(None, PLAIN)))
        status, out, _ = run("inspect", self.write(b"not a program"))
        self.assertEqual(status, 2)
        self.assertIn("unreadable", out)

    @unittest.skipUnless(sys.platform == "darwin", "reads a system program")
    def test_a_real_system_program_is_plain_and_has_an_identity(self):
        uuids, instrumented = MODULE.identity("/bin/ls")
        self.assertFalse(instrumented)
        self.assertTrue(all(re.fullmatch(r"[0-9A-F]{32}", value) for value in uuids))


class CheckTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory(prefix="deck-sidecar-tree-")
        self.addCleanup(directory.cleanup)
        self.tree = Tree(directory.name)
        self.tauri = str(self.tree.tauri)

    def check(self, *argv):
        return run("check", "--tauri", self.tauri, *argv)

    def test_each_context_accepts_only_its_own_placement(self):
        expected = {
            "debug": ("--profile", "debug"),
            "release": ("--profile", "release"),
            "coverage": ("--profile", "debug", "--coverage"),
        }
        for placed in expected:
            self.tree.place(placed)
            for context, argv in expected.items():
                status, out, err = self.check(*argv, "--binaries")
                self.assertEqual(status, 0 if context == placed else 1, (placed, context, err))
                if context == placed:
                    self.assertIn("4 files", out)

    def test_one_foreign_sidecar_is_named(self):
        self.tree.place("debug")
        self.tree.place("coverage", only="deck-lifecycle-helper")
        status, _, err = self.check("--profile", "debug", "--binaries")
        self.assertEqual(status, 1)
        self.assertIn("deck-lifecycle-helper", err)
        self.assertIn("is instrumented, expected plain", err)
        self.assertIn(self.tree.ids["deck-lifecycle-helper", "coverage"], err)
        for other in ("deck-status-helper:", "deck-mcp-runner:", "deck-mcp:"):
            self.assertNotIn(other, err)

    def test_a_missing_or_unreadable_file_fails_closed(self):
        self.tree.place("release")
        placed = self.tree.tauri / "binaries" / f"deck-mcp-{TRIPLE}"
        placed.unlink()
        self.assertEqual(self.check("--profile", "release", "--binaries")[0], 1)
        placed.write_text("not a program")
        self.assertEqual(self.check("--profile", "release", "--binaries")[0], 1)
        # the product the placement is compared with is gone
        self.tree.place("release")
        self.tree.product("deck-mcp", "release").unlink()
        status, _, err = self.check("--profile", "release", "--binaries")
        self.assertEqual(status, 1)
        self.assertIn("expected build product cannot be read", err)

    def test_a_product_in_the_wrong_state_is_not_a_reference(self):
        # an instrumented file in the ordinary target directory: equal UUIDs
        # would otherwise make an instrumented placement look right
        wrong = image(uuid(77), INSTRUMENTED)
        self.tree.product("deck-status-helper", "debug").write_bytes(wrong)
        self.tree.place("debug")
        status, _, err = self.check("--profile", "debug", "--binaries")
        self.assertEqual(status, 1)
        self.assertIn("this context builds plain sidecars", err)

    def test_a_bundle_is_checked_with_tmux_and_its_executable(self):
        for signed in (False, True):
            app = self.tree.bundle("release", signed=signed)
            status, out, err = self.check("--profile", "release", "--bundle", str(app))
            self.assertEqual(status, 0, err)
            self.assertIn("6 files", out)
        # a debug sidecar inside the release bundle
        macos = app / "Contents" / "MacOS"
        (macos / "deck-mcp").write_bytes(self.tree.product("deck-mcp", "debug").read_bytes())
        status, _, err = self.check("--profile", "release", "--bundle", str(app))
        self.assertEqual(status, 1)
        self.assertIn("deck-mcp:", err)
        self.assertNotIn("deck-mcp-runner:", err)

    def test_a_bundle_executable_from_another_profile_fails(self):
        app = self.tree.bundle("release", executable="deck-app")
        debug_main = self.tree.tauri / "target" / "debug" / "deck-app"
        (app / "Contents" / "MacOS" / "deck-app").write_bytes(debug_main.read_bytes())
        status, _, err = self.check("--profile", "release", "--bundle", str(app))
        self.assertEqual(status, 1)
        self.assertIn("deck-app:", err)
        self.assertEqual(self.check("--profile", "release", "--bundle", str(app), "--no-main")[0], 0)

    def test_the_executable_embeds_its_own_profiles_frontend(self):
        # a release executable compiled against the tree a debug build staged
        release = self.tree.tauri / "target" / "release" / "deck-app"
        release.write_bytes(release.read_bytes() + b"/test/wk-smoke.mjs\0")
        app = self.tree.bundle("release")
        status, _, err = self.check("--profile", "release", "--bundle", str(app))
        self.assertEqual(status, 1)
        self.assertIn("embeds the development frontend (/test/wk-smoke.mjs)", err)
        # the same question before anything is packaged
        self.tree.place("release")
        self.assertEqual(self.check("--profile", "release", "--binaries")[0], 0)
        status, _, err = self.check("--profile", "release", "--binaries", "--main-product", str(release))
        self.assertEqual(status, 1)
        self.assertIn("embeds the development frontend", err)
        # a debug executable without the smoke carrier cannot run a smoke
        debug = self.tree.tauri / "target" / "debug" / "deck-app"
        debug.write_bytes(image(uuid(901), PLAIN, tail=b"\0/index.html\0"))
        app = self.tree.bundle("debug")
        status, _, err = self.check("--profile", "debug", "--bundle", str(app))
        self.assertEqual(status, 1)
        self.assertIn("embeds no ui/test assets", err)

    def test_a_named_product_is_checked_with_the_placement(self):
        self.tree.place("debug")
        product = self.tree.tauri / "target" / "debug" / "deck-app"
        status, out, err = self.check("--profile", "debug", "--binaries", "--main-product", str(product))
        self.assertEqual(status, 0, err)
        self.assertIn("5 files", out)
        product.unlink()
        status, _, err = self.check("--profile", "debug", "--binaries", "--main-product", str(product))
        self.assertEqual(status, 1)
        self.assertIn("deck-app:", err)

    def test_two_candidate_executable_products_need_a_named_one(self):
        app = self.tree.bundle("release")
        native = self.tree.tauri / "target" / TRIPLE / "release" / "deck-app"
        native.parent.mkdir(parents=True)
        native.write_bytes(image(uuid(950), PLAIN))
        status, _, err = self.check("--profile", "release", "--bundle", str(app))
        self.assertEqual(status, 1)
        self.assertIn("--main-product", err)
        plain = str(self.tree.tauri / "target" / "release" / "deck-app")
        self.assertEqual(self.check("--profile", "release", "--bundle", str(app), "--main-product", plain)[0], 0)
        self.assertEqual(self.check("--profile", "release", "--bundle", str(app), "--main-product", str(native))[0], 1)

    def test_a_packaged_tmux_must_be_the_committed_file(self):
        app = self.tree.bundle("debug")
        (app / "Contents" / "MacOS" / "tmux").write_bytes(image(uuid(999), PLAIN))
        status, _, err = self.check("--profile", "debug", "--bundle", str(app))
        self.assertEqual(status, 1)
        self.assertIn("tmux:", err)

    def test_a_directory_that_is_not_the_app_tree_asks_nothing(self):
        with tempfile.TemporaryDirectory() as empty:
            self.assertEqual(run("check", "--tauri", empty, "--profile", "debug", "--binaries")[0], 2)
        app = self.tree.bundle("debug")
        (app / "Contents" / "Info.plist").unlink()
        self.assertEqual(self.check("--profile", "debug", "--bundle", str(app))[0], 2)


class StagedTests(unittest.TestCase):
    """`staged` judges a staged frontend by the contract build.rs stages by."""

    CONTRACT = json.loads((ROOT / "app/src-tauri/tests/fixtures/staging-contract.json").read_text())

    def setUp(self):
        directory = tempfile.TemporaryDirectory(prefix="deck-staged-")
        self.addCleanup(directory.cleanup)
        self.root = pathlib.Path(directory.name)
        self.source = self.root / "ui"
        for name in self.CONTRACT["source"]:
            self.write(self.source / name, name)

    @staticmethod
    def write(path, text):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def staging(self, profile):
        dist = self.root / f"ui-dist-{profile}"
        for name in self.CONTRACT[profile]:
            self.write(dist / name, name)
        return dist

    def test_the_contract_fixture_is_accepted_for_each_profile(self):
        # the Rust test of the same fixture stages exactly these lists
        for profile in ("debug", "release"):
            self.assertEqual(sorted(MODULE.staged_files(self.source, profile)), self.CONTRACT[profile])
            self.assertEqual(MODULE.staging_problems(self.source, self.staging(profile), profile), [])
            status, out, _ = run("staged", "--profile", profile, "--source", str(self.source),
                                 "--dist", str(self.root / f"ui-dist-{profile}"))
            self.assertEqual(status, 0)
            self.assertIn(f"{len(self.CONTRACT[profile])} files", out)

    def test_dot_entries_of_the_source_are_never_asked_for(self):
        # .DS_Store, a tool directory, an editor swap file and a nested hidden
        # directory are in the source; none is staged and none is missed
        dist = self.staging("debug")
        self.write(self.source / "js/.hidden/deeper/.more/x.js", "more")
        self.write(self.source / ".DS_Store", "changed by Finder")
        self.assertEqual(MODULE.staging_problems(self.source, dist, "debug"), [])

    def test_a_missing_or_changed_ordinary_file_is_reported(self):
        dist = self.staging("debug")
        (dist / "index.html").unlink()
        (dist / "js/app.js").write_text("older bytes")
        self.write(self.source / "js/new.js", "added to the source after the staging")
        self.assertEqual(sorted(MODULE.staging_problems(self.source, dist, "debug")), [
            "missing from the staged frontend: index.html",
            "missing from the staged frontend: js/new.js",
            "staged with other bytes than the source: js/app.js",
        ])
        status, _, err = run("staged", "--profile", "debug", "--source", str(self.source), "--dist", str(dist))
        self.assertEqual(status, 1)
        self.assertIn("3 difference(s)", err)

    def test_what_does_not_belong_in_the_staging_is_reported(self):
        dist = self.staging("debug")
        self.write(dist / ".DS_Store", "finder")
        self.write(dist / "js/.hidden/inner.js", "a hidden directory an older build staged")
        self.write(dist / "vendor/.cache/deep/blob.bin", "nested")
        self.write(dist / "js/removed.js", "deleted from the source since")
        self.assertEqual(sorted(MODULE.staging_problems(self.source, dist, "debug")), [
            "staged but not part of the frontend (a dot entry): .DS_Store",
            "staged but not part of the frontend (a dot entry): js/.hidden/inner.js",
            "staged but not part of the frontend (a dot entry): vendor/.cache/deep/blob.bin",
            "staged but not part of the frontend (not in the source tree): js/removed.js",
        ])

    def test_a_release_staging_holds_no_test_tree_and_a_debug_one_must(self):
        debug = self.staging("debug")
        problems = MODULE.staging_problems(self.source, debug, "release")
        self.assertEqual(sorted(problems), [
            "staged but not part of the frontend (ui/test in a release staging): test/fixtures/board.json",
            "staged but not part of the frontend (ui/test in a release staging): test/wk-smoke.mjs",
        ])
        release = self.staging("release")
        self.assertEqual(sorted(MODULE.staging_problems(self.source, release, "debug")), [
            "missing from the staged frontend: test/fixtures/board.json",
            "missing from the staged frontend: test/wk-smoke.mjs",
        ])
        # a nested directory that happens to be called test is ordinary frontend
        self.assertIn("js/test/kept.js", MODULE.staged_files(self.source, "release"))

    def test_a_tree_that_is_not_there_asks_nothing(self):
        status, _, _ = run("staged", "--profile", "debug", "--source", str(self.source), "--dist", str(self.root / "absent"))
        self.assertEqual(status, 2)


class PackagedTests(unittest.TestCase):
    """What a package RUNS is what was copied, which a check of binaries/ made
    earlier cannot know. The negative controls replace a shared sidecar after
    that check passed and the finished package must still be refused."""

    def setUp(self):
        directory = tempfile.TemporaryDirectory(prefix="deck-packaged-")
        self.addCleanup(directory.cleanup)
        self.tree = Tree(directory.name)
        self.tauri = self.tree.tauri
        self.tree.place("debug")
        # the staged frontend bundle.sh also checks: one file, staged
        for directory in (self.tauri.parent / "ui", self.tauri / "ui-dist"):
            directory.mkdir()
            (directory / "index.html").write_text("frontend")
        # the Pi observer's files, which bundle.sh copies beside the sidecars
        self.pi = self.tauri / "resources/pi"
        self.pi.mkdir(parents=True)
        for name in PI_RESOURCES:
            (self.pi / name).write_text(f"// {name}\n")

    def step(self, function, *arguments):
        """One function of app/bundle.sh, as app/run.sh calls it."""
        script = f'. "$0/app/bundle.sh" && {function} "$@"'
        return subprocess.run(["sh", "-c", script, str(ROOT), *map(str, arguments)], capture_output=True, text=True)

    def finish(self, app):
        """What app/run.sh does between the copy and the last check, as far as the check reads it."""
        with open(app / "Contents" / "Info.plist", "wb") as handle:
            plistlib.dump({"CFBundleExecutable": "deck"}, handle)

    def foreign(self, binary, context):
        (self.tauri / "binaries" / f"{binary}-{TRIPLE}").write_bytes(self.tree.product(binary, context).read_bytes())

    def test_the_dev_bundle_passes_when_nothing_changes_in_between(self):
        app = self.tauri / "target/debug/deck-dev.app"
        self.assertEqual(self.step("deck_bundle_precheck", ROOT, self.tauri).returncode, 0)
        self.assertEqual(self.step("deck_bundle_copy", self.tauri, app).returncode, 0)
        self.finish(app)
        done = self.step("deck_bundle_postcheck", ROOT, self.tauri, app)
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertIn("6 files", done.stdout)
        self.assertEqual(sorted(path.name for path in (app / "Contents/MacOS").iterdir()),
                         ["deck", "deck-lifecycle-helper", "deck-mcp", "deck-mcp-runner", "deck-status-helper", "tmux"])

    def test_the_dev_bundle_carries_the_pi_observers_files_and_nothing_else_there(self):
        app = self.tauri / "target/debug/deck-dev.app"
        self.assertEqual(self.step("deck_bundle_copy", self.tauri, app).returncode, 0)
        self.finish(app)
        carried = app / "Contents/Resources/pi"
        self.assertEqual(sorted(path.name for path in carried.iterdir()), sorted(PI_RESOURCES))
        for name in PI_RESOURCES:
            self.assertEqual((carried / name).read_bytes(), (self.pi / name).read_bytes())
        self.assertEqual(self.step("deck_bundle_postcheck", ROOT, self.tauri, app).returncode, 0)
        # the checkout's file changed after the copy: the bundle is not this checkout's
        (self.pi / PI_RESOURCES[0]).write_text("// edited\n")
        stale = self.step("deck_bundle_postcheck", ROOT, self.tauri, app)
        self.assertEqual(stale.returncode, 1)
        self.assertIn(f"does not hold this checkout's resources/pi/{PI_RESOURCES[0]}", stale.stderr)
        # copied again it passes; anything else in that directory is refused
        self.assertEqual(self.step("deck_bundle_copy", self.tauri, app).returncode, 0)
        self.finish(app)
        self.assertEqual(self.step("deck_bundle_postcheck", ROOT, self.tauri, app).returncode, 0)
        (carried / "fixture.ts").write_text("// a test fixture has no place in a bundle\n")
        extra = self.step("deck_bundle_postcheck", ROOT, self.tauri, app)
        self.assertEqual(extra.returncode, 1)
        self.assertIn("holds something other than the Pi observer's files", extra.stderr)
        # a copy removes what does not belong there
        self.assertEqual(self.step("deck_bundle_copy", self.tauri, app).returncode, 0)
        self.assertEqual(sorted(path.name for path in carried.iterdir()), sorted(PI_RESOURCES))
        # a checkout without one of the files builds no bundle
        (self.pi / PI_RESOURCES[1]).unlink()
        self.assertEqual(self.step("deck_bundle_copy", self.tauri, app).returncode, 1)

    def test_the_bundle_script_names_the_files_the_checkout_ships(self):
        text = (ROOT / "app/bundle.sh").read_text()
        self.assertIn(f'DECK_BUNDLE_PI_RESOURCES="{" ".join(PI_RESOURCES)}"', text)
        self.assertEqual(sorted(path.name for path in (ROOT / "app/src-tauri/resources/pi").iterdir()), sorted(PI_RESOURCES))

    def test_a_sidecar_replaced_after_the_first_check_is_refused_in_the_dev_bundle(self):
        for binary, context, said in (("deck-mcp", "coverage", "is instrumented, expected plain"),
                                      ("deck-lifecycle-helper", "release", "is not this checkout's build product")):
            with self.subTest(binary=binary, context=context):
                self.tree.place("debug")
                app = self.tauri / f"target/debug/deck-dev-{context}.app"
                first = self.step("deck_bundle_precheck", ROOT, self.tauri)
                self.assertEqual(first.returncode, 0, first.stderr)
                # another build context writes the shared directory now
                self.foreign(binary, context)
                self.assertEqual(self.step("deck_bundle_copy", self.tauri, app).returncode, 0)
                self.finish(app)
                last = self.step("deck_bundle_postcheck", ROOT, self.tauri, app)
                self.assertEqual(last.returncode, 1)
                self.assertIn(f"{binary}: ", last.stderr)
                self.assertIn(said, last.stderr)
                self.assertIn("sidecar provenance FAILED", last.stderr)

    def test_a_frontend_staged_by_another_profile_stops_the_dev_bundle_before_the_copy(self):
        (self.tauri / "ui-dist/.DS_Store").write_text("finder")
        first = self.step("deck_bundle_precheck", ROOT, self.tauri)
        self.assertEqual(first.returncode, 1)
        self.assertIn("a dot entry", first.stderr)

    def carrier(self, names):
        """A test carrier holding tmux, the executable and only `names` of the four sidecars, copied from binaries/."""
        app = self.tauri.parent / "carrier.app"
        macos = app / "Contents" / "MacOS"
        macos.mkdir(parents=True, exist_ok=True)
        for name in ("tmux", *names):
            (macos / name).write_bytes((self.tauri / "binaries" / f"{name}-{TRIPLE}").read_bytes())
        (macos / "deck").write_bytes((self.tauri / "target/debug/deck-app").read_bytes())
        self.finish(app)
        return app

    def check(self, app, sidecars):
        return run("check", "--tauri", str(self.tauri), "--profile", "debug", "--bundle", str(app),
                   "--sidecars", sidecars, "--main-product", str(self.tauri / "target/debug/deck-app"))

    def test_a_carrier_with_two_sidecars_is_checked_for_exactly_what_it_carries(self):
        two = ("deck-status-helper", "deck-lifecycle-helper")
        app = self.carrier(two)
        status, out, err = self.check(app, ",".join(two))
        self.assertEqual(status, 0, err)
        self.assertIn("4 files", out)  # two sidecars, tmux, the executable
        # without the option the same carrier is incomplete: nothing is skipped by default
        self.assertEqual(run("check", "--tauri", str(self.tauri), "--profile", "debug", "--bundle", str(app),
                             "--main-product", str(self.tauri / "target/debug/deck-app"))[0], 1)
        # a sidecar it was asked to carry is missing
        (app / "Contents/MacOS/deck-lifecycle-helper").unlink()
        self.assertEqual(self.check(app, ",".join(two))[0], 1)

    def test_a_sidecar_nobody_named_is_still_checked_when_it_is_in_the_carrier(self):
        two = ("deck-status-helper", "deck-lifecycle-helper")
        app = self.carrier(two)
        (app / "Contents/MacOS/deck-mcp").write_bytes(self.tree.product("deck-mcp", "coverage").read_bytes())
        status, _, err = self.check(app, ",".join(two))
        self.assertEqual(status, 1)
        self.assertIn("deck-mcp: ", err)

    def test_a_sidecar_replaced_after_the_first_check_is_refused_in_the_carrier(self):
        two = ("deck-status-helper", "deck-lifecycle-helper")
        self.assertEqual(run("check", "--tauri", str(self.tauri), "--profile", "debug", "--binaries",
                             "--main-product", str(self.tauri / "target/debug/deck-app"))[0], 0)
        self.foreign("deck-status-helper", "coverage")
        status, _, err = self.check(self.carrier(two), ",".join(two))
        self.assertEqual(status, 1)
        self.assertIn("deck-status-helper: ", err)
        self.assertIn("is instrumented, expected plain", err)

    def test_the_component_option_is_for_packages_and_known_names(self):
        app = self.carrier(("deck-status-helper",))
        self.assertEqual(self.check(app, "deck-status-helper,deck-unknown")[0], 2)
        self.assertEqual(run("check", "--tauri", str(self.tauri), "--profile", "debug", "--binaries",
                             "--sidecars", "deck-status-helper")[0], 2)


class RepositoryTests(unittest.TestCase):
    def test_the_table_is_build_rs_and_the_tauri_configuration(self):
        build = (ROOT / "app/src-tauri/build.rs").read_text()
        declared = {
            binary: crate
            for crate, manifest, binary, target in re.findall(
                r'\(\s*"([\w-]+)",\s*"([\w-]+)/Cargo\.toml",\s*"([\w-]+)",\s*"([\w-]+)/target",\s*\)', build
            )
            if crate == manifest == target
        }
        self.assertEqual(declared, BUILT)
        self.assertEqual(dict(MODULE.SIDECARS), BUILT)
        external = json.loads((ROOT / "app/src-tauri/tauri.conf.json").read_text())["bundle"]["externalBin"]
        self.assertEqual(
            sorted(external),
            sorted(f"binaries/{name}" for name in (*BUILT, *MODULE.PREBUILT)),
        )
        self.assertEqual(MODULE.PREBUILT, ("tmux",))
        # the asset names the frontend check looks for are real files of ui/test
        self.assertEqual(MODULE.TEST_ASSETS, (b"/test/static.test.mjs", b"/test/wk-smoke.mjs"))
        for name in MODULE.TEST_ASSETS:
            self.assertTrue((ROOT / "app/ui" / name.decode().lstrip("/")).is_file(), name)

    def test_every_packaging_path_checks_before_it_copies_and_again_before_it_runs(self):
        """A package is only as right as the last placement in binaries/, and what
        runs is what was copied: every path checks the shared outputs first and the
        finished package again before anything opens or ships it."""
        tool = "scripts/sidecar_provenance.py"
        steps = (ROOT / "app/bundle.sh").read_text()
        self.assertIn(f'"$1/{tool}" check --profile debug --binaries --tauri "$2"', steps)
        self.assertIn(f'"$1/{tool}" staged --profile debug --tauri "$2"', steps)
        self.assertIn(f'"$1/{tool}" check --profile debug --bundle "$3" --tauri "$2"', steps)
        run_sh = (ROOT / "app/run.sh").read_text()
        self.assertNotIn("cp binaries/", run_sh, "sidecars are copied by bundle.sh only")
        self.assertNotIn("cp target/debug/deck-app", run_sh)
        order = [run_sh.index(mark) for mark in (
            '. "$APP_DIR/bundle.sh"', "\ncargo build\n", 'deck_bundle_precheck "$REPO_ROOT" "$PWD"',
            'deck_bundle_copy "$PWD" "$APP"', "codesign --force --sign - --entitlements",
            'if ! deck_bundle_postcheck "$REPO_ROOT" "$PWD" "$APP"; then', "  exit 1\nfi\n")]
        self.assertEqual(order, sorted(order))
        opens = [match.start() for match in re.finditer(r"(?m)^\s*open ", run_sh)]
        self.assertTrue(opens and min(opens) > order[-1], "nothing opens the bundle before the last check")
        compat = (ROOT / "scripts/package-compat-candidate").read_text()
        check = compat.index(f'"$ROOT/{tool}" check --profile release --binaries')
        self.assertLess(check, compat.index('cp "$source"'))
        self.assertLess(compat.index("codesign --force"), compat.index(f'"$ROOT/{tool}" check --profile release --bundle "$APP"'))
        lens = (ROOT / "scripts/translation-lens-verify.py").read_text()
        order = [lens.index(mark) for mark in (
            '"staged", "--profile", "debug"', '"check", "--profile", "debug", "--binaries"',
            'TAURI / f"binaries/{name}-aarch64-apple-darwin"', '"codesign", "--force"',
            '"check", "--profile", "debug", "--bundle", str(bundle), "--sidecars", ",".join(BUNDLE_SIDECARS)',
            'report["bundlePath"] = str(bundle)')]
        self.assertEqual(order, sorted(order))
        self.assertIn('BUNDLE_SIDECARS = ("deck-status-helper", "deck-lifecycle-helper")', lens)
        self.assertNotIn("rglob", lens[lens.index("def build_bundle"):lens.index("def processes")])
        setup = (ROOT / "scripts/translation-setup-verify.py").read_text()
        self.assertIn("'staged', '--profile', 'debug'", setup)
        self.assertNotIn("rglob('*')", setup)
        # each workflow names the product of the cargo target it builds for
        for name, product in (
            ("nightly.yml", "app/src-tauri/target/aarch64-apple-darwin/release/deck-app"),
            ("release.yml", "app/src-tauri/target/release/deck-app"),
        ):
            workflow = (ROOT / ".github/workflows" / name).read_text()
            self.assertIn(product.replace("deck-app", "bundle/macos/deck.app"), workflow, name)
            self.assertIn(
                f'python3 {tool} check --profile release --bundle "$app_path" --main-product {product}',
                workflow,
                name,
            )
        gate = (ROOT / ".github/workflows/gate.yml").read_text()
        coverage = gate.index("run: cargo llvm-cov --workspace")
        self.assertLess(coverage, gate.index(f"python3 {tool} check --profile debug --coverage --binaries"))

if __name__ == "__main__":
    unittest.main()
