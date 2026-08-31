from __future__ import annotations

import base64
import importlib.util
import io
import json
import re
import runpy
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


class VersionToolTests(unittest.TestCase):
    def fixture(self, tauri: str = "0.4.37", cargo: str = "0.4.37", lock: str = "0.4.37") -> Path:
        root = Path(tempfile.mkdtemp(prefix="deck-version-test-"))
        source = root / "app" / "src-tauri"
        source.mkdir(parents=True)
        (source / "tauri.conf.json").write_text(json.dumps({"version": tauri}, indent=2) + "\n")
        (source / "Cargo.toml").write_text(f'[package]\nname = "deck-app"\nversion = "{cargo}"\n')
        (source / "Cargo.lock").write_text(
            f'[[package]]\nname = "deck-app"\nversion = "{lock}"\n\n'
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
        self.assertRegex(nightly, r"build-sign:[\s\S]*permissions:\n\s+contents: read")
        self.assertRegex(nightly, r"publish:[\s\S]*permissions:\n\s+contents: write")
        self.assertIn("NIGHTLY_TAURI_SIGNING_PRIVATE_KEY", nightly)
        self.assertIn("legacy Stable updater key is permitted only for the v0.5.4 migration", nightly)
        self.assertIn("Stable-only key", promote)
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


if __name__ == "__main__":
    unittest.main()
