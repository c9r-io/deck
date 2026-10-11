#!/usr/bin/env python3
"""Which build did a sidecar come from, and is it coverage-instrumented?

`app/src-tauri/binaries/` is ONE location shared by every cargo context of a
checkout (debug, release, `cargo llvm-cov`): build.rs places the context's
sidecars there for tauri `externalBin`, and every packaging path reads it
(`app/run.sh`, `scripts/package-compat-candidate`, the tauri bundler). A
package is therefore only as right as the last placement. This tool answers
the question from the files themselves, never from the build cache:

- **instrumented**: the Mach-O carries `__llvm_prf*` / `__llvm_cov*` sections
  (`-C instrument-coverage`). Such a program writes a `.profraw` file into the
  working directory of whoever runs it;
- **provenance**: its `LC_UUID` equals that of the product this checkout built
  for the expected context, `<crate>/target[/llvm-cov]/<triple>/<profile>/<bin>`.
  The linker derives the UUID from the linked image, so debug, release and
  instrumented builds differ, and code signing (ad hoc or Developer ID) leaves
  it unchanged: a signed bundle can still be compared with the build tree.

The application executable has one more shared input: the frontend build.rs
stages into `ui-dist/`, with `ui/test` for debug profiles and without it for
release profiles. Which tree was compiled in is read from the executable: the
embedded asset table names every staged file.

`check` fails closed: a file that is missing, unreadable, not Mach-O or
without a UUID never passes, and neither does an expected product that is
itself in the wrong state.

Usage:
  sidecar_provenance.py inspect FILE...
  sidecar_provenance.py check --profile debug|release [--coverage]
        (--binaries | --bundle APP | --dir DIR) [--tauri DIR] [--triple T]
        [--main-product FILE] [--no-main]

`--bundle` also checks the packaged tmux and the bundle's executable against
the profile's `deck-app` product. `--main-product` names that product; with
`--binaries` or `--dir` it adds the product's own state to the check.

Exit status: 0 every checked file is the expected build; 1 at least one is
not; 2 the question could not be asked (usage, unreadable tree).
"""

import argparse
import plistlib
import struct
import sys
from pathlib import Path

# (binary, crate directory under app/src-tauri). The same four build.rs
# builds; scripts/test_sidecar_provenance.py compares this table with
# build.rs and tauri.conf.json so the three cannot drift apart.
SIDECARS = (
    ("deck-status-helper", "status-helper"),
    ("deck-lifecycle-helper", "lifecycle-helper"),
    ("deck-mcp-runner", "mcp-runner"),
    ("deck-mcp", "mcp-adapter"),
)
# Committed, pinned by its SHA-256 (gate.yml); the same file in every context.
PREBUILT = ("tmux",)
MAIN_PRODUCT = "deck-app"
DEFAULT_TRIPLE = "aarch64-apple-darwin"

MH_MAGIC_64 = 0xFEEDFACF
FAT_MAGIC = 0xCAFEBABE
FAT_MAGIC_64 = 0xCAFEBABF
LC_SEGMENT_64 = 0x19
LC_UUID = 0x1B
INSTRUMENTATION_PREFIXES = (b"__llvm_prf", b"__llvm_cov")
# Embedded asset names of the development-only `ui/test` tree. The first is
# named by nothing but the asset table, so its presence means the tree was
# staged; a release executable must name none of them.
TEST_ASSETS = (b"/test/static.test.mjs", b"/test/wk-smoke.mjs")


class Unreadable(Exception):
    """The file cannot answer: missing, truncated or not a 64-bit Mach-O."""


