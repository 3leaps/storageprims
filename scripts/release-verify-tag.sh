#!/usr/bin/env bash
# Verify the tag with an isolated keyring imported only from the committed pin.
set -euo pipefail
# shellcheck source=scripts/release-tag-common.sh
source "$(dirname "$0")/release-tag-common.sh"
cd "$tag_root"
tag_version
tag_identity
tag_selector_shape
tag_checkout
object="$(git rev-parse "refs/tags/$STORAGEPRIMS_RELEASE_TAG")"
expected="$(mktemp)"
trap 'rm -f "$expected"' EXIT
tag_expected_message >"$expected"
tag_verify_object "$object" "$expected"
"$tag_root/scripts/verify-pinned-tag.sh"
[[ "$(awk '$1=="gpg" {print $2}' keys/expected-fingerprints.txt)" == "$STORAGEPRIMS_GPG_SIGNING_FINGERPRINT" ]] || tag_die 'authorized primary differs from committed anchor'
