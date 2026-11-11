#!/usr/bin/env python3
"""Export a commit's tree as its exact blob bytes, verified file by file.

Usage: release-export-tree.py REPO COMMIT DEST

Writes every blob of COMMIT under DEST (which must not exist) straight from
`git cat-file`, with no checkout semantics: no autocrlf, eol or filter
conversion, no export-ignore or export-subst, no sparse-checkout. Then checks
that DEST holds exactly the tree's paths, that every regular file's
`git hash-object --no-filters` equals its blob id and its executable bit
matches the tree mode. Any difference refuses. Symlinks (mode 120000) and
submodules (gitlinks) are refused: packaging follows a symlink to whatever it
names, which the blob hash cannot vouch for. The export is also checked to
hold no symlink at all before anything reads it.

The export never decides which repository git uses. A tree path with a
component, at any depth, that a filesystem may treat as `.git` is refused
before anything is written, and again if one is found in the export: after
dropping Unicode format characters, NFKC normalization, casefolding and
stripping trailing dots and spaces it is `.git`, or it is an 8.3 short name
`git~N`, or it contains `:` (an NTFS stream). Every git
command runs with no inherited GIT_* variable. The commit is attached through
a fresh git directory this script creates beside DEST, named explicitly by
GIT_DIR and GIT_WORK_TREE, and the only `.git` in the export is a gitfile
naming it, written exclusively after verification, so cargo can record the
commit in .cargo_vcs_info.json.
"""

import os
import pathlib
import re
import subprocess
import sys
import tempfile
import unicodedata


def names_git_dir(component):
    """Whether a filesystem could resolve this path component to `.git`.

    At least git's own core.protectHFS/protectNTFS set: format characters
    (category Cf, which includes U+FEFF and the HFS-ignorable code points) are
    dropped, the rest is NFKC-normalized and casefolded, and trailing dots and
    spaces are stripped; the result must not be `.git` or an 8.3 short name
    `git~N`. Any `:` (an NTFS stream such as `.git::$INDEX_ALLOCATION`) is
    refused outright.
    """
    if ":" in component:
        return True
    visible = "".join(c for c in component if unicodedata.category(c) != "Cf")
    name = unicodedata.normalize("NFKC", visible).casefold().rstrip(". ")
    return name == ".git" or re.fullmatch(r"git~[0-9]+", name) is not None


def clean_env(**extra):
    """The caller's environment with every GIT_* variable removed: none of
    GIT_DIR, GIT_WORK_TREE, GIT_INDEX_FILE, GIT_COMMON_DIR, GIT_OBJECT_DIRECTORY,
    GIT_ALTERNATE_OBJECT_DIRECTORIES, GIT_CEILING_DIRECTORIES,
    GIT_DISCOVERY_ACROSS_FILESYSTEM or GIT_CONFIG_* can steer git."""
    env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
    env.update(extra)
    return env


def git(repo, *args, data=None):
    result = subprocess.run(
        ["git", "-C", repo, "-c", "core.autocrlf=false", "-c", "core.eol=lf", *args],
        input=data,
        capture_output=True,
        check=False,
        env=clean_env(),
    )
    if result.returncode != 0:
        raise SystemExit(
            f"error: git {' '.join(args)} failed: {result.stderr.decode(errors='replace').strip()}"
        )
    return result.stdout


def tree_entries(repo, commit):
    raw = git(repo, "ls-tree", "-r", "-z", "--full-tree", commit)
    entries = {}
    for record in raw.split(b"\0"):
        if not record:
            continue
        meta, path = record.split(b"\t", 1)
        mode, kind, oid = meta.decode().split()
        if kind != "blob" or mode not in ("100644", "100755"):
            raise SystemExit(
                f"error: unsupported tree entry {mode} {kind} {path.decode(errors='replace')}"
            )
        name = path.decode()
        if any(names_git_dir(part) for part in name.split("/")):
            raise SystemExit(f"error: tree path names a git directory: {name!r}")
        entries[name] = (mode, oid)
    return entries


