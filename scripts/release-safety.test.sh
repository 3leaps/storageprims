#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
fixture="$(mktemp -d "${TMPDIR:-/tmp}/storageprims-release-safety.XXXXXX")"
trap 'rm -rf "$fixture"' EXIT

expect_fail() {
	if "$@" >/dev/null 2>&1; then
		echo "expected failure: $*" >&2
		exit 1
	fi
}

root="$fixture/repo"
git init -q -b main "$root"
(
	cd "$root"
	expect_fail "$SCRIPT_DIR/release-clean.sh" /
	mkdir -p dist
	ln -s "$fixture/outside" dist/release
	expect_fail "$SCRIPT_DIR/release-clean.sh"
	[[ -L dist/release ]]
)

source_dir="$fixture/source"
libdir="$fixture/lib"
includedir="$fixture/include"
mkdir -p "$source_dir" "$libdir" "$includedir"
shared="libstorageprims_ffi.so"
[[ "$(uname -s)" == "Darwin" ]] && shared="libstorageprims_ffi.dylib"
printf 'new-static\n' >"$source_dir/libstorageprims_ffi.a"
printf 'new-shared\n' >"$source_dir/$shared"
printf 'new-header\n' >"$source_dir/storageprims.h"
printf 'old\n' >"$libdir/libstorageprims_ffi.a"
ln -s "$fixture/outside" "$libdir/$shared"
printf 'old\n' >"$includedir/storageprims.h"
"$SCRIPT_DIR/install-ffi.sh" install "$source_dir" \
	"$source_dir/storageprims.h" "$libdir" "$includedir" >/dev/null
[[ ! -L "$libdir/$shared" ]]
grep -qx new-static "$libdir/libstorageprims_ffi.a"
grep -qx new-shared "$libdir/$shared"
grep -qx new-header "$includedir/storageprims.h"
"$SCRIPT_DIR/install-ffi.sh" uninstall "$source_dir" \
	"$source_dir/storageprims.h" "$libdir" "$includedir" >/dev/null
[[ -d "$libdir" && -d "$includedir" ]]
[[ ! -e "$libdir/libstorageprims_ffi.a" && ! -e "$includedir/storageprims.h" ]]
expect_fail "$SCRIPT_DIR/install-ffi.sh" install "$source_dir" \
	"$source_dir/storageprims.h" / "$includedir"

echo "[ok] cleanup and install safety tests passed"