def _slice(data, base, path):
    if len(data) < base + 32:
        raise Unreadable(f"{path}: truncated Mach-O header")
    magic, _cpu, _sub, _type, ncmds, sizeofcmds = struct.unpack_from("<6I", data, base)
    if magic != MH_MAGIC_64:
        raise Unreadable(f"{path}: not a little-endian 64-bit Mach-O image")
    offset = base + 32
    end = offset + sizeofcmds
    if end > len(data):
        raise Unreadable(f"{path}: load commands run past the end of the file")
    uuid = None
    sections = []
    for _ in range(ncmds):
        if offset + 8 > end:
            raise Unreadable(f"{path}: truncated load command")
        cmd, cmdsize = struct.unpack_from("<2I", data, offset)
        if cmdsize < 8 or offset + cmdsize > end:
            raise Unreadable(f"{path}: malformed load command size")
        if cmd == LC_UUID:
            if cmdsize < 24:
                raise Unreadable(f"{path}: malformed LC_UUID")
            uuid = data[offset + 8 : offset + 24].hex().upper()
        elif cmd == LC_SEGMENT_64:
            if cmdsize < 72:
                raise Unreadable(f"{path}: malformed LC_SEGMENT_64")
            (nsects,) = struct.unpack_from("<I", data, offset + 64)
            if 72 + nsects * 80 > cmdsize:
                raise Unreadable(f"{path}: sections run past their segment command")
            for index in range(nsects):
                at = offset + 72 + index * 80
                sections.append(data[at : at + 16].rstrip(b"\0"))
        offset += cmdsize
    found = sorted(
        name.decode("ascii", "replace")
        for name in sections
        if name.startswith(INSTRUMENTATION_PREFIXES)
    )
    return {"uuid": uuid, "instrumented": bool(found), "sections": found}


def inspect(path):
    """One entry per architecture slice: uuid, instrumented, section names."""
    path = Path(path)
    try:
        data = path.read_bytes()
    except OSError as error:
        raise Unreadable(f"{path}: {error.strerror or error}") from None
    if len(data) < 8:
        raise Unreadable(f"{path}: too short to be a Mach-O file")
    (magic,) = struct.unpack_from(">I", data, 0)
    if magic not in (FAT_MAGIC, FAT_MAGIC_64):
        return [_slice(data, 0, path)]
    (count,) = struct.unpack_from(">I", data, 4)
    wide = magic == FAT_MAGIC_64
    size = 32 if wide else 20
    if count == 0 or 8 + count * size > len(data):
        raise Unreadable(f"{path}: malformed universal header")
    slices = []
    for index in range(count):
        at = 8 + index * size
        (offset,) = struct.unpack_from(">Q" if wide else ">I", data, at + 8)
        slices.append(_slice(data, offset, path))
    return slices


def identity(path):
    """(uuids, instrumented) of a file; every slice must carry a UUID."""
    slices = inspect(path)
    if any(entry["uuid"] is None for entry in slices):
        raise Unreadable(f"{path}: no LC_UUID, provenance cannot be compared")
    return tuple(entry["uuid"] for entry in slices), any(
        entry["instrumented"] for entry in slices
    )


def state(instrumented):
    return "instrumented" if instrumented else "plain"


def product_path(tauri, crate, binary, triple, profile, coverage):
    target = Path(tauri) / crate / "target"
    if coverage:
        target = target / "llvm-cov"
    return target / triple / profile / binary


def compare(label, candidate, product, coverage, problems):
    """Candidate must be the product, and both in the expected state."""
    try:
        want_uuid, want_instrumented = identity(product)
    except Unreadable as error:
        problems.append(f"{label}: the expected build product cannot be read ({error})")
        return
    if want_instrumented != coverage:
        problems.append(
            f"{label}: the expected build product {product} is {state(want_instrumented)}, "
            f"this context builds {state(coverage)} sidecars"
        )
        return
    try:
        have_uuid, have_instrumented = identity(candidate)
    except Unreadable as error:
        problems.append(f"{label}: {error}")
        return
    if have_instrumented != coverage:
        problems.append(
            f"{label}: {candidate} is {state(have_instrumented)}, expected {state(coverage)}"
        )
    if have_uuid != want_uuid:
        problems.append(
            f"{label}: {candidate} (UUID {', '.join(have_uuid)}) is not this checkout's "
            f"build product {product} (UUID {', '.join(want_uuid)})"
        )


def frontend(label, executable, profile, problems):
    """The executable embeds the frontend tree its profile stages."""
    try:
        data = Path(executable).read_bytes()
    except OSError as error:
        problems.append(f"{label}: {executable}: {error.strerror or error}")
        return
    named = [name.decode() for name in TEST_ASSETS if name in data]
    if profile == "release" and named:
        problems.append(
            f"{label}: {executable} embeds the development frontend ({', '.join(named)}); "
            "a release build stages ui-dist/ without ui/test"
        )
    if profile == "debug" and TEST_ASSETS[0].decode() not in named:
        problems.append(
            f"{label}: {executable} embeds no ui/test assets; a debug build stages them "
            "for the WKWebView smoke"
        )


