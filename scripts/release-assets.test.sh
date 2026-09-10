#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
fixture="$(mktemp -d "${TMPDIR:-/tmp}/storageprims-release-assets.XXXXXX")"
payload="$(mktemp -d "${TMPDIR:-/tmp}/storageprims-release-payload.XXXXXX")"
trap 'rm -rf "$fixture" "$payload"' EXIT

export STORAGEPRIMS_RELEASE_TAG="v0.1.0"

expect_fail() {
	if "$@" >/dev/null 2>&1; then
		echo "expected failure: $*" >&2
		exit 1
	fi
}

printf 'license\n' >"$fixture/LICENSE-APACHE"
printf 'license\n' >"$fixture/LICENSE-MIT"
printf '{}\n' >"$fixture/sbom-0.1.0.cdx.json"

make_archive() {
	local platform="$1"
	local shared="$2"
	rm -rf "$payload"
	mkdir -p "$payload"
	printf 'static\n' >"$payload/libstorageprims_ffi.a"
	printf 'shared\n' >"$payload/$shared"
	printf 'header\n' >"$payload/storageprims.h"
	printf 'license\n' >"$payload/LICENSE-MIT"
	printf 'license\n' >"$payload/LICENSE-APACHE"
	tar -czf "$fixture/storageprims-ffi-0.1.0-${platform}.tar.gz" \
		-C "$payload" \
		LICENSE-APACHE LICENSE-MIT libstorageprims_ffi.a \
		"$shared" storageprims.h
}

make_archive darwin-arm64 libstorageprims_ffi.dylib
make_archive linux-amd64 libstorageprims_ffi.so
make_archive linux-arm64 libstorageprims_ffi.so
"$SCRIPT_DIR/validate-release-assets.sh" "$fixture" base >/dev/null

printf 'stale\n' >"$fixture/foreign.txt"
expect_fail "$SCRIPT_DIR/validate-release-assets.sh" "$fixture" base
rm "$fixture/foreign.txt"

rm -rf "$payload"
mkdir -p "$payload"
printf 'outside\n' >"$payload/outside"
ln -s outside "$payload/libstorageprims_ffi.a"
printf 'shared\n' >"$payload/libstorageprims_ffi.so"
printf 'header\n' >"$payload/storageprims.h"
printf 'license\n' >"$payload/LICENSE-MIT"
printf 'license\n' >"$payload/LICENSE-APACHE"
tar -czf "$fixture/storageprims-ffi-0.1.0-linux-amd64.tar.gz" \
	-C "$payload" \
	LICENSE-APACHE LICENSE-MIT libstorageprims_ffi.a \
	libstorageprims_ffi.so storageprims.h
expect_fail "$SCRIPT_DIR/validate-release-assets.sh" "$fixture" base

make_archive linux-amd64 libstorageprims_ffi.so
python3 - "$fixture/storageprims-ffi-0.1.0-linux-amd64.tar.gz" \
	"$payload" <<'PY'
import io
import sys
import tarfile

archive, payload = sys.argv[1:]
with tarfile.open(archive, "w:gz") as output:
    for name in (
        "LICENSE-APACHE",
        "LICENSE-MIT",
        "libstorageprims_ffi.a",
        "libstorageprims_ffi.so",
        "storageprims.h",
    ):
        data = b"fixture\n"
        info = tarfile.TarInfo("../escape" if name == "storageprims.h" else name)
        info.size = len(data)
        output.addfile(info, io.BytesIO(data))
PY
expect_fail "$SCRIPT_DIR/validate-release-assets.sh" "$fixture" base

make_archive linux-amd64 libstorageprims_ffi.so
printf 'notes\n' >"$fixture/release-notes-v0.1.0.md"
(
	cd "$fixture"
	printf '%s\n' \
		LICENSE-APACHE LICENSE-MIT release-notes-v0.1.0.md \
		sbom-0.1.0.cdx.json \
		storageprims-ffi-0.1.0-darwin-arm64.tar.gz \
		storageprims-ffi-0.1.0-linux-amd64.tar.gz \
		storageprims-ffi-0.1.0-linux-arm64.tar.gz |
		LC_ALL=C sort | xargs shasum -a 256 >SHA256SUMS
	printf '%s\n' \
		LICENSE-APACHE LICENSE-MIT release-notes-v0.1.0.md \
		sbom-0.1.0.cdx.json \
		storageprims-ffi-0.1.0-darwin-arm64.tar.gz \
		storageprims-ffi-0.1.0-linux-amd64.tar.gz \
		storageprims-ffi-0.1.0-linux-arm64.tar.gz |
		LC_ALL=C sort | xargs shasum -a 512 >SHA512SUMS
)
"$SCRIPT_DIR/verify-checksums.sh" "$fixture" >/dev/null
cp "$fixture/SHA256SUMS" "$fixture/SHA256SUMS.good"
printf '%s\n' "$(head -n 1 "$fixture/SHA256SUMS")" >>"$fixture/SHA256SUMS"
expect_fail "$SCRIPT_DIR/verify-checksums.sh" "$fixture"
mv "$fixture/SHA256SUMS.good" "$fixture/SHA256SUMS"
sed '1s#  #  nested/#' "$fixture/SHA256SUMS" >"$fixture/SHA256SUMS.bad"
mv "$fixture/SHA256SUMS.bad" "$fixture/SHA256SUMS"
expect_fail "$SCRIPT_DIR/verify-checksums.sh" "$fixture"

echo "[ok] exact release asset and archive tests passed"
