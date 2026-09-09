#!/usr/bin/env bash
# Run dependency checks without importing .gitignore patterns into Syft.

set -euo pipefail

command -v goneat >/dev/null 2>&1 || {
	echo "error: goneat is required (run 'make bootstrap')" >&2
	exit 1
}

goneat dependencies \
	--licenses \
	--cooling \
	--vuln \
	--no-ignore \
	--fail-on critical \
	--quiet \
	--output /dev/null

echo "[ok] dependency checks passed"
