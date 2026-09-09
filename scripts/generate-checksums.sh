#!/usr/bin/env bash
# Generate exact SHA256 and SHA512 manifests for one release.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=release-common.sh
# shellcheck disable=SC1091
source "$SCRIPT_DIR/release-common.sh"

directory="${1:-dist/release}"
require_release_guard 1 >/dev/null
release_tag >/dev/null
"$SCRIPT_DIR/validate-release-assets.sh" "$directory" signable >/dev/null

mapfile_compat() {
	local output_name="$1"
	shift
	local values=()
	while IFS= read -r value; do values+=("$value"); done < <("$@")
	eval "$output_name=(\"\${values[@]}\")"
}

assets=()
mapfile_compat assets release_signable_assets
(
	cd "$directory"
	printf '%s\n' "${assets[@]}" | LC_ALL=C sort | xargs shasum -a 256 >SHA256SUMS
	printf '%s\n' "${assets[@]}" | LC_ALL=C sort | xargs shasum -a 512 >SHA512SUMS
)
"$SCRIPT_DIR/validate-release-assets.sh" "$directory" checksummed >/dev/null

echo "[ok] exact SHA256 and SHA512 manifests generated"
