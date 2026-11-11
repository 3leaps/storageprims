#!/usr/bin/env python3
"""Keep the license texts in every publishable crate identical to the root copies.

check            every publishable workspace member has LICENSE-MIT and
                 LICENSE-APACHE as regular files byte-identical to the
                 workspace root copies, and `cargo package --list` puts both
                 at the package root
archive CRATE    the packaged .crate carries both texts as regular files
                 byte-identical to the workspace root copies
sync             copy the root texts into every publishable member (explicit
                 maintenance step; check and archive never rewrite files)

Publishable members are discovered from Cargo metadata, so a new crate is
covered without editing this script.
"""

import json
import os
import pathlib
import stat
import subprocess
import sys
import tarfile

if sys.version_info < (3, 11):
    sys.exit("error: license checks require Python 3.11 or newer")

LICENSES = ("LICENSE-MIT", "LICENSE-APACHE")


def metadata(root):
    return json.loads(
        subprocess.check_output(
            ["cargo", "metadata", "--no-deps", "--format-version", "1", "--locked"],
            cwd=root,
        )
    )


def canonical(workspace_root):
    texts = {}
    for name in LICENSES:
        path = workspace_root / name
        if path.is_symlink() or not path.is_file():
            raise ValueError(f"workspace root {name} must be a regular file")
        texts[name] = path.read_bytes()
        if not texts[name].strip():
            raise ValueError(f"workspace root {name} is empty")
    return texts


def publishable(meta):
    members = set(meta["workspace_members"])
    found = []
    for package in meta["packages"]:
        if package["id"] in members and package.get("publish") != []:
            found.append(
                (package["name"], pathlib.Path(package["manifest_path"]).parent)
            )
    if not found:
        raise ValueError("no publishable workspace members found")
    return sorted(found)


def check(root):
    meta = metadata(root)
    workspace_root = pathlib.Path(meta["workspace_root"])
    texts = canonical(workspace_root)
    errors = []
    crates = publishable(meta)
    for name, crate_dir in crates:
        for license_name in LICENSES:
            path = crate_dir / license_name
            try:
                mode = os.lstat(path).st_mode
            except FileNotFoundError:
                errors.append(f"{name}: {license_name} is missing")
                continue
            if not stat.S_ISREG(mode):
                errors.append(f"{name}: {license_name} is not a regular file")
            elif path.read_bytes() != texts[license_name]:
                errors.append(
                    f"{name}: {license_name} differs from the workspace root copy"
                )
        listed = subprocess.run(
            ["cargo", "package", "--list", "--locked", "--allow-dirty", "-p", name],
            cwd=workspace_root,
            capture_output=True,
            text=True,
        )
        if listed.returncode != 0:
            errors.append(
                f"{name}: cargo package --list failed: {listed.stderr.strip()}"
            )
            continue
        entries = set(listed.stdout.splitlines())
        for license_name in LICENSES:
            if license_name not in entries:
                errors.append(f"{name}: {license_name} is not in the package root")
    if errors:
        raise ValueError("\n".join(errors))
    print(f"[ok] License texts match in {len(crates)} publishable crates")


def check_archive(root, archive):
    texts = canonical(pathlib.Path(metadata(root)["workspace_root"]))
    archive = pathlib.Path(archive)
    if not archive.name.endswith(".crate"):
        raise ValueError(f"{archive}: not a .crate archive")
    prefix = archive.name.removesuffix(".crate")
    with tarfile.open(archive) as packaged:
        for license_name in LICENSES:
            member_name = f"{prefix}/{license_name}"
            try:
                member = packaged.getmember(member_name)
            except KeyError:
                raise ValueError(f"{archive.name}: {license_name} is missing") from None
            if not member.isreg():
                raise ValueError(
                    f"{archive.name}: {license_name} is not a regular file"
                )
            if packaged.extractfile(member).read() != texts[license_name]:
                raise ValueError(
                    f"{archive.name}: {license_name} differs from the workspace root copy"
                )
    print(f"[ok] {archive.name} carries both license texts")


def sync(root):
    meta = metadata(root)
    texts = canonical(pathlib.Path(meta["workspace_root"]))
    for name, crate_dir in publishable(meta):
        for license_name in LICENSES:
            path = crate_dir / license_name
            if path.is_symlink():
                path.unlink()
            if not path.exists() or path.read_bytes() != texts[license_name]:
                path.write_bytes(texts[license_name])
                print(f"updated {name}/{license_name}")


def main(argv):
    default_root = pathlib.Path(__file__).resolve().parent.parent
    root = pathlib.Path(os.environ.get("CRATE_LICENSES_ROOT", default_root))
    if argv == ["check"]:
        check(root)
    elif len(argv) == 2 and argv[0] == "archive":
        check_archive(root, argv[1])
    elif argv == ["sync"]:
        sync(root)
    else:
        raise ValueError("usage: crate-licenses.py check | archive CRATE | sync")


if __name__ == "__main__":
    try:
        main(sys.argv[1:])
    except (
        ValueError,
        OSError,
        subprocess.CalledProcessError,
        tarfile.TarError,
    ) as error:
        sys.exit(f"error: {error}")
