from __future__ import annotations

import base64
import importlib.util
import io
import json
import os
import plistlib
import re
import runpy
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
rv = runpy.run_path(str(ROOT / "scripts" / "release-version"))
spec = importlib.util.spec_from_file_location("release_channels", ROOT / "scripts" / "release_channels.py")
assert spec and spec.loader
rc = importlib.util.module_from_spec(spec)
spec.loader.exec_module(rc)
translation_binary = runpy.run_path(str(ROOT / "scripts" / "check_local_translation_binary.py"))
stapler = runpy.run_path(str(ROOT / "scripts" / "stapler-ticket"))


class StaplerValidationTests(unittest.TestCase):
    disconnect = (
        'Error Domain=NSURLErrorDomain Code=-1005 "The network connection was lost." '
        'https://api.apple-cloudkit.com/database/1/com.apple.gk.ticket-delivery/production/public/records/lookup'
    )

    def validate(self, results, operation="validate"):
        output, errors = io.StringIO(), io.StringIO()
        with patch("subprocess.run", side_effect=results) as run, patch("time.sleep") as sleep:
            with redirect_stdout(output), redirect_stderr(errors):
                status = stapler["ticket"](operation, "/tmp/Deck test.app")
        return status, run, sleep, output.getvalue() + errors.getvalue()

    def result(self, code=0, error=""):
        return subprocess.CompletedProcess([], code, "validation output\n", error)

    def test_success_needs_no_retry_and_preserves_command_boundary(self):
        status, run, sleep, output = self.validate([self.result()])
        self.assertEqual(status, 0)
        self.assertEqual(run.call_args.args[0], ["/usr/bin/xcrun", "stapler", "validate",
                                                str(Path("/tmp/Deck test.app").resolve())])
        self.assertEqual(run.call_args.kwargs["timeout"], 60)
        sleep.assert_not_called()
        self.assertIn("validation output", output)

    def test_disconnect_can_recover_only_after_successful_validation(self):
        status, run, sleep, output = self.validate([self.result(68, self.disconnect), self.result()])
        self.assertEqual(status, 0)
        self.assertEqual(run.call_count, 2)
        sleep.assert_called_once_with(3)
        self.assertIn(self.disconnect, output)

    def test_persistent_disconnect_fails_with_bounded_attempts(self):
        status, run, sleep, _ = self.validate([self.result(68, self.disconnect)] * 4)
        self.assertEqual(status, 68)
        self.assertEqual(run.call_count, 4)
        self.assertEqual([c.args[0] for c in sleep.call_args_list], [3, 6, 12])

    def test_unknown_errors_invalid_tickets_and_signals_fail_without_retry(self):
        for result in (self.result(68, "invalid ticket"), self.result(65, self.disconnect),
                       self.result(68, self.disconnect.replace("-1005", "-1200")),
                       self.result(68, self.disconnect.replace("api.apple-cloudkit.com", "example.com")),
                       self.result(-9)):
            with self.subTest(result=result):
                status, run, sleep, _ = self.validate([result])
                self.assertNotEqual(status, 0)
                self.assertEqual(run.call_count, 1)
                sleep.assert_not_called()

    def test_invalid_ticket_after_disconnect_is_not_retried(self):
        status, run, sleep, _ = self.validate([self.result(68, self.disconnect), self.result(65, "invalid")])
        self.assertEqual(status, 65)
        self.assertEqual(run.call_count, 2)
        sleep.assert_called_once_with(3)

    def test_timeout_is_a_terminal_failure(self):
        status, run, sleep, _ = self.validate([subprocess.TimeoutExpired("xcrun", 60)])
        self.assertEqual(status, 124)
        self.assertEqual(run.call_count, 1)
        sleep.assert_not_called()

    def test_staple_uses_the_same_closed_retry_budget_and_requires_success(self):
        cases = [
            ([self.result()], 0, 1),
            ([self.result(68, self.disconnect), self.result()], 0, 2),
            ([self.result(68, self.disconnect)] * 4, 68, 4),
            ([self.result(68, "invalid ticket")], 68, 1),
            ([subprocess.TimeoutExpired("xcrun", 60)], 124, 1),
        ]
        for results, expected, attempts in cases:
            with self.subTest(expected=expected, attempts=attempts):
                status, run, sleep, _ = self.validate(results, operation="staple")
                self.assertEqual(status, expected)
                self.assertEqual(run.call_count, attempts)
                self.assertEqual(run.call_args.args[0][2], "staple")
                self.assertEqual(sleep.call_count, attempts - 1)

    def test_no_other_stapler_operation_can_run(self):
        with patch("subprocess.run") as run:
            with self.assertRaises(ValueError):
                stapler["ticket"]("--help", "/tmp/Deck test.app")
        run.assert_not_called()

    def test_all_release_validation_paths_use_the_closed_wrapper(self):
        for name in ("nightly.yml", "release.yml", "promote.yml"):
            text = (ROOT / ".github/workflows" / name).read_text()
            self.assertNotIn("xcrun stapler validate", text)
            self.assertNotIn("xcrun stapler staple", text)
            self.assertIn("scripts/stapler-ticket validate", text)
            self.assertIn("spctl -a -vv", text)
        helper = (ROOT / "scripts/verify-tunnel-helper").read_text()
        self.assertIn('/stapler-ticket" validate "$app"', helper)
        self.assertIn('spctl -a -vv -t exec "$app"', helper)
        package = (ROOT / "scripts/package-tunnel-helper").read_text()
        self.assertIn('scripts/stapler-ticket staple "$app"', package)
        self.assertNotIn("xcrun stapler staple", package)
        # the app's missing-ticket fallback: validated first, stapled through
        # the wrapper only then, and the updater archive re-signed and read back
        nightly = (ROOT / ".github/workflows/nightly.yml").read_text()
        fallback = nightly.index('if ! scripts/stapler-ticket validate "$app_path"; then')
        block = nightly[fallback:nightly.index("spctl -a -vv -t exec", fallback)]
        for line in (
            'scripts/stapler-ticket staple "$app_path"',
            "tauri signer sign candidate/deck_aarch64.app.tar.gz",
            "tar -xzf candidate/deck_aarch64.app.tar.gz -C unpacked",
            'scripts/stapler-ticket validate "$app_path"',
        ):
            self.assertIn(line, block)
        self.assertNotIn("xcrun stapler", block)


def script_tests_the_gate_misses(gate: str, scripts: Path) -> list[str]:
    """Test files beside the tools that no gate step runs.

    A Python test module (`scripts/test_*.py`) must be an argument of the one
    `python3 -m unittest` step, and a test program (`scripts/test-*`) must
    have a `run:` step of its own. A file on disk that the gate does not name
    is a test nobody runs; a name the gate lists that is not on disk is a
    step that fails for the wrong reason.
    """
    commands = re.findall(r"(?m)^\s+run: python3 -m unittest (.+)$", gate)
    if len(commands) != 1:
        return [f"expected one unittest step, found {len(commands)}"]
    listed = commands[0].split()
    on_disk = sorted(f"scripts/{path.name}" for path in scripts.glob("test_*.py"))
    problems = [f"{name} is not in the unittest step" for name in on_disk if name not in listed]
    problems += [f"{name} is listed but does not exist" for name in listed if name not in on_disk]
    problems += [f"{name} is listed more than once" for name in sorted(set(listed)) if listed.count(name) > 1]
    for path in sorted(scripts.glob("test-*")):
        name = f"scripts/{path.name}"
        if not re.search(rf"(?m)^\s+run: {re.escape(name)}(?:\s|$)", gate):
            problems.append(f"{name} has no run step")
    return problems


