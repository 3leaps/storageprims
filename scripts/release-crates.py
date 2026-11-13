#!/usr/bin/env python3
"""Validate the ordered crates.io publication plan against Cargo metadata."""

import json
import pathlib
import subprocess
import sys

if sys.version_info < (3, 11):
    sys.exit("error: release crate checks require Python 3.11 or newer")

import tomllib

ROOT = pathlib.Path(__file__).resolve().parent.parent
PLAN = ROOT / "config/release/publishable-crates.txt"


def validate(packages, names, manifests):
    if (
        not names
        or len(names) != len(set(names))
        or any(not n.startswith("storageprims-") for n in names)
    ):
        raise ValueError("invalid or duplicate publishable crate name")
    by_name = {p["name"]: p for p in packages}
    if len(by_name) != len(packages):
        raise ValueError("duplicate workspace package")
    publishable = {p["name"] for p in packages if p.get("publish") != []}
    if set(names) != publishable:
        raise ValueError(
            f"crate list differs from publishable workspace crates: {sorted(publishable ^ set(names))}"
        )
    for name in names:
        if manifests[name]["package"].get("publish") is not True:
            raise ValueError(f"{name} must explicitly set publish = true")
        for dependency in by_name[name]["dependencies"]:
            dep_name = dependency["name"]
            if dep_name in names and names.index(dep_name) >= names.index(name):
                raise ValueError(
                    f"{name} must follow {dep_name} (including dev dependencies)"
                )
    for name in ("storageprims-ffi", "storageprims-cli"):
        if name in by_name and by_name[name].get("publish") != []:
            raise ValueError(f"{name} must remain unpublished")


def registry_checksum(crate, version, text):
    """The checksum crates.io records for crate@version, from its API JSON."""
    data = json.loads(text)
    record = data.get("version") or {}
    if record.get("crate") != crate or record.get("num") != version:
        raise ValueError(f"crates.io response does not describe {crate}@{version}")
    if record.get("yanked") is True:
        raise ValueError(f"{crate}@{version} is yanked on crates.io")
    checksum = record.get("checksum")
    if (
        not isinstance(checksum, str)
        or len(checksum) != 64
        or any(c not in "0123456789abcdef" for c in checksum)
    ):
        raise ValueError(
            f"crates.io response has no valid checksum for {crate}@{version}"
        )
    return checksum


def check_vcs_info(crate_file, crate, version, commit, path):
    """The packaged crate records the tagged commit and the crate's tree path."""
    import tarfile

    member = f"{crate}-{version}/.cargo_vcs_info.json"
    with tarfile.open(crate_file, "r:gz") as archive:
        try:
            handle = archive.extractfile(member)
        except KeyError:
            handle = None
        if handle is None:
            raise ValueError(f"{crate_file} has no .cargo_vcs_info.json")
        info = json.loads(handle.read())
    if info.get("git", {}).get("sha1") != commit:
        raise ValueError(
            f"{crate} records commit {info.get('git', {}).get('sha1')}, not {commit}"
        )
    if info.get("git", {}).get("dirty"):
        raise ValueError(f"{crate} was packaged from a dirty tree")
    if info.get("path_in_vcs") != path:
        raise ValueError(
            f"{crate} records path {info.get('path_in_vcs')!r}, not {path!r}"
        )


def main():
    if len(sys.argv) == 7 and sys.argv[1] == "vcs-info":
        check_vcs_info(*sys.argv[2:])
        return
    if len(sys.argv) == 4 and sys.argv[1] == "registry-checksum":
        print(registry_checksum(sys.argv[2], sys.argv[3], sys.stdin.read()))
        return
    names = PLAN.read_text().splitlines()
    if any(not n or n != n.strip() or n.startswith("#") for n in names):
        raise ValueError("crate list must contain one bare name per line")
    metadata = json.loads(
        subprocess.check_output(
            ["cargo", "metadata", "--no-deps", "--format-version", "1", "--locked"],
            cwd=ROOT,
        )
    )
    packages = metadata["packages"]
    manifests = {
        p["name"]: tomllib.loads(pathlib.Path(p["manifest_path"]).read_text())
        for p in packages
    }
    validate(packages, names, manifests)
    if sys.argv[1:] == ["list"]:
        print("\n".join(names))
    elif sys.argv[1:] == ["paths"]:
        by_name = {p["name"]: p for p in packages}
        for name in names:
            print(
                name,
                pathlib.Path(by_name[name]["manifest_path"]).parent.relative_to(ROOT),
            )
    elif sys.argv[1:] != ["check"]:
        raise ValueError(
            "usage: release-crates.py check|list|paths|vcs-info|registry-checksum"
        )


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError) as error:
        sys.exit(f"error: {error}")
