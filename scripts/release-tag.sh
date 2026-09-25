#!/usr/bin/env bash
# Create and verify a signed tag locally; publication is a separate command.
set -euo pipefail
# shellcheck source=scripts/release-tag-common.sh
source "$(dirname "$0")/release-tag-common.sh"
cd "$tag_root"
tag_version
tag_identity
tag_key_selector
tag_checkout
[[ -z "$(git ls-remote --tags origin "refs/tags/$STORAGEPRIMS_RELEASE_TAG")" ]] || tag_die 'remote tag already exists'
[[ -z "$(git show-ref --tags "refs/tags/$STORAGEPRIMS_RELEASE_TAG" || true)" ]] || tag_die 'local tag already exists'
message="$(mktemp)"
trap 'rm -f "$message"' EXIT
tag_expected_message >"$message"
export GIT_COMMITTER_NAME="$STORAGEPRIMS_TAGGER_NAME" GIT_COMMITTER_EMAIL="$STORAGEPRIMS_TAGGER_EMAIL"
git tag -s -a --cleanup=verbatim -u "$STORAGEPRIMS_PGP_KEY_ID" -F "$message" "$STORAGEPRIMS_RELEASE_TAG" HEAD
"$tag_root/scripts/release-verify-tag.sh"
echo '[ok] signed tag is local only'
