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
root="$(release_repo_root)"
# The draft must still hold the verified unsigned bytes that were signed. Every
# mutation below addresses that verified release id, never the tag.
"$SCRIPT_DIR/release-verify-draft.sh" "$directory" base >/dev/null
id=''
[[ ! -f "$root/dist/release.draft" || -L "$root/dist/release.draft" ]] ||
	id="$(awk -F= '$1=="id" {print $2}' "$root/dist/release.draft")"
[[ "$id" =~ ^[1-9][0-9]*$ ]] || {
	echo "error: verified draft id is unavailable" >&2
	exit 1
}

# Upload only assets the draft lacks; an existing name fails, nothing is
# replaced.
while IFS= read -r asset; do
	case "$asset" in
	LICENSE-* | sbom-* | storageprims-ffi-*) continue ;;
	esac
	[[ "$asset" =~ ^[A-Za-z0-9._-]+$ && -f "$directory/$asset" && ! -L "$directory/$asset" ]] || {
		echo "error: unsafe signed asset; draft may be partial" >&2
		exit 1
	}
	response="$(gh api --hostname github.com --method POST \
		-H 'Content-Type: application/octet-stream' \
		"https://uploads.github.com/repos/$STORAGEPRIMS_REPOSITORY/releases/$id/assets?name=$asset" \
		--input "$directory/$asset")" || {
		echo "error: signed asset upload failed; draft may be partial, inspect before proceeding" >&2
		exit 1
	}
	jq -e --arg name "$asset" 'select(.name == $name and .state == "uploaded")' \
		<<<"$response" >/dev/null || {
		echo "error: signed asset upload response invalid; draft may be partial" >&2
		exit 1
	}
done < <(release_signed_assets)

notes="$directory/release-notes-${tag}.md"
[[ -s "$notes" && ! -L "$notes" ]] || {
	echo "error: release notes are absent" >&2
	exit 1
}
response="$(jq -n --arg tag "$tag" --rawfile body "$notes" '{body: $body, tag_name: $tag, draft: true}' |
	gh api --hostname github.com --method PATCH "repos/$STORAGEPRIMS_REPOSITORY/releases/$id" --input -)" || {
	echo "error: release notes update failed; inspect the draft" >&2
	exit 1
}
jq -e --argjson id "$id" --arg tag "$tag" '.id == $id and .tag_name == $tag and .draft == true' <<<"$response" >/dev/null || {
	echo "error: release notes update did not leave the verified draft" >&2
	exit 1
}
# The same release, still a draft, now holds exactly the signed set.
STORAGEPRIMS_EXPECTED_DRAFT_ID="$id" \
	"$SCRIPT_DIR/release-verify-draft.sh" "$directory" signed >/dev/null
echo "[ok] signed assets uploaded; GitHub release remains draft"
