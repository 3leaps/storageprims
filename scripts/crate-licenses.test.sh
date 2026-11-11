#!/usr/bin/env bash
# Negative controls for scripts/crate-licenses.py against a throwaway workspace.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GUARD="$SCRIPT_DIR/crate-licenses.py"
temp_root="${RUNNER_TEMP:-${TMPDIR:-/tmp}}"
work=$(mktemp -d "$temp_root/storageprims-licenses.XXXXXX")
trap 'rm -rf "$work"' EXIT

export CRATE_LICENSES_ROOT="$work/ws"

new_crate() {
	local name=$1 publish=$2
	mkdir -p "$CRATE_LICENSES_ROOT/$name/src"
	printf '[package]\nname = "%s"\nversion = "0.1.0"\nedition = "2021"\nlicense = "MIT OR Apache-2.0"\ndescription = "fixture"\npublish = %s\n' \
		"$name" "$publish" >"$CRATE_LICENSES_ROOT/$name/Cargo.toml"
	: >"$CRATE_LICENSES_ROOT/$name/src/lib.rs"
}

reset_workspace() {
	rm -rf "$CRATE_LICENSES_ROOT"
	mkdir -p "$CRATE_LICENSES_ROOT"
	printf '[workspace]\nresolver = "2"\nmembers = ["alpha", "beta", "internal"]\n' >"$CRATE_LICENSES_ROOT/Cargo.toml"
	printf 'fixture mit text\n' >"$CRATE_LICENSES_ROOT/LICENSE-MIT"
	printf 'fixture apache text\n' >"$CRATE_LICENSES_ROOT/LICENSE-APACHE"
	new_crate alpha true
	new_crate beta true
	new_crate internal false
	(cd "$CRATE_LICENSES_ROOT" && cargo generate-lockfile --offline -q)
	"$GUARD" sync >/dev/null
}

expect_fail() {
	local label=$1 pattern=$2
	shift 2
	local output
	if output=$("$@" 2>&1); then
		printf 'error: %s: guard unexpectedly passed\n' "$label" >&2
		exit 1
	fi
	if ! grep -q -- "$pattern" <<<"$output"; then
		printf 'error: %s: wrong refusal:\n%s\n' "$label" "$output" >&2
		exit 1
	fi
}

reset_workspace
"$GUARD" check >/dev/null
[[ ! -e "$CRATE_LICENSES_ROOT/internal/LICENSE-MIT" ]] || {
	echo "error: sync wrote into an unpublished crate" >&2
	exit 1
}

rm "$CRATE_LICENSES_ROOT/alpha/LICENSE-MIT"
expect_fail "missing copy" "alpha: LICENSE-MIT is missing" "$GUARD" check

reset_workspace
printf 'stale\n' >>"$CRATE_LICENSES_ROOT/beta/LICENSE-APACHE"
expect_fail "changed copy" "beta: LICENSE-APACHE differs" "$GUARD" check

reset_workspace
rm "$CRATE_LICENSES_ROOT/alpha/LICENSE-APACHE"
ln -s ../LICENSE-APACHE "$CRATE_LICENSES_ROOT/alpha/LICENSE-APACHE"
expect_fail "symlink copy" "alpha: LICENSE-APACHE is not a regular file" "$GUARD" check
"$GUARD" sync >/dev/null
[[ ! -L "$CRATE_LICENSES_ROOT/alpha/LICENSE-APACHE" ]] && "$GUARD" check >/dev/null

reset_workspace
printf 'exclude = ["LICENSE-MIT"]\n' >>"$CRATE_LICENSES_ROOT/beta/Cargo.toml"
expect_fail "manifest exclusion" "beta: LICENSE-MIT is not in the package root" "$GUARD" check

reset_workspace
printf '[workspace]\nresolver = "2"\nmembers = ["alpha", "beta", "internal", "gamma"]\n' >"$CRATE_LICENSES_ROOT/Cargo.toml"
new_crate gamma true
(cd "$CRATE_LICENSES_ROOT" && cargo generate-lockfile --offline -q)
expect_fail "new publishable member" "gamma: LICENSE-MIT is missing" "$GUARD" check

reset_workspace
rm "$CRATE_LICENSES_ROOT/LICENSE-MIT"
expect_fail "missing root text" "workspace root LICENSE-MIT" "$GUARD" check

# Archive checks: build .crate-shaped tarballs directly.
reset_workspace
make_archive() {
	python3 -B - "$CRATE_LICENSES_ROOT" "$work/$1" "$2" <<'PY'
import io
import pathlib
import sys
import tarfile

root, archive, variant = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]), sys.argv[3]
prefix = archive.name.removesuffix(".crate")
with tarfile.open(archive, "w:gz") as tar:
    for name in ("LICENSE-MIT", "LICENSE-APACHE"):
        data = (root / name).read_bytes()
        info = tarfile.TarInfo(f"{prefix}/{name}")
        if variant == "missing" and name == "LICENSE-MIT":
            continue
        if variant == "changed" and name == "LICENSE-APACHE":
            data += b"stale\n"
        if variant == "symlink" and name == "LICENSE-MIT":
            info.type = tarfile.SYMTYPE
            info.linkname = "../LICENSE-MIT"
            tar.addfile(info)
            continue
        info.size = len(data)
        tar.addfile(info, io.BytesIO(data))
PY
}
make_archive alpha-0.1.0.crate good
"$GUARD" archive "$work/alpha-0.1.0.crate" >/dev/null
make_archive alpha-0.1.1.crate missing
expect_fail "archive missing" "LICENSE-MIT is missing" "$GUARD" archive "$work/alpha-0.1.1.crate"
make_archive alpha-0.1.2.crate changed
expect_fail "archive changed" "LICENSE-APACHE differs" "$GUARD" archive "$work/alpha-0.1.2.crate"
make_archive alpha-0.1.3.crate symlink
expect_fail "archive symlink" "LICENSE-MIT is not a regular file" "$GUARD" archive "$work/alpha-0.1.3.crate"

# A real cargo package archive from the fixture passes.
(cd "$CRATE_LICENSES_ROOT" && CARGO_TARGET_DIR="$work/target" cargo package --offline --locked --allow-dirty --no-verify -q -p beta)
"$GUARD" archive "$work/target/package/beta-0.1.0.crate" >/dev/null

echo "[ok] crate license guard and negative controls"
