#!/usr/bin/env bash
# Maintainer-only: publish the recorded, verified signed draft.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=release-common.sh
# shellcheck disable=SC1091
source "$SCRIPT_DIR/release-common.sh"
die() {
	echo "error: $*" >&2
	exit 1
}

directory="${1:-dist/release}"
require_release_guard 1 >/dev/null
tag="$(release_tag)"
root="$(release_repo_root)"
cd "$root"
[[ -z "${GH_REPO+x}" ]] || die 'GH_REPO must be unset'
commit="$(git rev-parse "refs/tags/$tag^{commit}")"
[[ -f dist/release.draft && ! -L dist/release.draft ]] || die 'recorded draft id is unavailable'
id="$(sed -n 's/^id=//p' dist/release.draft)"
[[ "$id" =~ ^[1-9][0-9]*$ ]] || die 'invalid recorded draft id'

"$SCRIPT_DIR/validate-release-assets.sh" "$directory" signed >/dev/null
"$SCRIPT_DIR/verify-checksums.sh" "$directory" >/dev/null
"$SCRIPT_DIR/verify-public-keys.sh" "$directory" >/dev/null
"$SCRIPT_DIR/verify-signatures.sh" "$directory" >/dev/null
STORAGEPRIMS_EXPECTED_DRAFT_ID="$id" "$SCRIPT_DIR/release-verify-draft.sh" "$directory" signed
"$SCRIPT_DIR/release-github-preflight.sh"
# Re-prove the annotated object immediately before the irreversible mutation.
"$SCRIPT_DIR/release-verify-remote-tag.sh"
notes="$directory/release-notes-${tag}.md"
[[ -s "$notes" && ! -L "$notes" ]] || die 'signed release notes are absent'
response="$(jq -n --arg tag "$tag" --rawfile body "$notes" '{tag_name: $tag, body: $body, draft: false, prerelease: false, make_latest: "true"}' |
	gh api --method PATCH "repos/$STORAGEPRIMS_REPOSITORY/releases/$id" --input -)" ||
	die 'publication request failed; inspect remote state before any retry'
jq -e --argjson id "$id" --arg tag "$tag" --arg commit "$commit" --rawfile body "$notes" '.id == $id and .tag_name == $tag and .target_commitish == $commit and .draft == false and .immutable == true and .prerelease == false and .body == $body' <<<"$response" >/dev/null || die 'publication response mismatch; inspect remote state'

scratch="$(mktemp -d "${TMPDIR:-/tmp}/storageprims-published.XXXXXX")"
trap 'rm -rf "$scratch"' EXIT
gh api "repos/$STORAGEPRIMS_REPOSITORY/releases/tags/$tag" >"$scratch/release" ||
	die 'published release could not be read'
jq -e --argjson id "$id" --arg tag "$tag" --arg commit "$commit" --rawfile body "$notes" '.id == $id and .tag_name == $tag and .target_commitish == $commit and .draft == false and .immutable == true and .prerelease == false and .body == $body' "$scratch/release" >/dev/null || die 'published release identity or immutability mismatch'
gh api --paginate "repos/$STORAGEPRIMS_REPOSITORY/releases/$id/assets?per_page=100" --jq '.[].name' | LC_ALL=C sort >"$scratch/actual"
release_signed_assets | LC_ALL=C sort >"$scratch/expected"
cmp -s "$scratch/expected" "$scratch/actual" || die 'published asset inventory mismatch'
echo "[ok] immutable release $tag published with the exact signed asset inventory"
