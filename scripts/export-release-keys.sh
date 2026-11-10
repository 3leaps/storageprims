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
# Public keys come from the verified tagged commit as inert blobs, never from
# the working tree, an environment path or a live keyring export.
commit="$(git -C "$(release_repo_root)" rev-parse --verify --quiet "refs/tags/$(release_tag)^{commit}")" || {
	echo "error: release tag commit is unavailable" >&2
	exit 1
}
export_committed_blob() {
	local path="$1" output="$2"
	[[ "$(git -C "$(release_repo_root)" cat-file -t "$commit:$path" 2>/dev/null)" == blob ]] || {
		echo "error: $path is absent from the tagged commit" >&2
		exit 1
	}
	git -C "$(release_repo_root)" cat-file blob "$commit:$path" >"$output"
	chmod 0644 "$output"
	[[ -s "$output" ]] || {
		echo "error: $path is empty in the tagged commit" >&2
		exit 1
	}
}

export_committed_blob docs/security/storageprims-minisign.pub \
	"$directory/storageprims-minisign.pub"
grep -q '^untrusted comment:' "$directory/storageprims-minisign.pub" || {
	echo "error: minisign public key has an unexpected format" >&2
	exit 1
}
grep -qi 'secret' "$directory/storageprims-minisign.pub" && {
	echo "error: refusing public material containing a secret marker" >&2
	exit 1
}

if [[ -n "${STORAGEPRIMS_PGP_KEY_ID:-}" || -n "${STORAGEPRIMS_GPG_HOMEDIR:-}" ]]; then
	require_complete_pgp_config
	export_committed_blob docs/security/release-signing-keys.asc \
		"$directory/storageprims-release-signing-key.asc"
	grep -q 'PRIVATE KEY' "$directory/storageprims-release-signing-key.asc" && {
		echo "error: refusing public material containing a private key block" >&2
		exit 1
	}
fi

"$SCRIPT_DIR/verify-public-keys.sh" "$directory" >/dev/null
"$SCRIPT_DIR/verify-signatures.sh" "$directory" >/dev/null
"$SCRIPT_DIR/validate-release-assets.sh" "$directory" signed >/dev/null
echo "[ok] public verification keys exported and proven"
