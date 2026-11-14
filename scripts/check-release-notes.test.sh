#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
set -euo pipefail

script="$(cd "$(dirname "$0")" && pwd)/check-release-notes.sh"
fixture="$(mktemp -d "${TMPDIR:-/tmp}/storageprims-notes-test.XXXXXX")"
trap 'rm -rf "$fixture"' EXIT
mkdir -p "$fixture/docs/releases"
cd "$fixture"
cat >RELEASE_NOTES.md <<'NOTES'
# Release Notes

## v1.2.3 — 2026-01-01

Sample release.

Full notes: [v1.2.3](docs/releases/v1.2.3.md)

---

## v1.2.2 — 2025-12-01

Previous release.
NOTES
cat >docs/releases/v1.2.3.md <<'NOTES'
## v1.2.3 — 2026-01-01

Sample release.

Full notes: [v1.2.3](docs/releases/v1.2.3.md)
NOTES
bash "$script" v1.2.3
# The historical no-blank-before-separator form remains accepted.
sed '/^Full notes:/{n;/^$/d;}' RELEASE_NOTES.md >compact.md
mv compact.md RELEASE_NOTES.md
bash "$script" v1.2.3
printf '\nUnexpected content.\n' >>docs/releases/v1.2.3.md
if bash "$script" v1.2.3 >/dev/null 2>&1; then
	echo 'error: mismatched per-cut content was accepted' >&2
	exit 1
fi
printf '## v1.2.4\n' >docs/releases/v1.2.4.md
if bash "$script" v1.2.4 >/dev/null 2>&1; then
	echo 'error: missing release section was accepted' >&2
	exit 1
fi
echo '[ok] release notes separator and content controls passed'
