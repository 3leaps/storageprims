#!/usr/bin/env bash
# Verify and upload the exact signed provenance set to the trusted draft.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=release-common.sh
# shellcheck disable=SC1091
source "$SCRIPT_DIR/release-common.sh"

directory="${1:-dist/release}"
require_release_guard 1 >/dev/null
tag="$(release_tag)"
"$SCRIPT_DIR/validate-release-assets.sh" "$directory" signed >/dev/null
"$SCRIPT_DIR/verify-checksums.sh" "$directory" >/dev/null
"$SCRIPT_DIR/verify-public-keys.sh" "$directory" >/dev/null
"$SCRIPT_DIR/verify-signatures.sh" "$directory" >/dev/null
assert_github_release_state release_base_assets

upload_files=()
while IFS= read -r asset; do
	case "$asset" in
	LICENSE-* | sbom-* | storageprims-ffi-*) ;;
	*) upload_files+=("$directory/$asset") ;;
	esac
done < <(release_signed_assets)

gh release upload "$tag" "${upload_files[@]}" \
	--repo "$STORAGEPRIMS_REPOSITORY" --clobber
gh release edit "$tag" --repo "$STORAGEPRIMS_REPOSITORY" \
	--notes-file "$directory/release-notes-${tag}.md"
assert_github_release_state release_signed_assets
echo "[ok] signed assets uploaded; GitHub release remains draft"
