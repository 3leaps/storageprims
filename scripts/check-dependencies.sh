#!/usr/bin/env bash
# Run license, cooling, and vulnerability dependency checks.

set -euo pipefail

command -v goneat >/dev/null 2>&1 || {
	echo "error: goneat is required (run 'make bootstrap')" >&2
	exit 1
}

command -v syft >/dev/null 2>&1 || {
	echo "error: syft is required (run 'make bootstrap')" >&2
	exit 1
}

tmp_dir=""
cleanup() {
	if [[ -n "${tmp_dir}" && -d "${tmp_dir}" ]]; then
		rm -rf "${tmp_dir}"
	fi
}
trap cleanup EXIT

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/storageprims-sbom.XXXXXX")"
tmp_sbom="${tmp_dir}/sbom.cdx.json"
syft scan dir:. \
	--exclude './.git/**' \
	--exclude './target/**' \
	--exclude './sbom/**' \
	--exclude './dist/**' \
	--exclude './bin/**' \
	--exclude './ffi-libs/**' \
	-q \
	-o "cyclonedx-json=${tmp_sbom}"

goneat dependencies \
	--licenses \
	--cooling \
	--vuln \
	--sbom-input "${tmp_sbom}" \
	--fail-on critical \
	--quiet \
	--output /dev/null

echo "[ok] dependency checks passed"