class VersionToolTests(unittest.TestCase):
    def fixture(self, tauri: str = "0.4.37", cargo: str = "0.4.37", lock: str = "0.4.37", adapter: str | None = None) -> Path:
        root = Path(tempfile.mkdtemp(prefix="deck-version-test-"))
        source = root / "app" / "src-tauri"
        source.mkdir(parents=True)
        (source / "tauri.conf.json").write_text(json.dumps({"version": tauri}, indent=2) + "\n")
        (source / "Cargo.toml").write_text(f'[package]\nname = "deck-app"\nversion = "{cargo}"\n')
        (source / "mcp-adapter").mkdir()
        (source / "mcp-adapter" / "Cargo.toml").write_text(
            f'[package]\nname = "deck-mcp"\nversion = "{adapter or cargo}"\n'
        )
        (source / "Cargo.lock").write_text(
            f'[[package]]\nname = "deck-app"\nversion = "{lock}"\n\n'
            f'[[package]]\nname = "deck-mcp"\nversion = "{adapter or lock}"\n\n'
            '[[package]]\nname = "other"\nversion = "9.9.9"\n'
        )
        return root

    def test_strict_versions(self) -> None:
        for value in ("0.4.37", "1.0.0", "12.34.567"):
            self.assertEqual(rv["parse_version"](value), tuple(map(int, value.split("."))))
        for value in ("1.2", "1.2.3.4", "01.2.3", "1.2.3-nightly.1", "1.2.3+sha"):
            with self.assertRaises(rv["VersionError"]):
                rv["parse_version"](value)

    def test_source_mismatch_fails(self) -> None:
        root = self.fixture(cargo="0.4.38")
        with self.assertRaises(rv["VersionError"]):
            rv["assert_consistent"](root)

        root = self.fixture(adapter="0.4.36")
        with self.assertRaises(rv["VersionError"]):
            rv["assert_consistent"](root)

    def test_not_strictly_newer_fails(self) -> None:
        with self.assertRaises(rv["VersionError"]):
            rv["assert_newer"]("0.4.37", ["0.4.36", "0.4.37"])
        rv["assert_newer"]("0.4.38", ["0.4.36", "0.4.37"])

    def test_set_updates_only_deck_versions(self) -> None:
        root = self.fixture("0.4.36", "0.4.36", "0.4.36")
        before_other = '[[package]]\nname = "other"\nversion = "9.9.9"\n'
        rv["set_version"](root, "0.4.37")
        self.assertEqual(rv["assert_consistent"](root), "0.4.37")
        self.assertIn(before_other, (root / "app/src-tauri/Cargo.lock").read_text())

    def test_cli_check_set_and_failure_exit_codes(self) -> None:
        root = self.fixture("0.4.36", "0.4.36", "0.4.36")

        def invoke(*args: str) -> tuple[int, str, str]:
            out, err = io.StringIO(), io.StringIO()
            with patch.object(sys, "argv", ["release-version", *args]), redirect_stdout(out), redirect_stderr(err):
                code = rv["main"]()
            return code, out.getvalue(), err.getvalue()

        code, out, err = invoke("--root", str(root), "check", "--skip-monotonic")
        self.assertEqual((code, out.strip(), err), (0, "0.4.36", ""))
        code, out, err = invoke("--root", str(root), "set", "0.4.37", "--skip-monotonic")
        self.assertEqual((code, out.strip(), err), (0, "0.4.37", ""))
        code, out, err = invoke("--root", str(root), "check", "--expected", "9.9.9", "--skip-monotonic")
        self.assertEqual(code, 2)
        self.assertEqual(out, "")
        self.assertIn("does not equal requested", err)