def check(args):
    tauri = Path(args.tauri)
    if not (tauri / "tauri.conf.json").is_file():
        print(f"{tauri} is not the app/src-tauri directory", file=sys.stderr)
        return 2
    problems = []
    main = None
    if args.binaries:
        where = tauri / "binaries"
        locate = lambda name: where / f"{name}-{args.triple}"
    elif args.dir:
        where = Path(args.dir)
        locate = lambda name: where / name
    else:
        where = Path(args.bundle) / "Contents" / "MacOS"
        locate = lambda name: where / name
        if not args.no_main:
            try:
                with open(Path(args.bundle) / "Contents" / "Info.plist", "rb") as handle:
                    main = where / plistlib.load(handle)["CFBundleExecutable"]
            except (OSError, KeyError, plistlib.InvalidFileException) as error:
                print(f"{args.bundle}: Info.plist cannot name the executable ({error})", file=sys.stderr)
                return 2
    for binary, crate in SIDECARS:
        compare(
            binary,
            locate(binary),
            product_path(tauri, crate, binary, args.triple, args.profile, args.coverage),
            args.coverage,
            problems,
        )
    if not args.binaries:
        # tmux is committed, not built: the packaged copy must be that file.
        for name in PREBUILT:
            compare(name, locate(name), tauri / "binaries" / f"{name}-{args.triple}", False, problems)
    products = []
    if args.main_product:
        products = [Path(args.main_product)]
    elif main is not None:
        target = tauri / "target"
        if args.coverage:
            target = target / "llvm-cov-target"
        products = [
            path
            for path in (
                target / args.triple / args.profile / MAIN_PRODUCT,
                target / args.profile / MAIN_PRODUCT,
            )
            if path.is_file()
        ]
        if len(products) != 1:
            problems.append(
                f"{MAIN_PRODUCT}: expected exactly one {args.profile} build product to compare "
                f"{main} with, found {len(products)}; name it with --main-product"
            )
    if len(products) == 1:
        # the product compared with itself: readable, and in this context's state
        compare(MAIN_PRODUCT, main or products[0], products[0], args.coverage, problems)
        frontend(MAIN_PRODUCT, main or products[0], args.profile, problems)
    context = f"{args.profile}{'/coverage' if args.coverage else ''}"
    if problems:
        for problem in problems:
            print(problem, file=sys.stderr)
        print(
            f"sidecar provenance FAILED for {where}: not the {context} build of this checkout. "
            "Another build context (debug, release or cargo llvm-cov) placed its sidecars "
            "after this one; build this context again, then package.",
            file=sys.stderr,
        )
        return 1
    checked = len(SIDECARS) + (0 if args.binaries else len(PREBUILT)) + len(products)
    print(f"sidecar provenance ok: {checked} files in {where} are the {context} build of this checkout")
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    commands = parser.add_subparsers(dest="command", required=True)
    one = commands.add_parser("inspect", help="print UUID and instrumentation of files")
    one.add_argument("files", nargs="+")
    two = commands.add_parser("check", help="compare placed or packaged sidecars with the build tree")
    two.add_argument("--profile", required=True, choices=("debug", "release"))
    two.add_argument("--coverage", action="store_true", help="expect the cargo llvm-cov sidecars")
    where = two.add_mutually_exclusive_group(required=True)
    where.add_argument("--binaries", action="store_true", help="check app/src-tauri/binaries")
    where.add_argument("--bundle", help="check an .app bundle (sidecars, tmux, main executable)")
    where.add_argument("--dir", help="check a directory holding the sidecars by plain name")
    two.add_argument("--tauri", default=str(Path(__file__).resolve().parent.parent / "app" / "src-tauri"))
    two.add_argument("--triple", default=DEFAULT_TRIPLE)
    two.add_argument("--main-product", help="the build product the bundle executable must be")
    two.add_argument("--no-main", action="store_true", help="do not check the bundle executable")
    args = parser.parse_args(argv)
    if args.command == "check":
        return check(args)
    status = 0
    for name in args.files:
        try:
            slices = inspect(name)
        except Unreadable as error:
            print(f"unreadable {error}")
            status = 2
            continue
        for entry in slices:
            print(f"{entry['uuid'] or 'no-uuid'} {state(entry['instrumented'])} {name}")
    return status


if __name__ == "__main__":
    sys.exit(main())
