#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=scripts/release-tag-common.sh
source "$(dirname "$0")/release-tag-common.sh"
cd "$tag_root"
"$tag_root/scripts/release-verify-tag.sh"
"$tag_root/scripts/release-guard-tag-ruleset.sh"
[[ -z "$(git ls-remote --tags origin "refs/tags/$STORAGEPRIMS_RELEASE_TAG")" ]] || tag_die 'remote tag already exists'
git push origin "refs/tags/$STORAGEPRIMS_RELEASE_TAG"
"$tag_root/scripts/release-verify-remote-tag.sh"
