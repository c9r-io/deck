#!/usr/bin/env python3
"""Deterministic validators/generators shared by Nightly and promotion workflows."""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import re
import sys
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import urlparse

VERSION_RE = re.compile(r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$")
STABLE_TAG_RE = re.compile(r"^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$")
CANDIDATE_TAG_RE = re.compile(
    r"^nightly-v((?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*))"
    r"-([0-9]{8})-([0-9a-f]{7,40})$"
)
FULL_SHA_RE = re.compile(r"^[0-9a-f]{40}$")
TARGET = "darwin-aarch64"
BUNDLE_ID = "io.c9r.deck"
ARCHIVE = "deck_aarch64.app.tar.gz"
SIGNATURE = f"{ARCHIVE}.sig"
IMMUTABLE_KINDS = ("dmg", "archive", "signature", "manifest")
UPDATER_KEY_EPOCHS = {"legacy-stable-v1", "nightly-v1"}
FORBIDDEN_PROMOTION = re.compile(
    r"\b(?:cargo\s+(?:build|run|install)|tauri(?:-action|\s+build)|npm\s+run\s+build|xcodebuild)\b",
    re.IGNORECASE,
)


class ReleaseError(RuntimeError):
    pass


def version_tuple(value: str) -> tuple[int, int, int]:
    match = VERSION_RE.fullmatch(value)
    if not match:
        raise ReleaseError(f"invalid numeric version: {value!r}")
    return tuple(int(part) for part in match.groups())  # type: ignore[return-value]


def candidate_tag(version: str, date: str, sha: str) -> str:
    version_tuple(version)
    if not re.fullmatch(r"[0-9]{8}", date):
        raise ReleaseError("candidate date must be YYYYMMDD digits")
    if not FULL_SHA_RE.fullmatch(sha):
        raise ReleaseError("candidate commit must be a lowercase full SHA")
    return f"nightly-v{version}-{date}-{sha[:7]}"


def parse_candidate_tag(tag: str) -> tuple[str, str, str]:
    match = CANDIDATE_TAG_RE.fullmatch(tag)
    if not match:
        raise ReleaseError("candidate tag does not match the closed Nightly format")
    version_tuple(match.group(1))
    return match.group(1), match.group(2), match.group(3)


def assert_newer(candidate: str, published: list[str]) -> None:
    value = version_tuple(candidate)
    for other in published:
        if value <= version_tuple(other):
            raise ReleaseError(f"{candidate} is not greater than published {other}")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def read_signature(path: Path) -> str:
    value = path.read_text().strip()
    try:
        decoded = base64.b64decode(value, validate=True)
    except ValueError as error:
        raise ReleaseError("updater signature asset is not valid Base64") from error
    if b"untrusted comment: signature from tauri secret key" not in decoded:
        raise ReleaseError("updater signature is not a Tauri minisign envelope")
    return value


def asset_record(path: Path) -> dict[str, object]:
    if not path.is_file():
        raise ReleaseError(f"missing artifact: {path.name}")
    return {"name": path.name, "bytes": path.stat().st_size, "sha256": sha256(path)}


def manifest(
    version: str, tag: str, signature: str, notes: str, pub_date: str
) -> dict[str, object]:
    version_tuple(version)
    parse_candidate_tag(tag) if tag.startswith("nightly-") else require_stable_tag(tag)
    url = f"https://github.com/c9r-io/deck/releases/download/{tag}/{ARCHIVE}"
    return {
        "version": version,
        "notes": notes,
        "pub_date": pub_date,
        "platforms": {TARGET: {"signature": signature, "url": url}},
    }


def require_stable_tag(tag: str) -> str:
    match = STABLE_TAG_RE.fullmatch(tag)
    if not match:
        raise ReleaseError("Stable tag must be strict vMAJOR.MINOR.PATCH")
    return ".".join(match.groups())


def verify_manifest(data: dict[str, object], version: str, tag: str, signature: str) -> None:
    if data.get("version") != version:
        raise ReleaseError("manifest version mismatch")
    platforms = data.get("platforms")
    if not isinstance(platforms, dict) or TARGET not in platforms:
        raise ReleaseError(f"manifest is missing target {TARGET}")
    entry = platforms[TARGET]
    if not isinstance(entry, dict) or entry.get("signature") != signature:
        raise ReleaseError("manifest signature mismatch")
    expected = f"https://github.com/c9r-io/deck/releases/download/{tag}/{ARCHIVE}"
    if entry.get("url") != expected:
        raise ReleaseError("manifest archive URL is not the immutable candidate/Stable asset")
    parsed = urlparse(str(entry.get("url", "")))
    if parsed.scheme != "https" or parsed.netloc != "github.com":
        raise ReleaseError("manifest URL must use GitHub HTTPS")


def write_sums(directory: Path, names: list[str], output: Path) -> None:
    rows = []
    for name in sorted(names):
        path = directory / name
        if not path.is_file():
            raise ReleaseError(f"missing artifact: {name}")
        rows.append(f"{sha256(path)}  {name}")
    output.write_text("\n".join(rows) + "\n")


def parse_sums(path: Path) -> dict[str, str]:
    result: dict[str, str] = {}
    for line in path.read_text().splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  ([A-Za-z0-9._-]+)", line)
        if not match or match.group(2) in result:
            raise ReleaseError("SHA256SUMS has an invalid or duplicate row")
        result[match.group(2)] = match.group(1)
    return result


def verify_provenance(
    provenance: dict[str, object], tag: str, commit: str, directory: Path
) -> str:
    version, _, tag_sha = parse_candidate_tag(tag)
    if not FULL_SHA_RE.fullmatch(commit) or not commit.startswith(tag_sha):
        raise ReleaseError("candidate tag and full commit disagree")
    if provenance.get("schema") != 2 or provenance.get("app_version") != version:
        raise ReleaseError("provenance version/schema mismatch")
    if provenance.get("commit") != commit or provenance.get("candidate_tag") != tag:
        raise ReleaseError("provenance tag/commit mismatch")
    if provenance.get("bundle_identifier") != BUNDLE_ID or provenance.get("updater_target") != TARGET:
        raise ReleaseError("provenance bundle/target mismatch")
    if provenance.get("test_gate") != "passed":
        raise ReleaseError("candidate test gate did not pass")
    if provenance.get("updater_key_epoch") not in UPDATER_KEY_EPOCHS:
        raise ReleaseError("candidate updater signing key is unknown")
    verification = provenance.get("verification")
    if not isinstance(verification, dict) or any(
        verification.get(key) != "passed" for key in ("codesign", "notarization", "stapler", "gatekeeper")
    ):
        raise ReleaseError("candidate Apple verification is incomplete")
    artifacts = provenance.get("artifacts")
    if not isinstance(artifacts, list) or len(artifacts) != 4:
        raise ReleaseError("provenance must describe four immutable artifacts")
    by_kind = {item.get("kind"): item for item in artifacts if isinstance(item, dict)}
    if set(by_kind) != set(IMMUTABLE_KINDS):
        raise ReleaseError("provenance artifact kinds are incomplete")
    for item in by_kind.values():
        name = item.get("name")
        if not isinstance(name, str) or Path(name).name != name:
            raise ReleaseError("unsafe provenance artifact name")
        actual = asset_record(directory / name)
        if actual["bytes"] != item.get("bytes") or actual["sha256"] != item.get("sha256"):
            raise ReleaseError(f"provenance hash/size mismatch for {name}")
    sums = parse_sums(directory / "SHA256SUMS")
    expected = {item["name"]: item["sha256"] for item in by_kind.values()}
    if sums != expected:
        raise ReleaseError("SHA256SUMS and provenance disagree")
    return version


def verify_release_metadata(
    release: dict[str, object], tag: str, prerelease: bool, commit: str | None = None
) -> None:
    if release.get("tagName", release.get("tag_name")) != tag:
        raise ReleaseError("Release tag mismatch")
    if bool(release.get("isDraft", release.get("draft", False))):
        raise ReleaseError("Release must be published, not draft")
    actual_pre = bool(release.get("isPrerelease", release.get("prerelease", False)))
    if actual_pre != prerelease:
        raise ReleaseError("Release prerelease state mismatch")
    if commit is not None and release.get("targetCommitish", release.get("target_commitish")) != commit:
        raise ReleaseError("Release target commit mismatch")


def release_asset_names(release: dict[str, object]) -> set[str]:
    assets = release.get("assets")
    if not isinstance(assets, list):
        raise ReleaseError("Release assets metadata is missing")
    names: set[str] = set()
    for asset in assets:
        name = asset.get("name") if isinstance(asset, dict) else asset
        if not isinstance(name, str) or name in names:
            raise ReleaseError("Release has invalid or duplicate asset metadata")
        names.add(name)
    return names


def create_provenance(
    directory: Path,
    dmg_name: str,
    version: str,
    tag: str,
    commit: str,
    run_id: str,
    run_attempt: str,
    built_at: str,
    team_id: str,
    identity: str,
    updater_key_epoch: str,
) -> dict[str, object]:
    expected_version, _, tag_sha = parse_candidate_tag(tag)
    if expected_version != version or not FULL_SHA_RE.fullmatch(commit) or not commit.startswith(tag_sha):
        raise ReleaseError("candidate version/tag/commit mismatch")
    if not re.fullmatch(r"[A-Z0-9]{6,16}", team_id):
        raise ReleaseError("public signing Team ID has an unsafe shape")
    if not identity or len(identity) > 120 or any(c in identity for c in "\r\n"):
        raise ReleaseError("public signing identity label has an unsafe shape")
    if updater_key_epoch not in UPDATER_KEY_EPOCHS:
        raise ReleaseError("unknown updater signing key epoch")
    artifacts = [
        {"kind": "dmg", **asset_record(directory / dmg_name)},
        {"kind": "archive", **asset_record(directory / ARCHIVE)},
        {"kind": "signature", **asset_record(directory / SIGNATURE)},
        {"kind": "manifest", **asset_record(directory / "candidate.json")},
    ]
    return {
        "schema": 2,
        "app_version": version,
        "commit": commit,
        "candidate_tag": tag,
        "workflow": {"run_id": run_id, "run_attempt": run_attempt, "built_at": built_at},
        "bundle_identifier": BUNDLE_ID,
        "updater_target": TARGET,
        "updater_key_epoch": updater_key_epoch,
        "test_gate": "passed",
        "signing": {"team_id": team_id, "identity": identity},
        "verification": {
            "codesign": "passed",
            "notarization": "passed",
            "stapler": "passed",
            "gatekeeper": "passed",
        },
        "artifacts": artifacts,
    }


def verify_candidate_directory(
    directory: Path,
    tag: str,
    commit: str,
    release: dict[str, object] | None = None,
) -> str:
    provenance = json.loads((directory / "provenance.json").read_text())
    version = verify_provenance(provenance, tag, commit, directory)
    signature = read_signature(directory / SIGNATURE)
    candidate = json.loads((directory / "candidate.json").read_text())
    verify_manifest(candidate, version, tag, signature)
    if release is not None:
        verify_release_metadata(release, tag, True, commit)
        required = {
            item["name"] for item in provenance["artifacts"] if isinstance(item, dict)
        } | {"SHA256SUMS", "provenance.json"}
        if not required.issubset(release_asset_names(release)):
            raise ReleaseError("candidate Release is missing required assets")
    return version


def assert_no_stable_conflict(version: str, tags: list[str], releases: list[str]) -> None:
    tag = f"v{version}"
    if tag in tags or tag in releases:
        raise ReleaseError(f"Stable tag or Release already exists: {tag}")


def assert_promotion_has_no_build_commands(path: Path) -> None:
    text = path.read_text()
    if FORBIDDEN_PROMOTION.search(text):
        raise ReleaseError("promotion workflow contains a forbidden build command")


def cli() -> int:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command", required=True)
    version = sub.add_parser("validate-version")
    version.add_argument("value")
    tag = sub.add_parser("candidate-tag")
    tag.add_argument("--version", required=True)
    tag.add_argument("--date", required=True)
    tag.add_argument("--sha", required=True)
    newer = sub.add_parser("assert-newer")
    newer.add_argument("--candidate", required=True)
    newer.add_argument("--published", action="append", default=[])
    make_manifest = sub.add_parser("manifest")
    make_manifest.add_argument("--version", required=True)
    make_manifest.add_argument("--tag", required=True)
    make_manifest.add_argument("--signature-file", type=Path, required=True)
    make_manifest.add_argument("--notes-file", type=Path, required=True)
    make_manifest.add_argument("--pub-date", required=True)
    make_manifest.add_argument("--output", type=Path, required=True)
    check_manifest = sub.add_parser("verify-manifest")
    check_manifest.add_argument("--file", type=Path, required=True)
    check_manifest.add_argument("--version", required=True)
    check_manifest.add_argument("--tag", required=True)
    check_manifest.add_argument("--signature-file", type=Path, required=True)
    sums = sub.add_parser("write-sums")
    sums.add_argument("--dir", type=Path, required=True)
    sums.add_argument("--output", type=Path, required=True)
    sums.add_argument("names", nargs="+")
    no_build = sub.add_parser("assert-no-build")
    no_build.add_argument("path", type=Path)
    provenance = sub.add_parser("provenance")
    provenance.add_argument("--dir", type=Path, required=True)
    provenance.add_argument("--dmg", required=True)
    provenance.add_argument("--version", required=True)
    provenance.add_argument("--tag", required=True)
    provenance.add_argument("--commit", required=True)
    provenance.add_argument("--run-id", required=True)
    provenance.add_argument("--run-attempt", required=True)
    provenance.add_argument("--built-at", required=True)
    provenance.add_argument("--team-id", required=True)
    provenance.add_argument("--identity", required=True)
    provenance.add_argument("--updater-key-epoch", required=True, choices=sorted(UPDATER_KEY_EPOCHS))
    provenance.add_argument("--output", type=Path, required=True)
    verify_candidate = sub.add_parser("verify-candidate")
    verify_candidate.add_argument("--dir", type=Path, required=True)
    verify_candidate.add_argument("--tag", required=True)
    verify_candidate.add_argument("--commit", required=True)
    verify_candidate.add_argument("--release-json", type=Path)
    args = parser.parse_args()
    try:
        if args.command == "validate-version":
            version_tuple(args.value)
            print(args.value)
        elif args.command == "candidate-tag":
            print(candidate_tag(args.version, args.date, args.sha))
        elif args.command == "assert-newer":
            assert_newer(args.candidate, args.published)
            print(args.candidate)
        elif args.command == "manifest":
            data = manifest(
                args.version,
                args.tag,
                read_signature(args.signature_file),
                args.notes_file.read_text(),
                args.pub_date,
            )
            args.output.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n")
        elif args.command == "verify-manifest":
            data = json.loads(args.file.read_text())
            verify_manifest(data, args.version, args.tag, read_signature(args.signature_file))
            print(args.version)
        elif args.command == "write-sums":
            write_sums(args.dir, args.names, args.output)
        elif args.command == "assert-no-build":
            assert_promotion_has_no_build_commands(args.path)
        elif args.command == "provenance":
            data = create_provenance(
                args.dir, args.dmg, args.version, args.tag, args.commit,
                args.run_id, args.run_attempt, args.built_at, args.team_id, args.identity,
                args.updater_key_epoch,
            )
            args.output.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n")
        elif args.command == "verify-candidate":
            release = json.loads(args.release_json.read_text()) if args.release_json else None
            print(verify_candidate_directory(args.dir, args.tag, args.commit, release))
        return 0
    except (OSError, ValueError, KeyError, json.JSONDecodeError, ReleaseError) as error:
        print(f"release-channels: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(cli())