def read_blobs(repo, oids):
    """Every blob's raw bytes in one `git cat-file --batch` call."""
    raw = git(
        repo, "cat-file", "--batch", data="".join(f"{oid}\n" for oid in oids).encode()
    )
    blobs, offset = {}, 0
    for oid in oids:
        header_end = raw.index(b"\n", offset)
        got, kind, size = raw[offset:header_end].decode().split()
        if got != oid or kind != "blob":
            raise SystemExit(f"error: unexpected object {got} {kind}")
        start = header_end + 1
        blobs[oid] = raw[start : start + int(size)]
        offset = start + int(size) + 1
    return blobs


def main():
    if len(sys.argv) != 4:
        raise SystemExit("usage: release-export-tree.py REPO COMMIT DEST")
    repo, commit, dest = sys.argv[1:]
    dest_path = pathlib.Path(dest)
    if dest_path.exists() or dest_path.is_symlink():
        raise SystemExit(f"error: {dest} already exists")
    entries = tree_entries(repo, commit)
    if not entries:
        raise SystemExit("error: the tree is empty")
    dest_path.mkdir(parents=True)
    root = dest_path.resolve()
    blobs = read_blobs(repo, sorted({oid for _, oid in entries.values()}))
    for path, (mode, oid) in sorted(entries.items()):
        target = root / path
        if root not in target.resolve().parents and target.resolve() != root:
            raise SystemExit(f"error: tree path escapes the export: {path}")
        target.parent.mkdir(parents=True, exist_ok=True)
        content = blobs[oid]
        fd = os.open(
            target,
            os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
            0o755 if mode == "100755" else 0o644,
        )
        with os.fdopen(fd, "wb") as handle:
            handle.write(content)
    verify(repo, entries, root)
    git_dir = attach_commit(repo, commit, root)
    verify(repo, entries, root, git_dir)


def gitfile_bytes(git_dir):
    return f"gitdir: {git_dir}\n".encode()


def attach_commit(repo, commit, root):
    """Make the export a git checkout of COMMIT without rewriting any file.

    Every command names the git directory and work tree explicitly; the git
    directory is created here, outside the export, so nothing in the export
    can redirect git elsewhere. The commit is fetched from the local
    repository (no network) and the index is built from its tree; read-tree
    never touches the working files, so the verified bytes stay as written.
    Last, the export gets a `.git` gitfile naming that directory, which cargo
    needs to record the commit in .cargo_vcs_info.json; since nothing passes
    --allow-dirty, cargo also refuses to package if any file differs from its
    blob: a second, independent check.
    """
    # A new directory (mkdtemp creates it exclusively), beside the export and
    # outside it; git init never runs in a discovered or existing `.git`.
    git_dir = pathlib.Path(
        tempfile.mkdtemp(prefix=f"{root.name}.gitdir.", dir=root.parent)
    ).resolve()
    explicit = clean_env(GIT_DIR=str(git_dir), GIT_WORK_TREE=str(root))

    def local(*args, env=explicit):
        result = subprocess.run(
            [
                "git",
                "-c",
                "core.autocrlf=false",
                "-c",
                "core.sparseCheckout=false",
                *args,
            ],
            capture_output=True,
            check=False,
            cwd=root,
            env=env,
        )
        if result.returncode != 0:
            raise SystemExit(
                f"error: git {' '.join(args)} in the export failed: {result.stderr.decode(errors='replace').strip()}"
            )
        return result.stdout.decode()

    full = git(repo, "rev-parse", "--verify", f"{commit}^{{commit}}").decode().strip()
    local("init", "-q")
    local("config", "core.autocrlf", "false")
    local("config", "core.sparseCheckout", "false")
    local("fetch", "-q", "--no-tags", str(pathlib.Path(repo).resolve()), full)
    local("update-ref", "HEAD", "FETCH_HEAD")
    if local("rev-parse", "HEAD").strip() != full:
        raise SystemExit("error: the export's HEAD is not the tagged commit")
    local("read-tree", "HEAD")
    local("update-index", "-q", "--refresh")
    status = local("status", "--porcelain", "--untracked-files=all", "--ignored=no")
    if status:
        raise SystemExit(f"error: the export differs from the tagged commit:\n{status}")
    # The one `.git`: a gitfile naming that directory, created exclusively
    # (O_CREAT|O_EXCL|O_NOFOLLOW) through the shared helper.
    with tempfile.NamedTemporaryFile(dir=git_dir, delete=False) as source:
        source.write(gitfile_bytes(git_dir))
    helper = pathlib.Path(__file__).resolve().parent / "write-exclusive.py"
    created = subprocess.run(
        [sys.executable, str(helper), source.name, str(root / ".git")],
        capture_output=True,
        check=False,
    )
    os.unlink(source.name)
    if created.returncode != 0:
        raise SystemExit(
            f"error: cannot write the export's gitfile: {created.stderr.decode(errors='replace').strip()}"
        )
    # What cargo will see from inside the export, discovering from the export
    # with nothing inherited: this gitfile, this git directory, the tagged
    # commit and a clean tree.
    discovered = clean_env()
    resolved = local("rev-parse", "--absolute-git-dir", env=discovered).strip()
    if pathlib.Path(resolved).resolve() != git_dir:
        raise SystemExit("error: the export does not resolve to its own git directory")
    if local("rev-parse", "HEAD", env=discovered).strip() != full:
        raise SystemExit("error: the export's HEAD is not the tagged commit")
    attached = local("status", "--porcelain", "--untracked-files=all", env=discovered)
    if attached:
        raise SystemExit("error: the attached export is not clean")
    return git_dir


