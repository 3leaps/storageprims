#!/usr/bin/env bash
# Reject secret or malformed key material in the release set.

set -euo pipefail

directory="${1:-dist/release}"
minisign_public="$directory/storageprims-minisign.pub"

[[ -f "$minisign_public" && ! -L "$minisign_public" ]] || {
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
if [[ -e "$pgp_public" ]]; then
	[[ -f "$pgp_public" && ! -L "$pgp_public" ]] || {
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
fi
echo "[ok] exported public keys contain public material only"
