#!/usr/bin/env bash
# require_complete_pgp_config refuses a GPG home inside the repository,
# including through a symlink, and an unavailable home.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd -P)"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
repo="$scratch/repo"
mkdir -p "$repo/scripts" "$repo/inside/gnupg" "$scratch/outside"
cp "$root/scripts/release-common.sh" "$repo/scripts/"
git -C "$repo" init -q
ln -s "$repo/inside/gnupg" "$scratch/link-to-inside"
check() {
	(
		cd "$repo"
		# shellcheck source=release-common.sh
		# shellcheck disable=SC1091
		source scripts/release-common.sh
		STORAGEPRIMS_PGP_KEY_ID=ABCDEF! STORAGEPRIMS_GPG_HOMEDIR="$1" require_complete_pgp_config
	) >"$scratch/out" 2>&1
}
refused() {
	local reason="$1" home="$2"
	if check "$home"; then
		echo "error: accepted $home, expected: $reason" >&2
		exit 1
	fi
	grep -q -- "$reason" "$scratch/out" || {
		echo "error: refused $home for another reason than '$reason'" >&2
		cat "$scratch/out" >&2
		exit 1
	}
}
check "$scratch/outside"
refused 'GPG home must be outside the repository' "$repo/inside/gnupg"
refused 'GPG home must be outside the repository' "$repo"
refused 'GPG home must be outside the repository' "$scratch/link-to-inside"
refused 'configured GPG home is unavailable' "$scratch/absent"
echo '[ok] PGP config: GPG home outside the repository (direct, root and symlinked refusals), unavailable home refused'
