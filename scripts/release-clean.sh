#!/usr/bin/env bash
# Remove only the repository-owned release staging directory.

set -euo pipefail

root="$(git rev-parse --show-toplevel)"
target="${1:-$root/dist/release}"
expected="$root/dist/release"

if [[ -z "$target" || "$target" == "/" || "$target" != "$expected" ]]; then
	echo "error: refusing unsafe release cleanup target" >&2
	exit 1
fi
if [[ -L "$target" ]]; then
	echo "error: refusing symlink release cleanup target" >&2
	exit 1
fi
if [[ -e "$target" && ! -d "$target" ]]; then
	echo "error: release cleanup target is not a directory" >&2
	exit 1
fi

if [[ -d "$target" ]]; then
	find "$target" -mindepth 1 -maxdepth 1 -exec rm -rf -- {} +
else
	mkdir -p "$target"
fi
echo "[ok] release staging directory is clean"
