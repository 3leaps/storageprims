#!/usr/bin/env python3
"""Check downloaded registry archives against API checksums and tag provenance."""

import hashlib
import importlib.util
import json
import pathlib
import re
import subprocess
import sys

sys.dont_write_bytecode = True

ROOT = pathlib.Path(__file__).resolve().parent.parent


def check_archive(path, crate, version, commit, expected):
    if not isinstance(expected, str) or not re.fullmatch(r"[0-9a-f]{64}", expected):
        raise ValueError("invalid crates.io checksum")
    if hashlib.sha256(path.read_bytes()).hexdigest() != expected:
        raise ValueError(f"{crate}: downloaded archive checksum mismatch")
    spec = importlib.util.spec_from_file_location(
        "release_crates", ROOT / "scripts/release-crates.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    module.check_vcs_info(str(path), crate, version, commit, f"crates/{crate}")
    subprocess.run(
        [
            sys.executable,
            "-I",
            str(ROOT / "scripts/crate-licenses.py"),
            "archive",
            str(path),
        ],
        check=True,
    )


def main():
    path, crate, version, commit, response = sys.argv[1:]
    record = json.loads(pathlib.Path(response).read_text())["version"]
    if (
        record.get("crate") != crate
        or record.get("num") != version
        or record.get("yanked") is not False
    ):
        raise ValueError("registry API identity mismatch")
    check_archive(pathlib.Path(path), crate, version, commit, record.get("checksum"))
    print(
        f"[ok] {crate}@{version}: registry checksum, licenses and tagged commit match"
    )


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError) as error:
        sys.exit(f"error: {error}")
