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
[[ -s docs/security/release-signing-keys.asc && ! -L docs/security/release-signing-keys.asc ]] || tag_die 'committed public pin required'
object="$(git rev-parse "refs/tags/$STORAGEPRIMS_RELEASE_TAG")"
expected="$(tag_expected_message)"
tag_verify_object "$object" "$expected"
temp="$(mktemp -d)"
trap 'rm -rf "$temp"' EXIT
chmod 700 "$temp"
GNUPGHOME="$temp" gpg --batch --quiet --import docs/security/release-signing-keys.asc
[[ "$(GNUPGHOME="$temp" gpg --batch --with-colons --list-keys | awk -F: '$1=="pub" {count++} END {print count+0}')" == 1 ]] || tag_die 'committed pin must hold exactly one primary key'
GNUPGHOME="$temp" git -c gpg.program=gpg verify-tag --raw "$STORAGEPRIMS_RELEASE_TAG" >"$temp/status" 2>&1 || tag_die 'tag signature invalid under committed pin'
awk -v subkey="${STORAGEPRIMS_PGP_KEY_ID%!}" -v primary="$STORAGEPRIMS_GPG_SIGNING_FINGERPRINT" \
	'$1=="[GNUPG:]" && $2=="VALIDSIG" && $3==subkey && $NF==primary {found++} END {exit found==1 ? 0 : 1}' \
	"$temp/status" || tag_die 'signer fingerprint does not match committed pin'
[[ "$(GNUPGHOME="$temp" gpg --batch --with-colons --fingerprint --list-keys | awk -F: '$1=="fpr" {print $10;exit}')" == "$STORAGEPRIMS_GPG_SIGNING_FINGERPRINT" ]] || tag_die 'committed public pin differs from authorized primary'
echo '[ok] signed tag verified against committed pin'