def verify(repo, entries, root, git_dir=None):
    found = set()
    for current, dirs, files in os.walk(root):
        # Every name, `.git` included, is checked before anything is skipped.
        for name in dirs + files:
            path = pathlib.Path(current) / name
            if path.is_symlink():
                raise SystemExit(
                    f"error: the export contains a symlink: {path.relative_to(root)}"
                )
            # The same predicate as the tree check; only the root gitfile
            # attach_commit wrote (checked byte for byte below) is allowed.
            own_gitfile = git_dir is not None and path == root / ".git"
            if names_git_dir(name) and not own_gitfile:
                raise SystemExit(
                    f"error: the export contains a git path: {path.relative_to(root)!s}"
                )
        for name in files:
            found.add(str((pathlib.Path(current) / name).relative_to(root)))
    # The one `.git` allowed, once attached, is the gitfile attach_commit
    # wrote, byte for byte.
    if git_dir is not None:
        gitfile = root / ".git"
        if (
            gitfile.is_symlink()
            or not gitfile.is_file()
            or gitfile.read_bytes() != gitfile_bytes(git_dir)
        ):
            raise SystemExit(
                "error: the export's .git is not the gitfile this run wrote"
            )
        found.discard(".git")
    if found != set(entries):
        missing = sorted(set(entries) - found)[:5]
        extra = sorted(found - set(entries))[:5]
        raise SystemExit(
            f"error: exported file set differs from the tree (missing {missing}, extra {extra})"
        )
    paths = list(entries)
    hashed = git(
        repo,
        "hash-object",
        "--no-filters",
        "--stdin-paths",
        data="\n".join(str(root / p) for p in paths).encode() + b"\n",
    )
    for path, oid in zip(paths, hashed.decode().split()):
        if oid != entries[path][1]:
            raise SystemExit(f"error: exported {path} differs from its blob")
        executable = os.stat(root / path).st_mode & 0o111 != 0
        if executable != (entries[path][0] == "100755"):
            raise SystemExit(f"error: exported {path} has the wrong mode")


if __name__ == "__main__":
    main()