class ReleaseChannelTests(unittest.TestCase):
    version = "0.4.37"
    sha = "a" * 40
    tag = "nightly-v0.4.37-20260829-aaaaaaa"

    def candidate_fixture(self) -> tuple[Path, dict[str, object]]:
        directory = Path(tempfile.mkdtemp(prefix="deck-candidate-test-"))
        signature = base64.b64encode(
            b"untrusted comment: signature from tauri secret key\nRUSAMPLE\n"
        ).decode()
        (directory / rc.ARCHIVE).write_bytes(b"archive")
        (directory / rc.SIGNATURE).write_text(signature + "\n")
        dmg = f"deck_{self.version}_aarch64.dmg"
        (directory / dmg).write_bytes(b"dmg")
        candidate = rc.manifest(self.version, self.tag, signature, "notes", "2026-08-29T00:00:00Z")
        (directory / "candidate.json").write_text(json.dumps(candidate))
        artifacts = [
            {"kind": "dmg", **rc.asset_record(directory / dmg)},
            {"kind": "archive", **rc.asset_record(directory / rc.ARCHIVE)},
            {"kind": "signature", **rc.asset_record(directory / rc.SIGNATURE)},
            {"kind": "manifest", **rc.asset_record(directory / "candidate.json")},
        ]
        rc.write_sums(directory, [str(item["name"]) for item in artifacts], directory / "SHA256SUMS")
        provenance: dict[str, object] = {
            "schema": 2,
            "app_version": self.version,
            "commit": self.sha,
            "candidate_tag": self.tag,
            "workflow": {"run_id": "123", "run_attempt": "1", "built_at": "2026-08-29T00:00:00Z"},
            "bundle_identifier": rc.BUNDLE_ID,
            "updater_target": rc.TARGET,
            "updater_key_epoch": "nightly-v1",
            "test_gate": "passed",
            "signing": {"team_id": "Y8ZG3D692W", "identity": "Developer ID Application"},
            "verification": {
                "codesign": "passed", "notarization": "passed",
                "stapler": "passed", "gatekeeper": "passed",
            },
            "artifacts": artifacts,
        }
        (directory / "provenance.json").write_text(json.dumps(provenance))
        return directory, provenance

    def test_candidate_tag_commit_and_provenance_match(self) -> None:
        self.assertEqual(rc.candidate_tag(self.version, "20260829", self.sha), self.tag)
        directory, provenance = self.candidate_fixture()
        self.assertEqual(rc.verify_provenance(provenance, self.tag, self.sha, directory), self.version)
        bad = dict(provenance, commit="b" * 40)
        with self.assertRaises(rc.ReleaseError):
            rc.verify_provenance(bad, self.tag, self.sha, directory)
        bad_key = dict(provenance, updater_key_epoch="unknown")
        with self.assertRaises(rc.ReleaseError):
            rc.verify_provenance(bad_key, self.tag, self.sha, directory)

    def test_missing_asset_and_wrong_hash_fail(self) -> None:
        directory, provenance = self.candidate_fixture()
        (directory / rc.ARCHIVE).unlink()
        with self.assertRaises(rc.ReleaseError):
            rc.verify_provenance(provenance, self.tag, self.sha, directory)
        directory, provenance = self.candidate_fixture()
        (directory / rc.ARCHIVE).write_bytes(b"tampered")
        with self.assertRaises(rc.ReleaseError):
            rc.verify_provenance(provenance, self.tag, self.sha, directory)

    def test_helper_archive_is_required_for_new_candidate_schema(self) -> None:
        directory, provenance = self.candidate_fixture()
        helper_name = f"Deck_Tunnel_Helper_nightly_{self.version}_{self.sha[:7]}_macOS.zip"
        (directory / helper_name).write_bytes(b"published helper archive")
        helper = {
            "archive": helper_name, "source_commit": self.sha,
            "version": "0.1.0", "binary_sha256": "a" * 64,
            "architecture": "arm64", "protocol_version": 1,
            "team_id": "Y8ZG3D692W", "signing_identifier": "io.c9r.deck-tunnelctl",
            "verification": {name: "passed" for name in
                             ("codesign", "notarization", "stapler", "gatekeeper", "extracted_archive")},
        }
        provenance["schema"] = 3
        provenance["helper"] = helper
        provenance["artifacts"].append({"kind": rc.HELPER_KIND, **rc.asset_record(directory / helper_name)})
        rc.write_sums(directory, [str(item["name"]) for item in provenance["artifacts"]], directory / "SHA256SUMS")
        self.assertEqual(rc.verify_provenance(provenance, self.tag, self.sha, directory), self.version)
        (directory / helper_name).unlink()
        with self.assertRaises(rc.ReleaseError):
            rc.verify_provenance(provenance, self.tag, self.sha, directory)
        (directory / helper_name).write_bytes(b"published helper archive")
        helper["source_commit"] = "b" * 40
        with self.assertRaises(rc.ReleaseError):
            rc.verify_provenance(provenance, self.tag, self.sha, directory)

    def test_signature_and_manifest_fields_fail_closed(self) -> None:
        directory, _ = self.candidate_fixture()
        signature = rc.read_signature(directory / rc.SIGNATURE)
        valid = json.loads((directory / "candidate.json").read_text())
        rc.verify_manifest(valid, self.version, self.tag, signature)
        for mutation in (
            lambda value: value.update(version="0.4.38"),
            lambda value: value["platforms"].pop(rc.TARGET),
            lambda value: value["platforms"][rc.TARGET].update(url="https://example.com/app.tar.gz"),
            lambda value: value["platforms"][rc.TARGET].update(signature="wrong"),
        ):
            changed = json.loads(json.dumps(valid))
            mutation(changed)
            with self.assertRaises(rc.ReleaseError):
                rc.verify_manifest(changed, self.version, self.tag, signature)
        (directory / rc.SIGNATURE).write_text("not-base64")
        with self.assertRaises(rc.ReleaseError):
            rc.read_signature(directory / rc.SIGNATURE)

    def test_release_state_and_stable_conflicts(self) -> None:
        rc.verify_release_metadata(
            {
                "tagName": self.tag, "isDraft": False, "isPrerelease": True,
                "targetCommitish": self.sha,
            }, self.tag, True, self.sha
        )
        with self.assertRaises(rc.ReleaseError):
            rc.verify_release_metadata(
                {
                    "tagName": self.tag, "isDraft": False, "isPrerelease": False,
                    "targetCommitish": self.sha,
                }, self.tag, True, self.sha
            )
        with self.assertRaises(rc.ReleaseError):
            rc.verify_release_metadata(
                {
                    "tagName": self.tag, "isDraft": False, "isPrerelease": True,
                    "targetCommitish": "b" * 40,
                }, self.tag, True, self.sha
            )
        with self.assertRaises(rc.ReleaseError):
            rc.assert_no_stable_conflict(self.version, [f"v{self.version}"], [])
        with self.assertRaises(rc.ReleaseError):
            rc.assert_no_stable_conflict(self.version, [], [f"v{self.version}"])

    def test_promotion_workflow_build_commands_are_forbidden(self) -> None:
        path = Path(tempfile.mkdtemp(prefix="deck-promotion-test-")) / "promote.yml"
        path.write_text("run: gh release download\n")
        rc.assert_promotion_has_no_build_commands(path)
        path.write_text("run: cargo build --release\n")
        with self.assertRaises(rc.ReleaseError):
            rc.assert_promotion_has_no_build_commands(path)
        path.write_text("run: brew install minisign\n")
        with self.assertRaises(rc.ReleaseError):
            rc.assert_promotion_has_no_build_commands(path)

    def test_updater_signing_tool_is_pinned_in_one_place(self) -> None:
        installer = (ROOT / "scripts/install-minisign").read_text()
        self.assertRegex(installer, r"(?m)^version=[0-9]+\.[0-9]+$")
        self.assertRegex(installer, r"(?m)^expected=[0-9a-f]{64}$")
        self.assertIn("shasum -a 256 -c", installer)
        for name in ("nightly.yml", "promote.yml"):
            workflow = (ROOT / ".github/workflows" / name).read_text()
            self.assertNotIn("brew install", workflow, name)
            self.assertIn("$(scripts/install-minisign)", workflow, name)
            # every minisign invocation goes through the installed path
            bare = re.search(r"(?m)^\s+(?:spawn\s+)?minisign\s.*$", workflow)
            self.assertIsNone(bare, f"{name}: {bare and bare.group(0).strip()}")
            self.assertNotIn("jedisct1/minisign", workflow, name)

    def test_workflow_gates_fail_closed(self) -> None:
        directory = Path(tempfile.mkdtemp(prefix="deck-gates-test-"))
        path = directory / "build.yml"
        gated = (
            "jobs:\n"
            "  gate:\n"
            "    runs-on: macos-15\n"
            "    steps:\n"
            "      - run: cargo audit --file app/src-tauri/Cargo.lock\n"
            "  build:\n"
            "    needs: [gate]\n"
            "    steps:\n"
            "      - uses: tauri-apps/tauri-action@0000000000000000000000000000000000000000\n"
        )
        path.write_text(gated)
        rc.assert_workflow_gates(path)
        for broken in (
            gated.replace("    needs: [gate]\n", ""),
            gated.replace("cargo audit --file", "cargo test --file"),
            gated.replace("Cargo.lock\n", "Cargo.lock || true\n"),
            gated.replace("    runs-on: macos-15\n", "    runs-on: macos-15\n    continue-on-error: true\n"),
            "jobs:\n  t:\n    steps:\n      - run: cargo test || true\n",
        ):
            path.write_text(broken)
            with self.assertRaises(rc.ReleaseError, msg=broken):
                rc.assert_workflow_gates(path)
        path.write_text("jobs:\n  t:\n    steps:\n      - run: gh release upload x || true\n")
        rc.assert_workflow_gates(path)
        # a build gated through a reusable workflow counts only when that
        # file exists beside it and runs the audit itself
        called = gated.replace(
            "    runs-on: macos-15\n    steps:\n      - run: cargo audit --file app/src-tauri/Cargo.lock\n",
            "    uses: ./.github/workflows/gate.yml\n",
        )
        path.write_text(called)
        with self.assertRaises(rc.ReleaseError):
            rc.assert_workflow_gates(path)
        (directory / "gate.yml").write_text("jobs:\n  gate:\n    steps:\n      - run: cargo audit --file app/src-tauri/Cargo.lock\n")
        rc.assert_workflow_gates(path)
        (directory / "gate.yml").write_text("jobs:\n  gate:\n    steps:\n      - run: cargo test\n")
        with self.assertRaises(rc.ReleaseError):
            rc.assert_workflow_gates(path)
        # a multi-job reusable gate counts only with the audit in an
        # unconditional job and a summary `gate` job over every other job
        split = (
            "jobs:\n"
            "  checks:\n"
            "    steps:\n"
            "      - run: cargo audit --file app/src-tauri/Cargo.lock\n"
            "  rust:\n"
            "    steps:\n"
            "      - run: cargo test\n"
            "  gate:\n"
            "    needs: [checks, rust]\n"
            "    if: always()\n"
            "    steps:\n"
            "      - env:\n"
            "          NEEDS: ${{ toJSON(needs) }}\n"
            "        run: python3 scripts/release_channels.py gate-results --needs \"$NEEDS\"\n"
        )
        (directory / "gate.yml").write_text(split)
        rc.assert_workflow_gates(path)
        for broken in (
            split.replace("  checks:\n    steps:", "  checks:\n    if: github.event_name == 'push'\n    steps:"),
            split.replace("    needs: [checks, rust]\n", "    needs: [rust]\n"),
            split.replace("    if: always()\n", ""),
            split.replace("    if: always()\n", "    if: success()\n"),
            split.replace("gate-results", "assert-workflow-gates"),
            split.replace("  gate:\n", "  summary:\n"),
        ):
            (directory / "gate.yml").write_text(broken)
            with self.assertRaises(rc.ReleaseError, msg=broken):
                rc.assert_workflow_gates(path)

    def test_gate_results_pass_only_when_every_job_succeeded_on_one_commit(self) -> None:
        commit = "a" * 40
        job = lambda result="success", sha=commit: {"result": result, "outputs": {"commit": sha}}  # noqa: E731
        needs = {"checks": job(), "rust": job(), "release": job(), "macos14-launch": job()}
        self.assertEqual(rc.assert_gate_results(needs), commit)
        self.assertEqual(rc.assert_gate_results(needs, commit), commit)
        self.assertEqual(rc.assert_gate_results(needs, "main"), commit)
        for name, broken in (
            ("failure", {**needs, "rust": job("failure")}),
            ("cancelled", {**needs, "release": job("cancelled")}),
            ("skipped", {**needs, "macos14-launch": {"result": "skipped", "outputs": {}}}),
            ("no result", {**needs, "checks": {"outputs": {"commit": commit}}}),
            ("no commit", {**needs, "checks": {"result": "success", "outputs": {}}}),
            ("short commit", {**needs, "checks": job(sha=commit[:12])}),
            ("two commits", {**needs, "release": job(sha="b" * 40)}),
            ("no jobs", {}),
            ("not an object", []),
        ):
            with self.assertRaises(rc.ReleaseError, msg=name):
                rc.assert_gate_results(broken)
        with self.assertRaises(rc.ReleaseError):
            rc.assert_gate_results(needs, "c" * 40)

        def cli(*argv: str) -> int:
            with patch.object(sys, "argv", ["release_channels.py", *argv]), \
                    redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
                return rc.cli()

        self.assertEqual(cli("gate-results", "--needs", json.dumps(needs)), 0)
        self.assertNotEqual(cli("gate-results", "--needs", json.dumps({**needs, "rust": job("cancelled")})), 0)
        self.assertNotEqual(cli("gate-results", "--needs", "not json"), 0)

    def test_repository_gate_splits_without_dropping_a_check(self) -> None:
        gate = (ROOT / ".github/workflows/gate.yml").read_text()
        jobs = rc.workflow_jobs(gate)
        self.assertEqual(set(jobs), {"checks", "rust", "release", "macos14-launch", "gate"})
        rc.assert_gate_summary(ROOT / ".github/workflows/gate.yml")
        self.assertEqual(rc.job_needs(jobs["macos14-launch"]), {"release"})
        for name in ("checks", "rust", "release"):
            self.assertEqual(rc.job_needs(jobs[name]), set(), f"{name} starts at once")
            self.assertNotRegex(jobs[name], r"(?m)^    if:", f"{name} always runs")
        for name in ("checks", "rust", "release", "macos14-launch", "gate"):
            self.assertIn("ref: ${{ inputs.ref }}", jobs[name], f"{name} checks out the gated ref")
        for name in ("checks", "rust", "release", "macos14-launch"):
            self.assertIn("commit: ${{ steps.commit.outputs.sha }}", jobs[name], name)
            self.assertIn('echo "sha=$(git rev-parse HEAD)" >> "$GITHUB_OUTPUT"', jobs[name], name)
        for command, owner in (
            ("shasum -a 256 -c app/src-tauri/binaries/tmux-aarch64-apple-darwin.sha256", "checks"),
            ("scripts/check-bergamot-patches", "checks"),
            ("cargo fmt --all --check --manifest-path app/src-tauri/Cargo.toml", "checks"),
            ("node --check", "checks"),
            ("scripts/ui-tests", "checks"),
            ("scripts/test-speech-bridge", "checks"),
            ("scripts/test-notification-bridge", "checks"),
            ("scripts/test-smoke-guard", "checks"),
            ("node app/ui/js/check.mjs", "checks"),
            ("python3 -m unittest scripts/test_release_tools.py", "checks"),
            ("scripts/check-workflows", "checks"),
            ("cargo audit --file app/src-tauri/Cargo.lock", "checks"),
            ("cargo clippy --all-targets --locked --manifest-path tools/deck-tunnelctl/Cargo.toml -- -D warnings", "checks"),
            ("cargo test --all-targets --locked --manifest-path tools/deck-tunnelctl/Cargo.toml", "checks"),
            ("cargo audit --file tools/deck-tunnelctl/Cargo.lock", "checks"),
            ("cargo clippy --workspace --all-targets --all-features --locked --manifest-path app/src-tauri/Cargo.toml -- -D warnings", "rust"),
            ("cargo llvm-cov --workspace --all-targets --all-features --locked --fail-under-lines 75 --fail-under-functions 75", "rust"),
            ("cargo build --locked -p deck-mcp-runner", "rust"),
            ("python3 scripts/ce_parity.py --plan scripts/ce/plan-full-local-2.json --mode ci --app-evidence", "rust"),
            ("--gate full_local_parity:ci", "rust"),
            ("cargo build --release --locked --manifest-path app/src-tauri/Cargo.toml", "release"),
            ("python3 scripts/check_local_translation_binary.py app/src-tauri/target/release/deck-app", "release"),
            ("scripts/check-edr-binary app/src-tauri/target/release/deck-app", "release"),
            ("scripts/package-compat-candidate", "release"),
            ("scripts/test-compat-candidate", "macos14-launch"),
        ):
            self.assertEqual([name for name, body in jobs.items() if command in body], [owner], command)
        # the coverage run is what writes the B-admission evidence: it gets
        # every probe variable from the binding step, and parity reads that file
        coverage = jobs["rust"][jobs["rust"].index("- name: cargo test + coverage"):]
        coverage = coverage[:coverage.index("\n      - name:", 1)]
        for variable in ("DECK_CE_EVIDENCE", "DECK_CE_PLAN_DIGEST", "DECK_CE_DECK_BUILD", "DECK_CE_PLAN_ID", "DECK_CE_CASE"):
            self.assertIn(f"{variable}: ${{{{ steps.ce-probe.outputs.{variable} }}}}", coverage)
        self.assertLess(jobs["rust"].index("--print-app-probe-env"), jobs["rust"].index("cargo llvm-cov"))
        self.assertLess(jobs["rust"].index("cargo llvm-cov"), jobs["rust"].index("--mode ci --app-evidence"))
        self.assertNotIn("--app-probe ", gate)
        self.assertNotIn("GITHUB_SHA", gate)

    def test_gate_runs_every_script_test(self) -> None:
        gate = (ROOT / ".github/workflows/gate.yml").read_text()
        self.assertEqual(script_tests_the_gate_misses(gate, ROOT / "scripts"), [])
        # the check itself, on a gate and a scripts directory made for it
        step = "        run: "
        ok = f"{step}python3 -m unittest scripts/test_a.py scripts/test_b.py\n{step}scripts/test-shell --flag\n"
        with tempfile.TemporaryDirectory() as tmp:
            scripts = Path(tmp)
            for name in ("test_a.py", "test_b.py", "test-shell", "helper.py", "tool"):
                (scripts / name).write_text("")
            self.assertEqual(script_tests_the_gate_misses(ok, scripts), [])
            for broken, problem in (
                (ok.replace(" scripts/test_b.py", ""), "scripts/test_b.py is not in the unittest step"),
                (ok.replace("test_b.py", "test_b.py scripts/test_gone.py"), "scripts/test_gone.py is listed but does not exist"),
                (ok.replace("test_b.py", "test_b.py scripts/test_a.py"), "scripts/test_a.py is listed more than once"),
                (ok.replace(f"{step}scripts/test-shell", f"{step}scripts/test-shell-two"), "scripts/test-shell has no run step"),
                # a comment or an argument that mentions the program is not a step
                (ok.replace(f"{step}scripts/test-shell", "        # scripts/test-shell"), "scripts/test-shell has no run step"),
                (ok.replace(f"{step}scripts/test-shell", f"{step}echo scripts/test-shell"), "scripts/test-shell has no run step"),
                (ok.replace(f"{step}python3", f"{step}echo python3"), "expected one unittest step, found 0"),
                (ok + ok, "expected one unittest step, found 2"),
            ):
                self.assertEqual(script_tests_the_gate_misses(broken, scripts), [problem], broken)

    def test_repository_workflows_gate_every_app_build_on_the_audit(self) -> None:
        for path in (ROOT / ".github/workflows").glob("*.yml"):
            rc.assert_workflow_gates(path)
        stable = (ROOT / ".github/workflows/release.yml").read_text()
        self.assertIn("needs: [resolve, gate]", stable)

    def test_gate_installs_prebuilt_checked_runners(self) -> None:
        gate = (ROOT / ".github/workflows/gate.yml").read_text()
        self.assertNotIn("cargo install", gate)
        self.assertIn("tool: cargo-llvm-cov@0.8.5\n", gate)
        self.assertIn("tool: cargo-audit@0.22.2\n", gate)
        self.assertEqual(len(re.findall(r"(?m)^\s+checksum: true$", gate)), 2)
        self.assertEqual(len(re.findall(r"(?m)^\s+fallback: none$", gate)), 2)

    def test_nightly_tags_are_ignored_by_stable_resolver(self) -> None:
        with self.assertRaises(rc.ReleaseError):
            rc.require_stable_tag(self.tag)
        self.assertEqual(rc.require_stable_tag("v0.4.37"), "0.4.37")

    def test_workflows_keep_channels_and_release_ownership_separate(self) -> None:
        nightly = (ROOT / ".github/workflows/nightly.yml").read_text()
        promote = (ROOT / ".github/workflows/promote.yml").read_text()
        stable = (ROOT / ".github/workflows/release.yml").read_text()
        self.assertIn("nightly-feed", nightly)
        self.assertIn("releaseDraft: true", stable)
        self.assertIn("promotion owns this release", stable)
        self.assertNotIn('gh release delete "$latest"', stable)
        self.assertRegex(stable, r"\^v\[0-9\]\+\\\.\[0-9\]\+\\\.\[0-9\]\+\$")
        self.assertLess(promote.index('gh release upload "$STABLE_TAG" stable/latest.json'),
                        promote.index('gh release edit "$STABLE_TAG" --draft=false'))
        self.assertNotIn("gh release upload nightly-feed", promote)
        self.assertNotIn("gh release edit nightly-feed", promote)
        self.assertIn("build-sign:", nightly)
        self.assertIn("scripts/package-tunnel-helper build", nightly)
        self.assertIn('"candidate/$helper_name"', nightly)
        self.assertIn('scripts/verify-tunnel-helper \'redownload/helper-extracted/Deck Tunnel Helper.app\' --stapled', nightly)
        self.assertRegex(nightly, r"build-sign:[\s\S]*permissions:\n\s+contents: read")
        self.assertRegex(nightly, r"publish:[\s\S]*permissions:\n\s+contents: write")
        self.assertIn("NIGHTLY_TAURI_SIGNING_PRIVATE_KEY", nightly)
        self.assertIn("legacy Stable updater key is permitted only for the v0.5.4 migration", nightly)
        self.assertIn("Stable-only key", promote)
        self.assertIn("helper_name=$(jq -er '.helper.archive' candidate/provenance.json)", promote)
        self.assertIn('cp "candidate/$helper_name" "stable/$helper_name"', promote)
        self.assertIn("'helper_archive')],", promote)
        self.assertIn('deck_aarch64.app.tar.gz.sig "$helper_name"', promote)
        self.assertIn('"stable/$helper_name"', promote)
        self.assertIn('deck_aarch64.app.tar.gz.sig "$helper_name" SHA256SUMS promotion.json', promote)
        site = (ROOT / ".github/workflows/site-deploy.yml").read_text()
        self.assertNotIn("gitHubToken", site)
        for path in (ROOT / ".github/workflows").glob("*.yml"):
            for action_ref in re.findall(r"uses:\s+[^@\s]+@([^\s#]+)", path.read_text()):
                self.assertRegex(action_ref, r"^[0-9a-f]{40}$", f"unpinned action in {path.name}")
        for workflow in (nightly, promote):
            self.assertIn("Print :CFBundleExecutable", workflow)
            self.assertIn('grep -aFq "$CANDIDATE_SHA"', workflow)
            self.assertNotIn('Contents/MacOS/deck"', workflow)
        rc.assert_promotion_has_no_build_commands(ROOT / ".github/workflows/promote.yml")

    def test_workflows_pin_the_repository_toolchain(self) -> None:
        toolchain = (ROOT / "rust-toolchain.toml").read_text()
        channel = re.search(r'^channel = "([0-9]+\.[0-9]+\.[0-9]+)"$', toolchain, re.M)
        self.assertIsNotNone(channel, "rust-toolchain.toml pins an exact version")
        for path in (ROOT / ".github/workflows").glob("*.yml"):
            text = path.read_text()
            uses = len(re.findall(r"uses:\s+dtolnay/rust-toolchain@", text))
            pins = re.findall(r"^\s+toolchain: (\S+)$", text, re.M)
            self.assertEqual(len(pins), uses, f"every rust-toolchain step in {path.name} pins a version")
            for pin in pins:
                self.assertEqual(pin, channel.group(1), f"{path.name} pins the rust-toolchain.toml version")

    def test_translation_gate_build_and_deployment_target(self) -> None:
        workflow = (ROOT / ".github/workflows/gate.yml").read_text()
        self.assertIn("runs-on: macos-26", workflow)
        self.assertIn("DEVELOPER_DIR: /Applications/Xcode_26.4.1.app/Contents/Developer", workflow)
        self.assertIn("cargo build --release --locked", workflow)
        self.assertIn("python3 scripts/check_local_translation_binary.py app/src-tauri/target/release/deck-app", workflow)
        config = (ROOT / "app/src-tauri/tauri.conf.json").read_text()
        self.assertIn('"minimumSystemVersion": "11.0"', config)

    def test_translation_artifact_gate_reads_macho_dependencies(self) -> None:
        parse = translation_binary["dependencies"]
        source = "Load command 1\n          cmd LC_LOAD_DYLIB\n"
        apple = "         name /System/Library/Frameworks/Translation.framework/Versions/A/Translation (offset 24)\n"
        self.assertEqual(parse(source + apple), ["/System/Library/Frameworks/Translation.framework/Versions/A/Translation"])
        self.assertEqual(parse(source), [])

    def test_provenance_generation_and_complete_candidate_verification(self) -> None:
        directory, _ = self.candidate_fixture()
        provenance = rc.create_provenance(
            directory=directory,
            dmg_name=f"deck_{self.version}_aarch64.dmg",
            version=self.version,
            tag=self.tag,
            commit=self.sha,
            run_id="123",
            run_attempt="2",
            built_at="2026-08-29T00:00:00Z",
            team_id="Y8ZG3D692W",
            identity="Developer ID Application",
            updater_key_epoch="nightly-v1",
        )
        (directory / "provenance.json").write_text(json.dumps(provenance))
        release = {
            "tagName": self.tag,
            "isDraft": False,
            "isPrerelease": True,
            "targetCommitish": self.sha,
            "assets": [
                {"name": item["name"]} for item in provenance["artifacts"]
            ] + [{"name": "SHA256SUMS"}, {"name": "provenance.json"}],
        }
        self.assertEqual(
            rc.verify_candidate_directory(directory, self.tag, self.sha, release),
            self.version,
        )
        self.assertEqual(
            rc.release_asset_names(release),
            {
                f"deck_{self.version}_aarch64.dmg",
                rc.ARCHIVE,
                rc.SIGNATURE,
                "candidate.json",
                "SHA256SUMS",
                "provenance.json",
            },
        )

        for kwargs in [
            {"team_id": "bad team"},
            {"identity": ""},
            {"identity": "bad\nidentity"},
            {"updater_key_epoch": "unknown"},
            {"commit": "b" * 40},
        ]:
            values = {
                "directory": directory,
                "dmg_name": f"deck_{self.version}_aarch64.dmg",
                "version": self.version,
                "tag": self.tag,
                "commit": self.sha,
                "run_id": "123",
                "run_attempt": "1",
                "built_at": "2026-08-29T00:00:00Z",
                "team_id": "Y8ZG3D692W",
                "identity": "Developer ID Application",
                "updater_key_epoch": "nightly-v1",
            }
            values.update(kwargs)
            with self.assertRaises(rc.ReleaseError):
                rc.create_provenance(**values)

    def test_checksum_and_release_asset_parsers_fail_closed(self) -> None:
        directory = Path(tempfile.mkdtemp(prefix="deck-sums-test-"))
        (directory / "a.txt").write_text("a")
        (directory / "b.txt").write_text("b")
        sums = directory / "SHA256SUMS"
        rc.write_sums(directory, ["b.txt", "a.txt"], sums)
        parsed = rc.parse_sums(sums)
        self.assertEqual(list(parsed), ["a.txt", "b.txt"])
        self.assertEqual(parsed["a.txt"], rc.sha256(directory / "a.txt"))
        with self.assertRaises(rc.ReleaseError):
            rc.write_sums(directory, ["missing.txt"], sums)
        for invalid in ["not a sum\n", f"{'0' * 64}  a.txt\n{'1' * 64}  a.txt\n"]:
            sums.write_text(invalid)
            with self.assertRaises(rc.ReleaseError):
                rc.parse_sums(sums)

        with self.assertRaises(rc.ReleaseError):
            rc.release_asset_names({})
        with self.assertRaises(rc.ReleaseError):
            rc.release_asset_names({"assets": [{"name": "a"}, {"name": "a"}]})

    def test_release_channel_cli_success_and_error_routes(self) -> None:
        directory, provenance = self.candidate_fixture()
        (directory / "provenance.json").write_text(json.dumps(provenance))
        notes = directory / "notes.txt"
        notes.write_text("candidate notes")

        def invoke(*args: str) -> tuple[int, str, str]:
            out, err = io.StringIO(), io.StringIO()
            with patch.object(sys, "argv", ["release_channels.py", *args]), redirect_stdout(out), redirect_stderr(err):
                code = rc.cli()
            return code, out.getvalue(), err.getvalue()

        code, out, err = invoke("validate-version", self.version)
        self.assertEqual((code, out.strip(), err), (0, self.version, ""))
        code, out, err = invoke(
            "candidate-tag", "--version", self.version, "--date", "20260829", "--sha", self.sha
        )
        self.assertEqual((code, out.strip(), err), (0, self.tag, ""))
        code, out, err = invoke(
            "assert-newer", "--candidate", self.version, "--published", "0.4.36"
        )
        self.assertEqual((code, out.strip(), err), (0, self.version, ""))

        manifest_path = directory / "cli-manifest.json"
        code, _, err = invoke(
            "manifest",
            "--version", self.version,
            "--tag", self.tag,
            "--signature-file", str(directory / rc.SIGNATURE),
            "--notes-file", str(notes),
            "--pub-date", "2026-08-29T00:00:00Z",
            "--output", str(manifest_path),
        )
        self.assertEqual((code, err), (0, ""))
        code, out, err = invoke(
            "verify-manifest",
            "--file", str(manifest_path),
            "--version", self.version,
            "--tag", self.tag,
            "--signature-file", str(directory / rc.SIGNATURE),
        )
        self.assertEqual((code, out.strip(), err), (0, self.version, ""))
        code, out, err = invoke("validate-version", "not-a-version")
        self.assertEqual(code, 2)
        self.assertEqual(out, "")
        self.assertIn("invalid numeric version", err)


