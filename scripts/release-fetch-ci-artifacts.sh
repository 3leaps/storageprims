#!/usr/bin/env bash
# Maintainer-only: stage the exact unsigned artifacts of one successful tag
# workflow run for the verified release tag into an empty dist/release.
#
# The workflow holds no release authority. Its artifacts are accepted only
# from a successful push run of release.yml at the verified tag and commit,
# with a release identity that names the same tag object, and licenses that
# equal the tagged commit's.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=release-common.sh
# shellcheck disable=SC1091
source "$SCRIPT_DIR/release-common.sh"

die() {
	echo "error: $*" >&2
	exit 1
}

destination="${1:-dist/release}"
# Annotated, local object equals origin's, and tag, HEAD and origin/main are
# one commit.
require_release_guard 1 >/dev/null
tag="$(release_tag)"
root="$(release_repo_root)"
cd "$root"
[[ -z "${GH_REPO+x}" ]] || die 'GH_REPO must be unset; repository authority is fixed'
for command_name in gh jq; do
	command -v "$command_name" >/dev/null 2>&1 || die 'required release command is unavailable'
done
"$SCRIPT_DIR/verify-pinned-tag.sh" >/dev/null || die 'tag does not verify against the committed pin'
object="$(git rev-parse "refs/tags/$tag")"
commit="$(git rev-parse "refs/tags/$tag^{commit}")"
[[ "$object" =~ ^[0-9a-f]{40}$ && "$commit" =~ ^[0-9a-f]{40}$ ]] || die 'tag identity is invalid'

expected="$root/dist/release"
[[ "$destination" == "$expected" || "$destination" == dist/release ]] ||
	die 'artifact destination must be repo-owned dist/release'
destination="$expected"
[[ ! -L "$root/dist" && ! -L "$destination" && ! -L "$destination.identity" ]] ||
	die 'unsafe artifact destination'
mkdir -p "$destination"
# A record from an earlier fetch must not survive a fetch that fails.
rm -f "$destination.identity"
[[ -z "$(find "$destination" -mindepth 1 -maxdepth 1 -print -quit)" ]] ||
	die 'destination must be empty; run make release-clean first'

scratch="$(mktemp -d "${TMPDIR:-/tmp}/storageprims-ci-artifacts.XXXXXX")"
trap 'rm -rf "$scratch"' EXIT

runs="$(gh run list --repo "$STORAGEPRIMS_REPOSITORY" --workflow release.yml \
	--branch "$tag" --limit 100 --json databaseId,headSha,conclusion,event \
	--jq "[.[] | select(.headSha == \"$commit\" and .conclusion == \"success\" and .event == \"push\") | .databaseId]")" ||
	die 'workflow runs could not be listed'
run="${STORAGEPRIMS_RELEASE_RUN_ID:-}"
if [[ -n "$run" ]]; then
	[[ "$run" =~ ^[1-9][0-9]*$ ]] || die 'invalid selected run'
	jq -e --argjson run "$run" 'index($run) != null' <<<"$runs" >/dev/null ||
		die 'selected run is not a successful push run for this tag and commit'
else
	[[ "$(jq length <<<"$runs")" == 1 ]] ||
		die 'select one exact successful run with STORAGEPRIMS_RELEASE_RUN_ID'
	run="$(jq -r '.[0]' <<<"$runs")"
fi
gh api "repos/$STORAGEPRIMS_REPOSITORY/actions/runs/$run" >"$scratch/run.json" ||
	die 'workflow run could not be read'
attempt="$(jq -r .run_attempt "$scratch/run.json")"
[[ "$attempt" =~ ^[1-9][0-9]*$ ]] || die 'invalid run attempt'
jq -e --arg commit "$commit" --arg tag "$tag" '
	.status == "completed" and .conclusion == "success" and .event == "push" and
	.head_sha == $commit and .head_branch == $tag and
	.path == ".github/workflows/release.yml"' "$scratch/run.json" >/dev/null ||
	die 'workflow run identity mismatch'

names=(release-identity sbom)
while read -r platform _shared _static; do
	names+=("ffi-$platform")
done <"$root/config/release/ffi-platforms.txt"
for name in "${names[@]}"; do
	gh run download "$run" --repo "$STORAGEPRIMS_REPOSITORY" --name "$name" \
		--dir "$scratch/download/$name" || die "artifact $name could not be downloaded"
done
# A rerun during the download must not silently change the attempt.
[[ "$(gh api "repos/$STORAGEPRIMS_REPOSITORY/actions/runs/$run" --jq .run_attempt)" == "$attempt" ]] ||
	die 'run attempt changed during download'

identity="$scratch/download/release-identity/release-identity.json"
[[ -f "$identity" && ! -L "$identity" ]] || die 'artifact release identity is missing'
jq -e --arg tag "$tag" --arg object "$object" --arg commit "$commit" '
	(keys == ["commit", "object", "tag"]) and .tag == $tag and
	.object == $object and .commit == $commit' "$identity" >/dev/null ||
	die 'artifact tag object or commit mismatch'
[[ "$(find "$scratch/download" -name release-identity.json | wc -l | tr -d ' ')" == 1 ]] ||
	die 'duplicate artifact release identity'

while IFS= read -r -d '' file; do
	[[ -f "$file" && ! -L "$file" ]] || die 'unsafe artifact entry'
	name="${file##*/}"
	[[ "$name" != release-identity.json ]] || continue
	[[ "$name" =~ ^[A-Za-z0-9._-]+$ ]] || die 'unsafe artifact name'
	[[ ! -e "$destination/$name" ]] || die 'duplicate artifact basename'
	cp "$file" "$destination/$name"
done < <(find "$scratch/download" -mindepth 1 ! -type d -print0)

for license in LICENSE-MIT LICENSE-APACHE; do
	git cat-file blob "$commit:$license" >"$scratch/$license" ||
		die "$license is absent from the tagged commit"
	cmp -s "$scratch/$license" "$destination/$license" ||
		die "$license differs from the tagged commit"
done
"$SCRIPT_DIR/validate-release-assets.sh" "$destination" base >/dev/null

printf 'tag=%s\nobject=%s\ncommit=%s\nrun=%s\nattempt=%s\n' \
	"$tag" "$object" "$commit" "$run" "$attempt" >"$destination.identity"
echo "[ok] staged exact unsigned artifacts from run $run attempt $attempt"
