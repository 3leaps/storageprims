#!/usr/bin/env bash
# Maintainer-only: prove that the one draft release for the verified tag holds
# exactly the expected assets of the local release directory, byte for byte.
#
# Mode `base` (default): the unsigned base assets staged by release-fetch; used
# before signatures are uploaded and when resuming against an existing draft.
# Mode `signed`: the complete signed set; used after upload. When
# STORAGEPRIMS_EXPECTED_DRAFT_ID is set the draft must be that release. The
# creation record in dist/release.draft binds subsequent verification and mutations.

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
mode="${2:-base}"
case "$mode" in
base) expected_assets=release_base_assets ;;
signed) expected_assets=release_signed_assets ;;
*) die 'mode must be base or signed' ;;
esac
expected_id="${STORAGEPRIMS_EXPECTED_DRAFT_ID:-}"
[[ -z "$expected_id" || "$expected_id" =~ ^[1-9][0-9]*$ ]] || die 'invalid expected draft id'
require_release_guard 1 >/dev/null
tag="$(release_tag)"
root="$(release_repo_root)"
cd "$root"
[[ -z "${GH_REPO+x}" ]] || die 'GH_REPO must be unset; repository authority is fixed'
object="$(git rev-parse "refs/tags/$tag")"
commit="$(git rev-parse "refs/tags/$tag^{commit}")"
record="$root/dist/release.identity"
[[ -f "$record" && ! -L "$record" ]] || die 'staged artifact record is absent; run make release-fetch'
[[ "$(awk -F= '$1=="tag" {print $2}' "$record")" == "$tag" &&
"$(awk -F= '$1=="object" {print $2}' "$record")" == "$object" &&
"$(awk -F= '$1=="commit" {print $2}' "$record")" == "$commit" ]] ||
	die 'staged artifacts were fetched for another tag object'

draft_record="$root/dist/release.draft"
if [[ -e "$draft_record" || -L "$draft_record" ]]; then
	[[ -f "$draft_record" && ! -L "$draft_record" ]] || die 'invalid draft identity record'
	recorded_id="$(sed -n 's/^id=//p' "$draft_record")"
	[[ "$recorded_id" =~ ^[1-9][0-9]*$ ]] || die 'invalid recorded draft id'
	[[ -z "$expected_id" || "$expected_id" == "$recorded_id" ]] || die 'expected id conflicts with recorded draft id'
	expected_id="$recorded_id"
else
	die 'draft identity record is absent; recover the creation id before verification'
fi

# Drafts are listed only for a caller with push access.
[[ "$(gh api --hostname github.com "repos/$STORAGEPRIMS_REPOSITORY" --jq .permissions.push 2>/dev/null)" == true ]] ||
	die 'the GitHub identity cannot see drafts'
scratch="$(mktemp -d "${TMPDIR:-/tmp}/storageprims-draft-verify.XXXXXX")"
trap 'rm -rf "$scratch"' EXIT
gh api --hostname github.com --paginate "repos/$STORAGEPRIMS_REPOSITORY/releases?per_page=100" \
	--jq ".[] | select(.tag_name == \"$tag\") | {id, draft, target_commitish, prerelease, body}" >"$scratch/matches" ||
	die 'the release list could not be read'
[[ "$(jq -s length "$scratch/matches")" == 1 ]] || die 'expected exactly one release for the tag'
jq -e --arg commit "$commit" '.draft == true and .target_commitish == $commit and (.id | type) == "number"' \
	"$scratch/matches" >/dev/null || die 'the release for the tag is not a draft at the tag commit'
jq -e '.prerelease == false' "$scratch/matches" >/dev/null || die 'draft must not be a prerelease'
if [[ "$mode" == signed ]]; then
	notes="$directory/release-notes-${tag}.md"
	[[ -s "$notes" && ! -L "$notes" ]] || die 'signed release notes are absent'
	jq -e --rawfile notes "$notes" '.body == $notes' "$scratch/matches" >/dev/null ||
		die 'draft body differs from signed release notes'
fi
id="$(jq -r .id "$scratch/matches")"
[[ -z "$expected_id" || "$id" == "$expected_id" ]] || die 'the draft for the tag is not the verified release'

gh api --hostname github.com --paginate "repos/$STORAGEPRIMS_REPOSITORY/releases/$id/assets?per_page=100" \
	--jq '.[] | "\(.id)\t\(.name)"' >"$scratch/assets" || die 'the draft asset list could not be read'
cut -f2 "$scratch/assets" | LC_ALL=C sort >"$scratch/actual"
"$expected_assets" | LC_ALL=C sort >"$scratch/expected"
cmp -s "$scratch/expected" "$scratch/actual" || {
	diff -u "$scratch/expected" "$scratch/actual" >&2 || true
	die "draft asset inventory is not exactly the $mode set"
}
while IFS=$'\t' read -r asset_id name; do
	[[ "$asset_id" =~ ^[1-9][0-9]*$ && "$name" =~ ^[A-Za-z0-9._-]+$ ]] || die 'unsafe draft asset entry'
	[[ -f "$directory/$name" && ! -L "$directory/$name" ]] || die "staged $name is missing"
	gh api --hostname github.com -H 'Accept: application/octet-stream' \
		"repos/$STORAGEPRIMS_REPOSITORY/releases/assets/$asset_id" >"$scratch/download" ||
		die "draft asset $name could not be downloaded"
	cmp -s "$scratch/download" "$directory/$name" ||
		die "draft asset $name differs from the verified local $mode set"
done <"$scratch/assets"
printf 'id=%s\n' "$id" >"$root/dist/release.draft"
echo "[ok] draft $id holds exactly the verified $mode assets"
