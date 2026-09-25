#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=scripts/release-tag-common.sh
source "$(dirname "$0")/release-tag-common.sh"
cd "$tag_root"
"$tag_root/scripts/release-verify-tag.sh"
local_object="$(git rev-parse "refs/tags/$STORAGEPRIMS_RELEASE_TAG")"
remote_object="$(git ls-remote origin "refs/tags/$STORAGEPRIMS_RELEASE_TAG" | awk '{print $1}')"
[[ -n "$remote_object" && "$remote_object" == "$local_object" ]] || tag_die 'remote tag object differs from local tag'
tag_ref="$(gh api "repos/3leaps/storageprims/git/ref/tags/$STORAGEPRIMS_RELEASE_TAG")"
[[ "$(jq -r '.object.type' <<<"$tag_ref")" == tag && "$(jq -r '.object.sha' <<<"$tag_ref")" == "$local_object" ]] || tag_die 'GitHub tag ref mismatch'
tag_json="$(gh api "repos/3leaps/storageprims/git/tags/$local_object")"
jq -e --arg commit "$(git rev-parse HEAD)" --arg tag "$STORAGEPRIMS_RELEASE_TAG" \
	'.tag == $tag and .object.type == "commit" and .object.sha == $commit and .verification.verified == true and .verification.reason == "valid"' \
	<<<"$tag_json" >/dev/null || tag_die 'GitHub does not report verified tag at HEAD'
echo '[ok] remote signed tag matches local object and GitHub verification'
