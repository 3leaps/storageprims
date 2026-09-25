#!/usr/bin/env bash
# Verify an annotated tag using only the committed public pin, in an isolated keyring.
set -euo pipefail
die() {
	echo "error: $*" >&2
	exit 1
}
tag="${STORAGEPRIMS_RELEASE_TAG:-}"
[[ "$tag" =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] || die 'canonical release tag required'
pin=docs/security/release-signing-keys.asc
anchors=keys/expected-fingerprints.txt
[[ -s "$pin" && ! -L "$pin" && -s "$anchors" && ! -L "$anchors" ]] || die 'committed public pin and anchors required'
"$(dirname "$0")/validate-release-anchors.sh" >/dev/null
object="$(git rev-parse "refs/tags/$tag" 2>/dev/null)" || die 'tag absent'
[[ "$(git cat-file -t "$object")" == tag ]] || die 'annotated tag required'
[[ "$(git rev-parse "${object}^{}")" == "$(git rev-parse HEAD)" ]] || die 'tag target must be HEAD'
[[ "$(git cat-file tag "$object" | sed -n 's/^tag //p' | head -1)" == "$tag" ]] || die 'tag name mismatch'
tagger="$(git cat-file tag "$object" | sed -n 's/^tagger \(.*\) [0-9][0-9]* [+-][0-9][0-9][0-9][0-9]$/\1/p' | head -1)"
[[ "$tagger" == "$(cat config/release/tagger-identity.txt)" ]] || die 'infosec tagger required'
primary="$(awk '$1=="gpg" {print $2}' "$anchors")"
[[ "$primary" =~ ^[0-9A-F]{40}$ ]] || die 'primary anchor malformed'
keyring="$(mktemp -d)"
trap 'rm -rf "$keyring"' EXIT
chmod 700 "$keyring"
GNUPGHOME="$keyring" gpg --batch --quiet --import "$pin" || die 'pin import failed'
[[ "$(GNUPGHOME="$keyring" gpg --batch --with-colons --list-keys | awk -F: '$1=="pub" {count++} END {print count+0}')" == 1 ]] || die 'pin must hold exactly one primary key'
pinned_primary="$(GNUPGHOME="$keyring" gpg --batch --with-colons --fingerprint --list-keys | awk -F: '$1=="fpr" {print $10;exit}')"
[[ "$pinned_primary" == "$primary" ]] || die 'pin does not match primary anchor'
GNUPGHOME="$keyring" git -c gpg.program=gpg verify-tag --raw "$object" >"$keyring/status" 2>&1 || {
	awk '$1=="[GNUPG:]" {print "signature status:", $2}' "$keyring/status" >&2
	die 'tag signature invalid under pin'
}
awk -v primary="$primary" -v selector="${STORAGEPRIMS_PGP_KEY_ID:-}" '
  $1=="[GNUPG:]" && $2=="VALIDSIG" {
    valid++; subkey=$3; signer_primary=$NF
  }
  $1=="[GNUPG:]" && $2=="GOODSIG" {good++}
  $1=="[GNUPG:]" && $2 ~ /^(EXPKEYSIG|EXPSIG|REVKEYSIG|KEYREVOKED|BADSIG|ERRSIG)$/ {bad++}
  END {
    if (bad || good!=1 || valid!=1 || signer_primary!=primary || subkey==primary) exit 1
    if (selector!="" && (selector !~ /^[0-9A-F]{40}!$/ || subkey "!" != selector)) exit 1
  }
' "$keyring/status" || die 'signing subkey, expiry or revocation check failed'
echo '[ok] signed tag verified against committed public pin'
