#!/usr/bin/env bash
# Verify minisign and, when configured, PGP signatures for both manifests.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=release-common.sh
# shellcheck disable=SC1091
source "$SCRIPT_DIR/release-common.sh"

directory="${1:-dist/release}"
release_tag >/dev/null
"$SCRIPT_DIR/verify-public-keys.sh" "$directory" >/dev/null

for manifest in SHA256SUMS SHA512SUMS; do
	[[ -f "$directory/${manifest}.minisig" ]] || {
		echo "error: required minisign signature is missing" >&2
		exit 1
	}
	minisign -Vm "$directory/$manifest" \
		-p "$directory/storageprims-minisign.pub" \
		-x "$directory/${manifest}.minisig" >/dev/null
done

pgp_public="$directory/storageprims-release-signing-key.asc"
pgp_present=0
[[ -e "$pgp_public" ]] && pgp_present=1
for manifest in SHA256SUMS SHA512SUMS; do
	[[ -e "$directory/${manifest}.asc" ]] && pgp_present=1
done
if [[ "$pgp_present" == "1" ]]; then
	for file in "$pgp_public" \
		"$directory/SHA256SUMS.asc" "$directory/SHA512SUMS.asc"; do
		[[ -f "$file" ]] || {
			echo "error: partial PGP release material is forbidden" >&2
			exit 1
		}
	done
	temporary_gpg="$(mktemp -d "${TMPDIR:-/tmp}/storageprims-gpg.XXXXXX")"
	trap 'rm -rf "$temporary_gpg"' EXIT
	chmod 0700 "$temporary_gpg"
	gpg --homedir "$temporary_gpg" --batch --import "$pgp_public" >/dev/null 2>&1
	for manifest in SHA256SUMS SHA512SUMS; do
		gpg --homedir "$temporary_gpg" --batch \
			--verify "$directory/${manifest}.asc" "$directory/$manifest" \
			>/dev/null 2>&1
	done
fi
echo "[ok] all configured signatures verified"
