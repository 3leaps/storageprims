#!/usr/bin/env python3
"""Check GitHub Actions workflows against the read-only release boundary.

Every rule applies to the decoded YAML (via `yq -o=json`), so quoting, flow
style and block scalars cannot hide a value. Shell continuations are joined
and comment lines dropped before commands are matched.

Usage: release-workflow-policy.py FILE...   (exit 1 on any violation)

Rules:
- top-level permissions are exactly {contents: read};
- a job may only narrow permissions (each value read or none);
- every action and reusable workflow is a remote owner/repo pinned to a full
  commit SHA (no local ./ or docker:// actions), and the Node 20 action SHAs
  are refused;
- every actions/checkout step sets persist-credentials to boolean false;
- every job container and service image is digest-pinned;
- no expression references secrets (the job token is read-only, so without a
  secret a workflow holds no release authority), and shells are standard;
- run scripts never create or change a GitHub release, call the API with a
  write method or fields, publish a crate, or `docker run` an image without a
  digest on the image argument itself.

Action identity is compared case-insensitively, as GitHub resolves it.
"""

import json
import re
import shlex
import subprocess
import sys

NODE20_SHAS = {
    "11d5960a326750d5838078e36cf38b85af677262",  # actions/checkout v4.4.0
    "ea165f8d65b6e75b540449e92b4886f43607fa02",  # actions/upload-artifact v4.6.2
}
PINNED_USES = re.compile(r"^[A-Za-z0-9._-]+/[A-Za-z0-9._/-]+@([0-9a-f]{40})$")
DIGEST_IMAGE = re.compile(r"^[^\s@]+@sha256:[0-9a-f]{64}$")
FORBIDDEN_RUN = re.compile(
    r"(?i)\brelease\s+(create|upload|edit|delete)\b"
    r"|/releases\b(?!/download/)"
    r"|\buploads\.github\.com\b"
    r"|\bcargo\b[^\n]*\bpublish\b"
    r"|action-gh-release"
)
# gh api writes when given a method other than GET or any request field.
GH_API = re.compile(r"(?i)(^|[\s;&|(`'\"/])gh['\"]?\s+api\b")
GH_API_WRITE = re.compile(
    r"(?i)(\s(-X|--method)[\s=]*['\"]?(POST|PATCH|PUT|DELETE)\b"
    r"|\s(-f|-F|--field|--raw-field|--input)[\s=])"
)
SECRETS = re.compile(r"\bsecrets\b")
SHELLS = {"bash", "sh", "pwsh", "powershell"}
# docker run options that take a separate value argument.
DOCKER_VALUE_OPTIONS = {
    "-v", "--volume", "-w", "--workdir", "-e", "--env", "-u", "--user",
    "--entrypoint", "--name", "--network", "--platform", "--mount",
    "--env-file", "-p", "--publish", "--label", "-l", "--cpus", "-m", "--memory",
}


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


def action_name(uses):
    return uses.split("@", 1)[0].lower() if isinstance(uses, str) else ""


def check_uses(where, uses, errors):
    if not isinstance(uses, str):
        errors.append(f"{where}: uses is not a string")
        return
    match = PINNED_USES.match(uses)
    if not match:
        errors.append(f"{where}: {uses!r} is not pinned to a full commit SHA")
    elif match.group(1) in NODE20_SHAS:
        errors.append(f"{where}: {uses!r} is a refused Node 20 action SHA")
    if "action-gh-release" in uses.lower():
        errors.append(f"{where}: {uses!r} creates GitHub releases")


def check_image(where, image, errors):
    if isinstance(image, dict):
        image = image.get("image")
    if not isinstance(image, str) or not DIGEST_IMAGE.match(image):
        errors.append(f"{where}: image {image!r} is not digest-pinned")


def docker_image(line):
    """The image argument of a `docker run` command line, or None."""
    try:
        words = shlex.split(line, comments=False, posix=True)
    except ValueError:
        return None
    for i in range(len(words) - 1):
        if words[i] == "docker" and words[i + 1] == "run":
            rest = words[i + 2:]
            j = 0
            while j < len(rest):
                word = rest[j]
                if word in DOCKER_VALUE_OPTIONS:
                    j += 2
                elif word.startswith("-"):
                    j += 1
                else:
                    return word
            return None
    return None


def strings(node):
    """Every key and string value in the decoded document."""
    if isinstance(node, dict):
        for key, value in node.items():
            yield str(key)
            yield from strings(value)
    elif isinstance(node, list):
        for value in node:
            yield from strings(value)
    elif isinstance(node, str):
        yield node


def check_shell(where, holder, errors):
    shell = ((holder or {}).get("defaults") or {}).get("run", {}).get("shell") if isinstance(holder, dict) else None
    if shell is not None and shell not in SHELLS:
        errors.append(f"{where}: default shell {shell!r} is not a standard shell")


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
    if any(SECRETS.search(text) for text in strings(doc)):
        errors.append(f"{path}: workflow references secrets")
    check_shell(path, doc, errors)
    jobs = doc.get("jobs")
    if not isinstance(jobs, dict) or not jobs:
        return errors + [f"{path}: no jobs"]
    for name, job in jobs.items():
        where = f"{path}: job {name}"
        if not isinstance(job, dict):
            errors.append(f"{where}: job is not a mapping")
            continue
        check_permissions(where, job.get("permissions"), False, errors)
        check_shell(where, job, errors)
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
                if action_name(step["uses"]) == "actions/checkout":
                    persist = (step.get("with") or {}).get("persist-credentials")
                    if persist is not False:
                        errors.append(f"{step_where}: checkout must set persist-credentials: false (got {persist!r})")
            if "shell" in step and step["shell"] not in SHELLS:
                errors.append(f"{step_where}: shell {step['shell']!r} is not a standard shell")
            run = step.get("run")
            if run is None:
                continue
            if not isinstance(run, str):
                errors.append(f"{step_where}: run is not a string")
                continue
            for line in commands(run):
                if FORBIDDEN_RUN.search(line) or (GH_API.search(line) and GH_API_WRITE.search(line)):
                    errors.append(f"{step_where}: forbidden release or publish command: {line}")
                if re.search(r"\bdocker\s+run\b", line):
                    image = docker_image(line)
                    if image is None or not DIGEST_IMAGE.match(image):
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
