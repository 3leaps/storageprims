#!/usr/bin/env bash
# Reject secret or malformed key material in the release set.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/release-decernor.sh
source "$SCRIPT_DIR/release-decernor.sh"
resolve_release_decernor ceremony
directory="${1:-dist/release}"
minisign_public="$directory/storageprims-minisign.pub"

[[ -s "$minisign_public" && ! -L "$minisign_public" ]] || {
	echo "error: exported minisign public key is missing or unsafe" >&2
	exit 1
}
grep -q '^untrusted comment:' "$minisign_public" || {
	echo "error: exported minisign public key is malformed" >&2
	exit 1
}
if grep -qi 'secret' "$minisign_public"; then
	echo "error: exported minisign material contains a secret marker" >&2
	exit 1
fi

pgp_public="$directory/storageprims-release-signing-key.asc"
[[ -s "$pgp_public" && ! -L "$pgp_public" ]] || {
	echo "error: exported PGP key is unsafe" >&2
	exit 1
}
grep -q 'BEGIN PGP PUBLIC KEY BLOCK' "$pgp_public" || {
	echo "error: exported PGP key is malformed" >&2
	exit 1
}
if grep -q 'PRIVATE KEY BLOCK' "$pgp_public"; then
	echo "error: exported PGP material contains a private key" >&2
	exit 1
fi
for ext in txt ndjson; do
	[[ -s "$directory/expected-fingerprints.$ext" && ! -L "$directory/expected-fingerprints.$ext" ]] || {
		echo 'error: staged public anchor is missing or unsafe' >&2
		exit 1
	}
done
"$RELEASE_DECERNOR_BIN" fingerprint verify \
	--anchors "$directory/expected-fingerprints.txt" \
	--anchors-ndjson "$directory/expected-fingerprints.ndjson" \
	--gpg "$pgp_public" --minisign "$minisign_public" >/dev/null
echo "[ok] exported public keys contain public material only"
