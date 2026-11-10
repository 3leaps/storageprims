#!/usr/bin/env python3
"""Check GitHub Actions workflows against the read-only release boundary.

Every rule applies to the decoded YAML (via `yq -o=json`), so quoting, flow
style and block scalars cannot hide a value. Shell continuations are joined
and comment lines dropped before commands are matched.

Usage: release-workflow-policy.py FILE...   (exit 1 on any violation)

Rules:
- top-level permissions are exactly {contents: read};
- a job may only narrow permissions (each value read or none);
- every action and reusable workflow is a full commit SHA (local ./ excepted),
  and the Node 20 action SHAs are refused;
- every actions/checkout step sets persist-credentials to boolean false;
- every job container and service image is digest-pinned;
- run scripts never create or change a GitHub release, publish a crate, or
  `docker run` an image without a digest.
"""

import json
import re
import subprocess
import sys

NODE20_SHAS = {
    "11d5960a326750d5838078e36cf38b85af677262",  # actions/checkout v4.4.0
    "ea165f8d65b6e75b540449e92b4886f43607fa02",  # actions/upload-artifact v4.6.2
}
PINNED_USES = re.compile(r"^[A-Za-z0-9._-]+/[A-Za-z0-9._/-]+@([0-9a-f]{40})$")
DIGEST_IMAGE = re.compile(r"^[^\s@]+@sha256:[0-9a-f]{64}$")
FORBIDDEN_RUN = re.compile(
    r"(^|[\s;&|(`])[\\'\"]*(?:\S*/)?gh['\"]?\s+release\b"
    r"|\bapi\.github\.com/repos/\S*/releases\b"
    r"|(^|[\s'\"])repos/\S+/releases\b"
    r"|\buploads\.github\.com\b"
    r"|\bcargo\b[^\n]*\bpublish\b"
)
DIGEST_IN_TEXT = re.compile(r"@sha256:[0-9a-f]{64}")


def load(path):
    try:
        out = subprocess.run(
            ["yq", "-o=json", ".", path], check=True, capture_output=True, text=True
        ).stdout
    except FileNotFoundError:
        sys.exit("error: yq is required to check workflow policy")
    except subprocess.CalledProcessError as exc:
        sys.exit(f"error: {path}: yq could not parse the workflow: {exc.stderr.strip()}")
    return json.loads(out)


def commands(script):
    """Logical shell lines: continuations joined, comment-only lines dropped."""
    joined = re.sub(r"\\\n", " ", script)
    for line in joined.splitlines():
        stripped = line.strip()
        if stripped and not stripped.startswith("#"):
            yield stripped


def check_uses(where, uses, errors):
    if not isinstance(uses, str):
        errors.append(f"{where}: uses is not a string")
        return
    if uses.startswith("./"):
        return
    match = PINNED_USES.match(uses)
    if not match:
        errors.append(f"{where}: {uses!r} is not pinned to a full commit SHA")
    elif match.group(1) in NODE20_SHAS:
        errors.append(f"{where}: {uses!r} is a refused Node 20 action SHA")
    if "action-gh-release" in uses:
        errors.append(f"{where}: {uses!r} creates GitHub releases")


def check_image(where, image, errors):
    if isinstance(image, dict):
        image = image.get("image")
    if not isinstance(image, str) or not DIGEST_IMAGE.match(image):
        errors.append(f"{where}: image {image!r} is not digest-pinned")


def check_permissions(where, perms, top, errors):
    if top:
        if perms != {"contents": "read"}:
            errors.append(f"{where}: top-level permissions must be exactly {{contents: read}}, got {perms!r}")
        return
    if perms is None:
        return
    if not isinstance(perms, dict):
        errors.append(f"{where}: job permissions {perms!r} must be a mapping of read or none")
        return
    for scope, level in perms.items():
        if level not in ("read", "none"):
            errors.append(f"{where}: permission {scope}: {level!r} grants more than read")


def check(path):
    errors = []
    doc = load(path)
    if not isinstance(doc, dict):
        return [f"{path}: workflow is not a mapping"]
    check_permissions(f"{path}", doc.get("permissions"), True, errors)
    jobs = doc.get("jobs")
    if not isinstance(jobs, dict) or not jobs:
        return errors + [f"{path}: no jobs"]
    for name, job in jobs.items():
        where = f"{path}: job {name}"
        if not isinstance(job, dict):
            errors.append(f"{where}: job is not a mapping")
            continue
        check_permissions(where, job.get("permissions"), False, errors)
        if "uses" in job:
            check_uses(where, job["uses"], errors)
        if "container" in job:
            check_image(f"{where} container", job["container"], errors)
        for service, spec in (job.get("services") or {}).items():
            check_image(f"{where} service {service}", spec, errors)
        for index, step in enumerate(job.get("steps") or []):
            step_where = f"{where} step {index + 1}"
            if not isinstance(step, dict):
                errors.append(f"{step_where}: step is not a mapping")
                continue
            if "uses" in step:
                check_uses(step_where, step["uses"], errors)
                uses = step["uses"] if isinstance(step["uses"], str) else ""
                if uses.split("@", 1)[0] == "actions/checkout":
                    persist = (step.get("with") or {}).get("persist-credentials")
                    if persist is not False:
                        errors.append(f"{step_where}: checkout must set persist-credentials: false (got {persist!r})")
            run = step.get("run")
            if run is None:
                continue
            if not isinstance(run, str):
                errors.append(f"{step_where}: run is not a string")
                continue
            for line in commands(run):
                if FORBIDDEN_RUN.search(line):
                    errors.append(f"{step_where}: forbidden release or publish command: {line}")
                if re.search(r"\bdocker\s+run\b", line) and not DIGEST_IN_TEXT.search(line):
                    errors.append(f"{step_where}: docker run image is not digest-pinned: {line}")
    return errors


def main(paths):
    if not paths:
        sys.exit("usage: release-workflow-policy.py FILE...")
    errors = []
    for path in paths:
        errors.extend(check(path))
    for error in errors:
        print(f"error: {error}", file=sys.stderr)
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