@unittest.skipUnless(sys.platform == "darwin", "check-edr-binary reads a bundle with PlistBuddy and strings")
class EdrBinaryBundleTests(unittest.TestCase):
    """scripts/check-edr-binary against local fixture bundles.

    The lifecycle helper is required exactly when the bundle's own application
    executable names it: a candidate built before the helper existed stays
    acceptable at promotion, and a bundle that can use the helper cannot ship
    without it. A helper that is present is always scanned. The fixtures are
    synthetic text files; what a real release executable carries is measured
    separately on a local release build, never assumed here.
    """

    SCRIPT = ROOT / "scripts" / "check-edr-binary"
    HELPER = "deck-lifecycle-helper"
    # The three spellings an application executable may carry the name in:
    # today's two literals of agent_status/hooks.rs and the dedicated marker.
    EVIDENCE = {
        "name": HELPER,
        "hook-marker": f"deck.app/Contents/MacOS/{HELPER}",
        "requires-marker": f"deck-bundle-requires:{HELPER}",
    }
    OLD_SIDECARS = ("deck-status-helper", "deck-mcp", "deck-mcp-runner")
    CLEAN_HELPER = "fixture lifecycle helper\n"
    # spelled in two parts so that this file never holds a scanned token whole
    FORBIDDEN = "launch" + "ctl"

    def bundle(self, root: Path, name: str, *, app_names_helper: bool | str, helper: str | None, **other: str) -> Path:
        """`app_names_helper`: False, True (the hook marker) or an EVIDENCE key."""
        app = root / f"{name}.app"
        macos = app / "Contents" / "MacOS"
        macos.mkdir(parents=True)
        with (app / "Contents" / "Info.plist").open("wb") as stream:
            plistlib.dump({"CFBundleExecutable": "deck"}, stream)
        key = "hook-marker" if app_names_helper is True else app_names_helper
        marker = f"{self.EVIDENCE[key]}\n" if key else ""
        (macos / "deck").write_text("fixture application executable\n" + marker + other.get("deck", ""))
        for sidecar in self.OLD_SIDECARS:
            (macos / sidecar).write_text(f"fixture sidecar {sidecar}\n" + other.get(sidecar, ""))
        if helper is not None:
            (macos / self.HELPER).write_text(helper)
        return app

    def check(self, app: Path) -> subprocess.CompletedProcess[str]:
        return subprocess.run([str(self.SCRIPT), str(app)], capture_output=True, text=True, timeout=60)

    def test_a_new_bundle_without_the_helper_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            for key in self.EVIDENCE:
                missing = self.check(self.bundle(root, f"missing-{key}", app_names_helper=key, helper=None))
                self.assertEqual(missing.returncode, 1, key)
                self.assertIn(f"references {self.HELPER}, but the bundle does not contain it", missing.stderr, key)
                self.assertNotIn("vocabulary clean", missing.stdout, key)
            # the name inside a longer run of text, as a linker lays constants out
            packed = self.check(self.bundle(root, "packed", app_names_helper=False, helper=None,
                                            deck=f"agent-status{self.HELPER}deck.app/Contents\n"))
            self.assertEqual(packed.returncode, 1)

    def test_a_new_bundle_with_the_helper_has_the_helper_scanned(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            for key in self.EVIDENCE:
                complete = self.check(self.bundle(root, f"new-{key}", app_names_helper=key, helper=self.CLEAN_HELPER))
                self.assertEqual(complete.returncode, 0, complete.stderr)
                self.assertIn(f"/MacOS/{self.HELPER}", complete.stdout, "a present helper is among the scanned files")
                for sidecar in ("deck", *self.OLD_SIDECARS):
                    self.assertIn(f"/MacOS/{sidecar}", complete.stdout, sidecar)
            dirty = self.check(self.bundle(root, "new-dirty", app_names_helper=True,
                                           helper=self.CLEAN_HELPER + self.FORBIDDEN + " load\n"))
            self.assertEqual(dirty.returncode, 1)
            self.assertIn(f"{self.HELPER} contains forbidden EDR token: {self.FORBIDDEN}", dirty.stderr)

    # ---- the Pi observer's files (Contents/Resources/pi) ----

    PI_MARKER = "Contents/Resources/pi"
    PI_FILES = ROOT / "app" / "src-tauri" / "resources" / "pi"

    def pi_bundle(self, root: Path, name: str, *, named: bool, files: dict[str, str] | None) -> Path:
        """A complete bundle whose application names the Pi directory or not,
        and that holds `files` there (None: no such directory)."""
        app = self.bundle(root, name, app_names_helper=True, helper=self.CLEAN_HELPER,
                          deck=f"{self.PI_MARKER}\n" if named else "")
        if files is not None:
            directory = app / "Contents" / "Resources" / "pi"
            directory.mkdir(parents=True)
            for file, text in files.items():
                (directory / file).write_text(text)
        return app

    def test_a_bundle_that_names_the_pi_observer_must_carry_it(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            real = {path.name: path.read_text() for path in sorted(self.PI_FILES.glob("*.ts"))}
            self.assertIn("observer.ts", real)
            # the checkout's own files pass the packaged vocabulary
            complete = self.check(self.pi_bundle(root, "complete", named=True, files=real))
            self.assertEqual((complete.returncode, complete.stderr), (0, ""))
            missing = self.check(self.pi_bundle(root, "missing", named=True, files=None))
            self.assertEqual(missing.returncode, 1)
            self.assertIn("references Contents/Resources/pi, but the bundle does not contain it", missing.stderr)
            # a bundle from before Pi names nothing and lacks nothing
            old = self.check(self.pi_bundle(root, "old", named=False, files=None))
            self.assertEqual((old.returncode, old.stderr), (0, ""))
            # a directory that is there is checked whatever the application says
            for named in (True, False):
                without = self.check(self.pi_bundle(root, f"no-observer-{named}", named=named,
                                                    files={"signal.ts": real["signal.ts"]}))
                self.assertEqual(without.returncode, 1, named)
                self.assertIn("does not contain observer.ts", without.stderr)

    def test_a_packaged_pi_observer_file_is_held_to_the_closed_vocabulary(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            clean = (self.PI_FILES / "observer.ts").read_text()
            # spelled in parts so that this file never holds a scanned token whole
            for index, token in enumerate(("child" + "_process", "sp" + "awn(", "send" + "UserMessage(",
                                           "switch" + "Session(", "execute" + "Tool(", "fe" + "tch(",
                                           "node:" + "http", "needs" + "-input", "write" + "FileSync(",
                                           "get" + "Entries(", "re" + "quire(")):
                dirty = self.check(self.pi_bundle(root, f"dirty-{index}", named=True,
                                                  files={"observer.ts": clean + f"\n{token}\n"}))
                self.assertEqual(dirty.returncode, 1, token)
                self.assertIn("contains forbidden Pi observer token", dirty.stderr, token)
            # nothing but regular .ts files lives there
            stray = self.check(self.pi_bundle(root, "stray", named=True,
                                              files={"observer.ts": clean, "helper.sh": "true\n"}))
            self.assertEqual(stray.returncode, 1)
            self.assertIn("is not a Pi observer file", stray.stderr)
            linked = self.pi_bundle(root, "linked", named=True, files={"observer.ts": clean})
            (linked / "Contents" / "Resources" / "pi" / "extra.ts").symlink_to("/etc/hosts")
            result = self.check(linked)
            self.assertEqual(result.returncode, 1)
            self.assertIn("is not a regular file", result.stderr)
            # the test-only fixtures use exactly what the observer may not: they
            # would be refused if anything ever packaged them
            fixture = (ROOT / "scripts" / "pi" / "fixture.ts").read_text()
            packaged = self.check(self.pi_bundle(root, "fixture", named=True,
                                                 files={"observer.ts": clean, "fixture.ts": fixture}))
            self.assertEqual(packaged.returncode, 1)
            self.assertIn("contains forbidden Pi observer token", packaged.stderr)

    def test_a_bundle_from_before_the_helper_is_never_rejected_for_lacking_it(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            old = self.check(self.bundle(root, "old", app_names_helper=False, helper=None))
            self.assertEqual((old.returncode, old.stderr), (0, ""))
            self.assertIn("EDR binary vocabulary clean", old.stdout)
            self.assertNotIn(self.HELPER, old.stdout, "nothing is scanned that the bundle does not have")
            # the older status helper's name is not evidence of the newer one
            near = self.check(self.bundle(root, "old-near", app_names_helper=False, helper=None,
                                          deck="deck.app/Contents/MacOS/deck-status-helper\ndeck-lifecycle\n"))
            self.assertEqual((near.returncode, near.stderr), (0, ""))

    def test_a_helper_inside_an_old_looking_bundle_is_still_scanned(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            clean = self.check(self.bundle(root, "old-with", app_names_helper=False, helper=self.CLEAN_HELPER))
            self.assertEqual(clean.returncode, 0, clean.stderr)
            self.assertIn(f"/MacOS/{self.HELPER}", clean.stdout)
            dirty = self.check(self.bundle(root, "old-dirty", app_names_helper=False,
                                           helper=self.CLEAN_HELPER + self.FORBIDDEN + " load\n"))
            self.assertEqual(dirty.returncode, 1)
            self.assertIn(f"{self.HELPER} contains forbidden EDR token: {self.FORBIDDEN}", dirty.stderr)
            # a helper that is not a regular file is not a scanned helper, in
            # an old-looking bundle as much as in a new one
            for name, names_helper in (("old-odd", False), ("new-odd", True)):
                odd = self.bundle(root, name, app_names_helper=names_helper, helper=None)
                (odd / "Contents" / "MacOS" / self.HELPER).mkdir()
                self.assertNotEqual(self.check(odd).returncode, 0, name)
                dangling = self.bundle(root, f"{name}-link", app_names_helper=names_helper, helper=None)
                (dangling / "Contents" / "MacOS" / self.HELPER).symlink_to("nowhere")
                self.assertNotEqual(self.check(dangling).returncode, 0, name)

    def test_an_application_executable_that_decides_nothing_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            # with and without a helper: the helper's presence does not excuse it
            for name, helper in (("unreadable", None), ("unreadable-with", self.CLEAN_HELPER)):
                if os.geteuid() == 0:
                    break  # the superuser reads a mode-0 file
                app = self.bundle(root, name, app_names_helper=False, helper=helper)
                executable = app / "Contents" / "MacOS" / "deck"
                executable.chmod(0)
                try:
                    result = self.check(app)
                finally:
                    executable.chmod(0o600)
                self.assertEqual(result.returncode, 1, name)
                self.assertIn("could not be read to decide", result.stderr, name)
                self.assertNotIn("vocabulary clean", result.stdout, name)
            empty = self.bundle(root, "empty", app_names_helper=False, helper=None)
            (empty / "Contents" / "MacOS" / "deck").write_text("")
            result = self.check(empty)
            self.assertEqual(result.returncode, 1)
            self.assertIn("is empty and cannot decide", result.stderr)
            gone = self.bundle(root, "gone", app_names_helper=False, helper=None)
            (gone / "Contents" / "MacOS" / "deck").unlink()
            self.assertNotEqual(self.check(gone).returncode, 0)
            folder = self.bundle(root, "folder", app_names_helper=False, helper=None)
            (folder / "Contents" / "MacOS" / "deck").unlink()
            (folder / "Contents" / "MacOS" / "deck").mkdir()
            self.assertNotEqual(self.check(folder).returncode, 0)
            plistless = self.bundle(root, "plistless", app_names_helper=False, helper=None)
            (plistless / "Contents" / "Info.plist").unlink()
            self.assertNotEqual(self.check(plistless).returncode, 0)

    def test_one_bare_executable_is_scanned_without_the_bundle_rule(self) -> None:
        """The form gate.yml uses on target/release/deck-app: a file, no bundle."""
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            for key in self.EVIDENCE:
                bare = root / f"deck-app-{key}"
                bare.write_text(f"fixture bare executable\n{self.EVIDENCE[key]}\n")
                result = self.check(bare)
                self.assertEqual((result.returncode, result.stderr), (0, ""), key)
                self.assertEqual(result.stdout.strip(), f"EDR binary vocabulary clean: {bare}", key)
            old = root / "deck-app-old"
            old.write_text("fixture bare executable\n")
            self.assertEqual(self.check(old).returncode, 0)
            dirty = root / "deck-app-dirty"
            dirty.write_text(f"fixture bare executable\n{self.FORBIDDEN}\n")
            result = self.check(dirty)
            self.assertEqual(result.returncode, 1)
            self.assertIn(f"contains forbidden EDR token: {self.FORBIDDEN}", result.stderr)
            self.assertNotEqual(self.check(root / "absent").returncode, 0)

    def test_the_older_sidecars_stay_required_and_scanned(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            for sidecar in self.OLD_SIDECARS:
                gone = self.bundle(root, f"without-{sidecar}", app_names_helper=False, helper=None)
                (gone / "Contents" / "MacOS" / sidecar).unlink()
                self.assertNotEqual(self.check(gone).returncode, 0, sidecar)
                dirty = self.check(self.bundle(root, f"dirty-{sidecar}", app_names_helper=False, helper=None,
                                               **{sidecar: self.FORBIDDEN + "\n"}))
                self.assertEqual(dirty.returncode, 1, sidecar)
                self.assertIn(f"{sidecar} contains forbidden EDR token", dirty.stderr)
            dirty_app = self.check(self.bundle(root, "dirty-app", app_names_helper=True, helper="fixture\n",
                                               deck=self.FORBIDDEN + "\n"))
            self.assertEqual(dirty_app.returncode, 1)
            self.assertIn("/MacOS/deck contains forbidden EDR token", dirty_app.stderr)

    def test_the_source_still_holds_the_evidence_the_rule_reads(self) -> None:
        """The source still holds the evidence the rule reads.

        Either the helper's file name is a constant that live code reads at run
        time (it is joined onto the executable's directory to find the sidecar),
        or the dedicated `#[used]` marker is present. A source that has neither
        would let a new bundle pass for an old one. This is a source pin: that
        a linked release executable keeps the bytes is measured on a build, not
        proven here, and `#[used]` alone does not promise it.
        """
        source = (ROOT / "app" / "src-tauri" / "src" / "agent_status" / "hooks.rs").read_text()
        runtime_literal = (
            f'const LIFECYCLE_HELPER_NAME: &str = "{self.HELPER}";' in source
            and re.search(r"\.join\(\s*LIFECYCLE_HELPER_NAME\s*\)", source) is not None
        )
        used_marker = re.search(
            r'#\[used\]\s*(?:pub(?:\([a-z]+\))? )?static \w+: \[u8; \d+\] = \*b"'
            + re.escape(self.EVIDENCE["requires-marker"]) + '";',
            source,
        ) is not None
        self.assertTrue(runtime_literal or used_marker)
        script = self.SCRIPT.read_text()
        self.assertIn(f"grep -F -c -- {self.HELPER}", script)
        self.assertIn(self.EVIDENCE["requires-marker"], script, "the script documents the marker it also accepts")
        for spelling in self.EVIDENCE.values():
            self.assertIn(self.HELPER, spelling)


class PiBuildIdentityTests(unittest.TestCase):
    """`scripts/pi_build_identity.py`: what ties a piece of Pi evidence to the sources it ran from."""

    def setUp(self) -> None:
        self.identity = runpy.run_path(str(ROOT / "scripts" / "pi_build_identity.py"))
        self.dir = tempfile.TemporaryDirectory()
        self.repo = Path(self.dir.name).resolve()
        self.git("init", "-q")
        (self.repo / "app/src-tauri/src").mkdir(parents=True)
        (self.repo / "app/src-tauri/src/main.rs").write_text("fn main() {}\n")
        (self.repo / "notes.md").write_text("not a source\n")
        self.git("add", "-A")
        self.git("-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-q", "-m", "one")

    def tearDown(self) -> None:
        self.dir.cleanup()

    def git(self, *args: str) -> str:
        return subprocess.run(["/usr/bin/git", "-C", str(self.repo), *args], capture_output=True, text=True, check=True).stdout

    def read(self) -> dict:
        return self.identity["identity"](self.repo)

    def test_a_clean_checkout_names_its_commit_and_is_not_dirty(self) -> None:
        clean = self.read()
        self.assertEqual(clean["head"], self.git("rev-parse", "HEAD").strip())
        self.assertEqual((clean["dirty"], clean["dirty_paths"], clean["source_files"]), (False, [], 1))
        self.assertEqual(clean["diff_sha256"], "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")

    def test_the_source_hash_follows_the_bytes_on_disk_and_only_the_sources(self) -> None:
        clean = self.read()
        # a file outside the source roots changes the dirt, not the source hash
        (self.repo / "notes.md").write_text("changed\n")
        other = self.read()
        self.assertEqual(other["source_sha256"], clean["source_sha256"])
        self.assertEqual((other["dirty"], other["dirty_paths"]), (True, ["notes.md"]))
        self.assertNotEqual(other["diff_sha256"], clean["diff_sha256"])
        # a changed source, and a new one git would track, change it
        (self.repo / "app/src-tauri/src/main.rs").write_text("fn main() { }\n")
        changed = self.read()
        self.assertNotEqual(changed["source_sha256"], clean["source_sha256"])
        (self.repo / "app/src-tauri/src/new.rs").write_text("// new\n")
        added = self.read()
        self.assertNotEqual(added["source_sha256"], changed["source_sha256"])
        self.assertEqual(added["source_files"], 2)
        self.assertIn("app/src-tauri/src/new.rs", added["dirty_paths"])
        # committed, the same bytes are the same hash: a run before the commit and the commit agree
        self.git("add", "-A")
        self.git("-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-q", "-m", "two")
        committed = self.read()
        self.assertEqual(committed["source_sha256"], added["source_sha256"])
        self.assertEqual((committed["dirty"], committed["dirty_paths"]), (False, []))
        # an ignored file is nobody's source
        (self.repo / ".gitignore").write_text("app/src-tauri/src/generated.rs\n")
        (self.repo / "app/src-tauri/src/generated.rs").write_text("// built\n")
        self.assertEqual(self.read()["source_sha256"], committed["source_sha256"])

    def test_no_file_content_or_path_outside_the_repository_is_in_what_it_says(self) -> None:
        (self.repo / "app/src-tauri/src/secret.rs").write_text("// TOP-SECRET-CONTENT\n")
        said = json.dumps(self.read())
        self.assertNotIn("TOP-SECRET-CONTENT", said)
        self.assertNotIn(str(self.repo), said)


if __name__ == "__main__":
    unittest.main()


class PiNativeTapTests(unittest.TestCase):
    """`scripts/pi_native.py`: how the driver reads the backend half's output (follow-up 02, R03)."""

    BEFORE = "test agent_pi::tests::native::tap ... "
    # Writes what the test put beside it, as it is; a line `--- later ---` is not written: what follows it is
    # written once the test has sent a line. It ends with its standard input, and after twenty seconds anyway.
    BACKEND = """#!/usr/bin/env python3
import signal, sys
signal.alarm(20)
for line in open(__file__ + ".out").read().splitlines(keepends=True):
    if line == "--- later ---\\n":
        sys.stdin.readline()
        continue
    sys.stdout.write(line)
    sys.stdout.flush()
sys.stdin.read()
"""

    def setUp(self) -> None:
        self.native = runpy.run_path(str(ROOT / "scripts" / "pi_native.py"))
        self.dir = tempfile.TemporaryDirectory()
        self.root = Path(self.dir.name).resolve()
        (self.root / "logs").mkdir()
        self.taps: list = []

    def tearDown(self) -> None:
        for tap in self.taps:
            try:
                tap.process.stdin.close()
            except OSError:
                pass
            tap.process.wait(timeout=10)
            tap.process.stdout.close()
            tap.log.close()
            tap.raw.close()
            tap.err.close()
        self.dir.cleanup()

    def run_with(self, output: str):
        """A stand-in for the driver's run whose backend half writes exactly `output` and then waits."""
        app = self.root / "backend"
        app.write_text(self.BACKEND)
        app.chmod(0o700)
        (self.root / "backend.out").write_text(output)
        failed = self.native["Failed"]

        class Run:
            taps = 0
            logs = self.root / "logs"
            work = self.root
            env = {"PATH": "/usr/bin:/bin"}

            def registry(self, *args, **kwargs):
                return None

            def note(self, *args, **kwargs):
                return None

            def until(self, label, predicate, timeout=20):
                end = __import__("time").monotonic() + timeout
                while __import__("time").monotonic() < end:
                    value = predicate()
                    if value:
                        return value
                    __import__("time").sleep(0.01)
                raise failed(f"timed out waiting for {label}")

        run = Run()
        run.app = app
        return run

    def tap(self, run):
        tap = self.native["Tap"](run)
        self.taps.append(tap)
        return tap

    @staticmethod
    def line(**event) -> str:
        return "@tap " + json.dumps(event) + "\n"

    def test_an_event_begins_its_line_or_follows_libtest_s_own_words_and_nothing_else(self) -> None:
        event = self.native["tap_event"]
        framing = self.native["Framing"]
        self.assertEqual(self.native["LIBTEST_BEFORE"], self.BEFORE)
        ready, now = {"ready": {"socket": "deck-dev"}, "t": 3}, {"now": {"store": {}}, "t": 1}
        # an ordinary line
        self.assertEqual(event(self.line(**ready)), ready)
        self.assertEqual(event(self.line(**ready).rstrip("\n")), ready)
        # the first line a test writes stands behind libtest's words: `ready` or a sample, whichever came first
        self.assertEqual(event(self.BEFORE + self.line(**ready)), ready)
        self.assertEqual(event(self.BEFORE + self.line(**now)), now)
        # what libtest and the harness write that is no event
        for other in ["", "\n", "running 1 test\n", self.BEFORE + "\n", self.BEFORE + "ok\n", "ok\n",
                      "test result: ok. 1 passed; 0 failed\n", "agent_pi::tests::native::tap: no DECK_PI_NATIVE_ROOT\n",
                      "@tapped\n", "@tap\n"]:
            self.assertIsNone(event(other), other)
        # an event anywhere else in a line, behind other words or another test's, is not looked for: it is an error
        for bad in ["log: " + self.line(**ready), " " + self.line(**ready), "test other::tap ... " + self.line(**ready),
                    self.BEFORE + self.BEFORE + self.line(**ready), self.BEFORE + "x" + self.line(**ready),
                    self.line(**ready).rstrip("\n") + self.line(**now)]:
            if bad.count("@tap ") == 2 and bad.startswith("@tap "):
                # two events on one line: the line is no JSON object
                with self.assertRaises(framing):
                    event(bad)
                continue
            with self.assertRaises(framing, msg=bad):
                event(bad)
        # an event that is no JSON object is an error too, whatever stands in front of it
        for bad in ["@tap {\"ready\": \n", "@tap [1]\n", "@tap 7\n", "@tap \n", "@tap {'a': 1}\n",
                    self.BEFORE + "@tap {\"now\": \n"]:
            with self.assertRaises(framing, msg=bad):
                event(bad)

    def test_the_first_event_is_read_whichever_it_is(self) -> None:
        ready = self.line(ready={"socket": "deck-dev", "status_sock": "/x"}, t=5)
        sample = self.line(now={"store": {"deck-a": {"state": "working", "episode": 4}}, "leases": []}, t=1)
        # `ready` first, on libtest's line
        first = self.tap(self.run_with("\nrunning 1 test\n" + self.BEFORE + ready + sample))
        self.assertEqual(first.ready, {"socket": "deck-dev", "status_sock": "/x"})
        first.run.until("the sample", lambda: len(first.since()) == 2, 5)
        self.assertEqual(first.state("deck-a"), "working")
        # a sample first, on libtest's line: it is not lost, and `ready` behind it is found
        second = self.tap(self.run_with("\nrunning 1 test\n" + self.BEFORE + sample + ready))
        self.assertEqual(second.ready["socket"], "deck-dev")
        self.assertEqual([sorted(event)[0] for event in second.since()], ["now", "ready"])
        self.assertEqual(second.state("deck-a"), "working")
        # every line is kept as it was written, beside the events
        raw = (self.root / "logs" / f"tap-{second.number}.stdout").read_text()
        self.assertEqual(raw, "\nrunning 1 test\n" + self.BEFORE + sample + ready)
        events = (self.root / "logs" / f"tap-{second.number}.jsonl").read_text().splitlines()
        self.assertEqual(len(events), 2)

    def test_a_second_life_of_the_backend_half_is_read_the_same_way(self) -> None:
        ready = self.line(ready={"socket": "deck-dev"}, t=5)
        run = self.run_with("\nrunning 1 test\n" + self.BEFORE + ready)
        first = self.tap(run)
        # Deck "restarts": another process, whose first line stands behind libtest's words again
        (self.root / "backend.out").write_text("\nrunning 1 test\n" + self.BEFORE + self.line(now={"store": {}}, t=0) + ready)
        second = self.tap(run)
        self.assertEqual((first.number, second.number), (1, 2))
        self.assertEqual(len(first.since()), 1)
        self.assertEqual([sorted(event)[0] for event in second.since()], ["now", "ready"])
        for number in (1, 2):
            self.assertTrue((self.root / "logs" / f"tap-{number}.stdout").read_text().count(self.BEFORE) == 1)

    def test_a_line_that_breaks_the_framing_fails_the_run_at_once_and_is_kept(self) -> None:
        failed = self.native["Failed"]
        ready = self.line(ready={"socket": "deck-dev"}, t=5)
        for bad in ["@tap {\"ready\": \n", "panicked at src/x.rs: " + self.line(now={"store": {}}), "@tap [\"ready\"]\n"]:
            run = self.run_with("\nrunning 1 test\n" + bad + ready)
            started = __import__("time").monotonic()
            with self.assertRaises(failed) as refused:
                self.native["Tap"](run)
            self.assertLess(__import__("time").monotonic() - started, 5, "not after the thirty seconds a silent backend gets")
            self.assertIn("not what it must be", str(refused.exception))
            raw = (self.root / "logs" / f"tap-{run.taps}.stdout").read_text()
            self.assertIn(bad, raw, "the line that broke it is kept")
            # nothing behind the broken line was taken for an event
            self.assertEqual((self.root / "logs" / f"tap-{run.taps}.jsonl").read_text(), "")
        # a framing that breaks later fails whoever asks next, and what was read before it is not served either
        run = self.run_with(self.BEFORE + ready + "--- later ---\n@tap not json\n" + self.line(now={"store": {}}))
        tap = self.tap(run)
        self.assertEqual(len(tap.since()), 1)
        tap.process.stdin.write("\n")
        tap.process.stdin.flush()
        run.until("the broken line", lambda: tap.broken, 5)
        for ask in (tap.mark, tap.since, tap.now, lambda: tap.first(lambda event: "now" in event, "a sample", 1)):
            with self.assertRaises(failed):
                ask()
        self.assertIn("@tap not json\n", (self.root / "logs" / f"tap-{tap.number}.stdout").read_text())
