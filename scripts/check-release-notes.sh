#!/usr/bin/env bash
# Prove the per-cut file is exactly the matching RELEASE_NOTES section.

set -euo pipefail

tag="${1:?usage: check-release-notes.sh vX.Y.Z}"
[[ "$tag" =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] || {
	echo "error: release tag must be canonical vX.Y.Z" >&2
	exit 1
}
cut_file="docs/releases/${tag}.md"
[[ -f "$cut_file" ]] || {
	echo "error: per-cut release notes are missing" >&2
	exit 1
}

extracted="$(mktemp "${TMPDIR:-/tmp}/storageprims-release-notes.XXXXXX")"
trap 'rm -f "$extracted"' EXIT
awk -v heading="## ${tag} " '
	index($0, heading) == 1 { found = 1 }
	found && /^---$/ { exit }
	found { print }
	END { if (!found) exit 1 }
' RELEASE_NOTES.md >"$extracted"

cmp -s "$extracted" "$cut_file" || {
	echo "error: per-cut notes must exactly match their RELEASE_NOTES section" >&2
	exit 1
}
echo "[ok] per-cut release notes match the landing page"
