"""Pinned FR-5D/FR-5E model identity for the isolated quality demo."""

import gzip
import hashlib
import json
from pathlib import Path
import shutil
import urllib.request


ENGINE_COMMIT = "9271618ebbdc5d21ac4dc4df9e72beb7ce644774"
REGISTRY_URL = "https://storage.googleapis.com/moz-fx-translations-data--303e-prod-translations-data/db/models.json"
FILES = {
    "lexicalShortlist": ("lex.50.50.enzh.s2t.bin", "8575d8daa10e2dbff316dcdf8e1ce475357bcc2c92bdc63b736a2d5add22f681"),
    "model": ("model.enzh.intgemm.alphas.bin", "4e5accc141373565ddc8fa1565bceaa8d0c3482a82cab8131c719ebcc6c2157c"),
    "srcVocab": ("srcvocab.enzh.spm", "bd9b65504acc6d9726dd281f7defc2adb7c2c22d0688fe2f84697de25197c8c5"),
    "trgVocab": ("trgvocab.enzh.spm", "aded6993c36e440284d11cec3f6b8aef9c0e43188a772d80be342a713adf223d"),
}


def digest(path):
    value = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(block)
    return value.hexdigest()


def verify_model(directory):
    directory = Path(directory)
    for filename, expected in FILES.values():
        path = directory / filename
        if not path.is_file() or digest(path) != expected:
            raise ValueError(f"missing or mismatched pinned model asset: {filename}")
    return directory


def fetch_model(directory):
    """Only called by the explicit --prepare command, never by normal launch."""
    directory = Path(directory)
    if directory.exists():
        return verify_model(directory)
    with urllib.request.urlopen(REGISTRY_URL, timeout=30) as response:
        registry = json.load(response)
    released = [item for item in registry["models"]["en-zh"] if item["releaseStatus"] == "Release"]
    if len(released) != 1 or released[0]["architecture"] != "base-memory":
        raise ValueError("pinned released en-zh model identity unavailable")
    entry = released[0]
    staging = directory.with_name(directory.name + ".partial")
    if staging.exists():
        raise ValueError(f"incomplete setup exists: {staging}; inspect it before retrying")
    staging.mkdir(parents=True)
    try:
        for kind, (filename, expected) in FILES.items():
            info = entry["files"][kind]
            if Path(info["path"]).name.removesuffix(".gz") != filename:
                raise ValueError(f"registry changed the pinned {kind} filename")
            url = registry["baseUrl"].rstrip("/") + "/" + info["path"]
            compressed = staging / (filename + ".gz")
            with urllib.request.urlopen(url, timeout=120) as response, compressed.open("wb") as output:
                shutil.copyfileobj(response, output)
            with gzip.open(compressed, "rb") as source, (staging / filename).open("wb") as output:
                shutil.copyfileobj(source, output)
            compressed.unlink()
            if digest(staging / filename) != expected:
                raise ValueError(f"pinned {kind} SHA-256 mismatch")
        verify_model(staging)
        staging.rename(directory)
    except Exception:
        shutil.rmtree(staging)
        raise
    return directory
