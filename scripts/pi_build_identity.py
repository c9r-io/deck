#!/usr/bin/env python3
"""Which build a piece of Pi evidence was produced from.

Evidence names a commit; a commit does not say what the working tree held
when the run was made, and a file built from it says even less. This prints
(and `identity()` returns) what a reviewer needs to tie a run to its sources
without trusting the run's own word:

- `head`: the commit that was checked out;
- `dirty`: whether tracked files differed from it or untracked ones existed,
  and `dirty_paths`: their names (never their content);
- `diff_sha256`: SHA-256 of `git diff HEAD` (the empty diff has a fixed hash);
- `source_sha256`: one SHA-256 over the content of every file under
  `SOURCE_ROOTS` that git tracks or would track, each as
  `<path>\\0<sha256 of the file>\\n` in path order. It is the same for a clean
  checkout of a commit and for a working tree that holds exactly those bytes,
  so "the run before the commit" and "the commit" can be compared.

    pi_build_identity.py [--repo DIR]

Nothing here reads anything outside the repository, and nothing is written.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
# What decides the behaviour the Pi evidence is about: the backend, the
# observer and its packaging, the frontend, and the drivers themselves.
SOURCE_ROOTS = (
    "app/src-tauri/src",
    "app/src-tauri/resources",
    "app/src-tauri/tests",
    "app/src-tauri/build",
    "app/src-tauri/build.rs",
    "app/src-tauri/Cargo.toml",
    "app/src-tauri/Cargo.lock",
    "app/src-tauri/tauri.conf.json",
    "app/src-tauri/status-helper",
    "app/src-tauri/lifecycle-helper",
    "app/ui",
    "app/bundle.sh",
    "app/run.sh",
    "scripts/pi",
    "scripts/pi_native.py",
    "scripts/pi-live-verify.py",
    "scripts/pi_build_identity.py",
    "scripts/edr_runtime.py",
)


def _git(repo: Path, *args: str) -> bytes:
    return subprocess.run(["/usr/bin/git", "-C", str(repo), *args], capture_output=True, check=True).stdout


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def source_sha256(repo: Path = REPO) -> tuple[str, int]:
    """The hash of the sources as they are on disk now, and how many files it covers."""
    listed = _git(repo, "ls-files", "-z", "--cached", "--others", "--exclude-standard", "--", *SOURCE_ROOTS)
    paths = sorted({name for name in listed.decode().split("\0") if name})
    digest, count = hashlib.sha256(), 0
    for name in paths:
        path = repo / name
        if not path.is_file():  # deleted in the working tree, or a link to a directory
            continue
        digest.update(f"{name}\0{sha256_file(path)}\n".encode())
        count += 1
    return digest.hexdigest(), count


def identity(repo: Path = REPO) -> dict:
    status = _git(repo, "status", "--porcelain", "-z").decode()
    dirty = sorted(entry[3:] for entry in status.split("\0") if entry)
    source, files = source_sha256(repo)
    return {
        "head": _git(repo, "rev-parse", "HEAD").decode().strip(),
        "dirty": bool(dirty),
        "dirty_paths": dirty,
        "diff_sha256": hashlib.sha256(_git(repo, "diff", "HEAD")).hexdigest(),
        "source_sha256": source,
        "source_files": files,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--repo", default=str(REPO))
    args = parser.parse_args()
    print(json.dumps(identity(Path(args.repo).resolve()), indent=1))
    return 0


if __name__ == "__main__":
    sys.exit(main())
