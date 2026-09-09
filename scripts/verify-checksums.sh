#!/usr/bin/env bash
# Verify exact, duplicate-free checksum manifests and every signed input.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=release-common.sh
# shellcheck disable=SC1091
source "$SCRIPT_DIR/release-common.sh"

directory="${1:-dist/release}"
release_tag >/dev/null

expected="$(mktemp "${TMPDIR:-/tmp}/storageprims-checksum-expected.XXXXXX")"
actual="$(mktemp "${TMPDIR:-/tmp}/storageprims-checksum-actual.XXXXXX")"
trap 'rm -f "$expected" "$actual"' EXIT
release_signable_assets | LC_ALL=C sort >"$expected"

for manifest in SHA256SUMS SHA512SUMS; do
	[[ -f "$directory/$manifest" && ! -L "$directory/$manifest" ]] || {
		echo "error: missing checksum manifest" >&2
		exit 1
	}
	awk '
		NF != 2 { exit 1 }
		$2 !~ /^\*?[A-Za-z0-9][A-Za-z0-9._-]*$/ { exit 1 }
		{
			name = $2
			sub(/^\*/, "", name)
			print name
		}
	' "$directory/$manifest" | LC_ALL=C sort >"$actual" || {
		echo "error: malformed or path-bearing checksum entry" >&2
		exit 1
	}
	if [[ "$(wc -l <"$actual" | tr -d ' ')" != "$(sort -u "$actual" | wc -l | tr -d ' ')" ]]; then
		echo "error: duplicate checksum entry" >&2
		exit 1
	fi
	cmp -s "$expected" "$actual" || {
		echo "error: checksum manifest inventory mismatch" >&2
		exit 1
	}
	algorithm="${manifest#SHA}"
	algorithm="${algorithm%SUMS}"
	(cd "$directory" && shasum -a "$algorithm" -c "$manifest")
done

validate_all_ffi_archives "$directory"
echo "[ok] exact checksum manifests verified"
