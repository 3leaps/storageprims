#!/usr/bin/env bash
# Download the exact unsigned CI asset set from the trusted draft release.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=release-common.sh
# shellcheck disable=SC1091
source "$SCRIPT_DIR/release-common.sh"

destination="${1:-dist/release}"
require_release_guard 1 >/dev/null
tag="$(release_tag)"
assert_github_release_state release_base_assets

root="$(release_repo_root)"
expected="$root/dist/release"
if [[ "$destination" != "$expected" && "$destination" != "dist/release" ]]; then
	echo "error: release download destination must be repo-owned dist/release" >&2
	exit 1
fi
destination="$expected"
mkdir -p "$destination"
if find "$destination" -mindepth 1 -maxdepth 1 -print -quit | grep -q .; then
	echo "error: release download requires an empty staging directory" >&2
	exit 1
fi

args=()
while IFS= read -r asset; do
	args+=(--pattern "$asset")
done < <(release_base_assets)
gh release download "$tag" --repo "$STORAGEPRIMS_REPOSITORY" \
	--dir "$destination" "${args[@]}"
"$SCRIPT_DIR/validate-release-assets.sh" "$destination" base >/dev/null
echo "[ok] exact unsigned draft assets downloaded"
