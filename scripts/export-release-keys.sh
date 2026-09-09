#!/usr/bin/env bash
# Export explicit public verification material and prove it verifies this cut.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=release-common.sh
# shellcheck disable=SC1091
source "$SCRIPT_DIR/release-common.sh"

directory="${1:-dist/release}"
require_release_guard 1 >/dev/null
release_tag >/dev/null
"$SCRIPT_DIR/validate-release-assets.sh" \
	"$directory" signed-without-keys >/dev/null
: "${STORAGEPRIMS_MINISIGN_PUB:?load the approved minisign public key}"
[[ -f "$STORAGEPRIMS_MINISIGN_PUB" && ! -L "$STORAGEPRIMS_MINISIGN_PUB" ]] || {
	echo "error: configured minisign public key is unavailable" >&2
	exit 1
}
grep -q '^untrusted comment:' "$STORAGEPRIMS_MINISIGN_PUB" || {
	echo "error: minisign public key has an unexpected format" >&2
	exit 1
}
grep -qi 'secret' "$STORAGEPRIMS_MINISIGN_PUB" && {
	echo "error: refusing public material containing a secret marker" >&2
	exit 1
}

cp "$STORAGEPRIMS_MINISIGN_PUB" "$directory/storageprims-minisign.pub"
chmod 0644 "$directory/storageprims-minisign.pub"

if [[ -n "${STORAGEPRIMS_PGP_KEY_ID:-}" || -n "${STORAGEPRIMS_GPG_HOMEDIR:-}" ]]; then
	require_complete_pgp_config
	gpg --homedir "$STORAGEPRIMS_GPG_HOMEDIR" --batch --armor \
		--export "$STORAGEPRIMS_PGP_KEY_ID" \
		>"$directory/storageprims-release-signing-key.asc"
	[[ -s "$directory/storageprims-release-signing-key.asc" ]] || {
		echo "error: PGP public-key export is empty" >&2
		exit 1
	}
fi

"$SCRIPT_DIR/verify-public-keys.sh" "$directory" >/dev/null
"$SCRIPT_DIR/verify-signatures.sh" "$directory" >/dev/null
"$SCRIPT_DIR/validate-release-assets.sh" "$directory" signed >/dev/null
echo "[ok] public verification keys exported and proven"
