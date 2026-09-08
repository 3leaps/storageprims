#!/usr/bin/env bash
# Verify publishable crate archives before the first staged crates.io release.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
VERSION_FILE="$PROJECT_ROOT/VERSION"

version=$(tr -d '[:space:]' <"$VERSION_FILE")
temp_root="${RUNNER_TEMP:-${TMPDIR:-/tmp}}"
package_target=$(mktemp -d "$temp_root/storageprims-package.XXXXXX")
trap 'rm -rf "$package_target"' EXIT

export CARGO_TARGET_DIR="$package_target"

cd "$PROJECT_ROOT"
cargo package \
	-p storageprims-core \
	-p storageprims-ops \
	-p storageprims-s3 \
	--locked \
	--config 'patch.crates-io.storageprims-core.path="crates/storageprims-core"'

package_dir="$CARGO_TARGET_DIR/package"
expected=(
	"storageprims-core-$version.crate"
	"storageprims-ops-$version.crate"
	"storageprims-s3-$version.crate"
)
actual=()
while IFS= read -r archive; do
	actual+=("$(basename "$archive")")
done < <(find "$package_dir" -maxdepth 1 -type f -name 'storageprims-*.crate' | sort)

if [[ "${actual[*]}" != "${expected[*]}" ]]; then
	printf '[ERROR] package artifacts differ from the expected publishable crates\n' >&2
	printf 'expected: %s\n' "${expected[*]}" >&2
	printf 'actual:   %s\n' "${actual[*]}" >&2
	exit 1
fi

for crate in storageprims-ops storageprims-s3; do
	archive="$package_dir/$crate-$version.crate"
	manifest_path="$crate-$version/Cargo.toml"
	if ! tar -xOf "$archive" "$manifest_path" | awk -v version="$version" '
		$0 == "[dependencies.storageprims-core]" {
			in_core = 1
			next
		}
		in_core && /^\[/ {
			in_core = 0
		}
		in_core && $0 == "version = \"" version "\"" {
			found_version = 1
		}
		in_core && /^path[[:space:]]*=/ {
			found_path = 1
		}
		END {
			exit !(found_version && !found_path)
		}
	'; then
		printf '[ERROR] %s has an invalid normalized storageprims-core dependency\n' "$archive" >&2
		exit 1
	fi
done

printf '[ok] Verified package archives: %s\n' "${expected[*]}"
