#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd -P)"
dest="${1:-dist/release}"
for ext in txt ndjson; do
	source_file="$root/keys/expected-fingerprints.$ext"
	[[ -s "$source_file" && ! -L "$source_file" ]] || {
		echo "error: missing committed fingerprint pin" >&2
		exit 1
	}
	cp "$source_file" "$dest/expected-fingerprints.$ext"
done
echo '[ok] public fingerprint anchors staged'
