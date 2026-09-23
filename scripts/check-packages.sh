#!/usr/bin/env bash
# Verify publishable crate archives without publishing.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
VERSION_FILE="$PROJECT_ROOT/VERSION"

version=$(tr -d '[:space:]' <"$VERSION_FILE")
"$SCRIPT_DIR/release-crates.py" check
mapfile_crates() {
	while IFS= read -r crate; do crates+=("$crate"); done < <("$SCRIPT_DIR/release-crates.py" list)
}
crates=()
mapfile_crates
temp_root="${RUNNER_TEMP:-${TMPDIR:-/tmp}}"
package_target=$(mktemp -d "$temp_root/storageprims-package.XXXXXX")
trap 'rm -rf "$package_target"' EXIT

export CARGO_TARGET_DIR="$package_target"

cd "$PROJECT_ROOT"
package_args=()
patch_args=()
expected=()
for crate in "${crates[@]}"; do
	package_args+=(-p "$crate")
	if [[ "$crate" != "${crates[${#crates[@]} - 1]}" ]]; then
		patch_args+=(--config "patch.crates-io.${crate}.path=\"crates/${crate}\"")
	fi
	expected+=("$crate-$version.crate")
done
cargo package "${package_args[@]}" --locked ${patch_args[@]+"${patch_args[@]}"}

package_dir="$CARGO_TARGET_DIR/package"
actual=()
while IFS= read -r archive; do
	actual+=("$(basename "$archive")")
done < <(find "$package_dir" -maxdepth 1 -type f -name 'storageprims-*.crate' | sort)

sorted_expected=()
while IFS= read -r archive; do
	sorted_expected+=("$archive")
done < <(printf '%s\n' "${expected[@]}" | LC_ALL=C sort)
if [[ "${actual[*]}" != "${sorted_expected[*]}" ]]; then
	printf '[ERROR] package artifacts differ from the expected publishable crates\n' >&2
	printf 'expected: %s\n' "${sorted_expected[*]}" >&2
	printf 'actual:   %s\n' "${actual[*]}" >&2
	exit 1
fi

for crate in "${crates[@]}"; do
	archive="$package_dir/$crate-$version.crate"
	manifest_path="$crate-$version/Cargo.toml"
	python3 - "$archive" "$manifest_path" "$version" <<'PY'
import sys
import tarfile
if sys.version_info < (3, 11):
    sys.exit("error: package inspection requires Python 3.11 or newer")
import tomllib

archive, manifest_path, version = sys.argv[1:]
with tarfile.open(archive) as packaged:
    manifest = tomllib.loads(packaged.extractfile(manifest_path).read().decode())

def check_dependencies(sections):
    for section in ("dependencies", "dev-dependencies", "build-dependencies"):
        for name, spec in sections.get(section, {}).items():
            if name.startswith("storageprims-"):
                assert isinstance(spec, dict) and spec.get("version") == version and "path" not in spec, (name, spec)

check_dependencies(manifest)
for target in manifest.get("target", {}).values():
    check_dependencies(target)
PY
done

printf '[ok] Verified package archives: %s\n' "${expected[*]}"
