#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(git rev-parse --show-toplevel)"
fixture="$(mktemp -d "${TMPDIR:-/tmp}/storageprims-release-assets.XXXXXX")"
payload="$(mktemp -d "${TMPDIR:-/tmp}/storageprims-release-payload.XXXXXX")"
trap 'rm -rf "$fixture" "$payload"' EXIT

version="$(cat "$root/VERSION")"
expected_platforms="$(cut -d' ' -f1 "$root/config/release/ffi-platforms.txt" | LC_ALL=C sort)"
workflow_platforms="$(awk '$1 == "platform:" { print $2 }' "$root/.github/workflows/release.yml" | LC_ALL=C sort)"
[[ "$expected_platforms" == "$workflow_platforms" ]] || {
	echo 'error: FFI release matrix differs from platform inventory' >&2
	exit 1
}
[[ "$version" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] || {
	echo "error: fixture requires a stable VERSION" >&2
	exit 1
}
export STORAGEPRIMS_RELEASE_TAG="v${version}"

expect_fail() {
	if "$@" >/dev/null 2>&1; then
		echo "expected failure: $*" >&2
		exit 1
	fi
}

printf 'license\n' >"$fixture/LICENSE-APACHE"
printf 'license\n' >"$fixture/LICENSE-MIT"
printf '{}\n' >"$fixture/sbom-${version}.cdx.json"

make_archive() {
	local platform="$1"
	local shared="$2"
	local static="$3"
	rm -rf "$payload"
	mkdir -p "$payload"
	printf 'static\n' >"$payload/$static"
	printf 'shared\n' >"$payload/$shared"
	printf 'header\n' >"$payload/storageprims.h"
	printf 'license\n' >"$payload/LICENSE-MIT"
	printf 'license\n' >"$payload/LICENSE-APACHE"
	tar -czf "$fixture/storageprims-ffi-${version}-${platform}.tar.gz" \
		-C "$payload" \
		LICENSE-APACHE LICENSE-MIT "$static" \
		"$shared" storageprims.h
}

while read -r platform shared static; do
	make_archive "$platform" "$shared" "$static"
done <"$root/config/release/ffi-platforms.txt"
"$SCRIPT_DIR/validate-release-assets.sh" "$fixture" base >/dev/null
wrong_tag="v${version%.*}.$((${version##*.} + 1))"
expect_fail env STORAGEPRIMS_RELEASE_TAG="$wrong_tag" \
	"$SCRIPT_DIR/validate-release-assets.sh" "$fixture" base

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
tar -czf "$fixture/storageprims-ffi-${version}-linux-amd64.tar.gz" \
	-C "$payload" \
	LICENSE-APACHE LICENSE-MIT libstorageprims_ffi.a \
	libstorageprims_ffi.so storageprims.h
expect_fail "$SCRIPT_DIR/validate-release-assets.sh" "$fixture" base

make_archive linux-amd64 libstorageprims_ffi.so libstorageprims_ffi.a
python3 - "$fixture/storageprims-ffi-${version}-linux-amd64.tar.gz" \
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

make_archive linux-amd64 libstorageprims_ffi.so libstorageprims_ffi.a
printf 'notes\n' >"$fixture/release-notes-${STORAGEPRIMS_RELEASE_TAG}.md"
printf 'gpg fixture\nminisign fixture\n' >"$fixture/expected-fingerprints.txt"
printf '{"fixture":true}\n' >"$fixture/expected-fingerprints.ndjson"
# shellcheck source=release-common.sh
# shellcheck disable=SC1091
source "$SCRIPT_DIR/release-common.sh"
signable_assets="$(release_signable_assets)"
(
	cd "$fixture"
	printf '%s\n' "$signable_assets" | LC_ALL=C sort | xargs shasum -a 256 >SHA256SUMS
	printf '%s\n' "$signable_assets" | LC_ALL=C sort | xargs shasum -a 512 >SHA512SUMS
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
