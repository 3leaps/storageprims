#!/usr/bin/env bash
# export-release-keys.sh publishes the public keys committed in the tagged
# commit, not the working tree, an environment path or a live keyring, and
# refuses a tagged commit that lacks them.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd -P)"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
repo="$scratch/repo"
mkdir -p "$repo/scripts" "$repo/docs/security" "$repo/dist/release"
cp "$root/scripts/export-release-keys.sh" "$repo/scripts/"
# Stub the guard, inventory and verification steps; this suite covers only
# where the exported public material comes from.
cat >"$repo/scripts/release-common.sh" <<'SH'
release_repo_root() { cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P; }
release_tag() { printf '%s\n' "$STORAGEPRIMS_RELEASE_TAG"; }
require_release_guard() { :; }
require_complete_pgp_config() { :; }
SH
for stub in validate-release-assets.sh verify-public-keys.sh verify-signatures.sh; do
	printf '#!/usr/bin/env bash\nexit 0\n' >"$repo/scripts/$stub"
	chmod +x "$repo/scripts/$stub"
done
committed_pub=$'untrusted comment: fixture release signing\nRWQfixturecommittedpublickeyAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\n'
committed_asc=$'-----BEGIN PGP PUBLIC KEY BLOCK-----\n\nfixturecommitted\n-----END PGP PUBLIC KEY BLOCK-----\n'
printf '%s' "$committed_pub" >"$repo/docs/security/storageprims-minisign.pub"
printf '%s' "$committed_asc" >"$repo/docs/security/release-signing-keys.asc"
git -C "$repo" init -q -b main
git -C "$repo" config user.name 'Fixture Author'
git -C "$repo" config user.email 'fixture@example.invalid'
git -C "$repo" add -A
git -C "$repo" commit -qm 'fixture release'
git -C "$repo" tag -am 'Fixture release' v1.2.3
# The working tree and the environment now name different material.
printf 'untrusted comment: working tree\nRWQworkingtreekey\n' >"$repo/docs/security/storageprims-minisign.pub"
printf 'untrusted comment: env path\nRWQenvpathkey\n' >"$scratch/env.pub"
export STORAGEPRIMS_RELEASE_TAG=v1.2.3 STORAGEPRIMS_MINISIGN_PUB="$scratch/env.pub"
run() { (cd "$repo" && bash scripts/export-release-keys.sh dist/release) >"$scratch/out" 2>&1; }
refused() {
	local reason="$1"
	shift
	if "$@"; then
		echo "error: accepted, expected: $reason" >&2
		cat "$scratch/out" >&2
		exit 1
	fi
	grep -q -- "$reason" "$scratch/out" || {
		echo "error: refused for another reason than '$reason'" >&2
		cat "$scratch/out" >&2
		exit 1
	}
}

# Minisign only: the committed blob is published, nothing else.
run || {
	cat "$scratch/out" >&2
	exit 1
}
[[ "$(cat "$repo/dist/release/storageprims-minisign.pub")" == "${committed_pub%$'\n'}" ]] || {
	echo 'error: exported minisign key is not the committed blob' >&2
	exit 1
}
[[ ! -e "$repo/dist/release/storageprims-release-signing-key.asc" ]]

# With PGP configured, the committed OpenPGP export is published, not a
# keyring export (no gpg is reachable from this run).
rm -f "$repo/dist/release/storageprims-minisign.pub"
mkdir -p "$scratch/nogpg"
printf '#!/usr/bin/env bash\necho "gpg must not run" >&2\nexit 1\n' >"$scratch/nogpg/gpg"
chmod +x "$scratch/nogpg/gpg"
PATH="$scratch/nogpg:$PATH" STORAGEPRIMS_PGP_KEY_ID=fixture STORAGEPRIMS_GPG_HOMEDIR="$scratch" run || {
	cat "$scratch/out" >&2
	exit 1
}
[[ "$(cat "$repo/dist/release/storageprims-release-signing-key.asc")" == "${committed_asc%$'\n'}" ]] || {
	echo 'error: exported OpenPGP key is not the committed blob' >&2
	exit 1
}

# A tagged commit without the pin is refused, whatever the working tree holds.
rm -f "$repo/dist/release/"*
git -C "$repo" rm -q --cached docs/security/storageprims-minisign.pub
git -C "$repo" commit -qm 'drop pin'
git -C "$repo" tag -fam 'Fixture release without pin' v1.2.3 >/dev/null
refused 'storageprims-minisign.pub is absent from the tagged commit' run

# A missing tag is refused.
git -C "$repo" tag -d v1.2.3 >/dev/null
refused 'release tag commit is unavailable' run

# A private key block in the committed OpenPGP export is refused.
git -C "$repo" add docs/security/storageprims-minisign.pub
printf -- '-----BEGIN PGP PRIVATE KEY BLOCK-----\nfixture\n' >"$repo/docs/security/release-signing-keys.asc"
git -C "$repo" add docs/security/release-signing-keys.asc
git -C "$repo" commit -qm 'bad export'
git -C "$repo" tag -am 'Fixture release with bad export' v1.2.3
run_pgp() { STORAGEPRIMS_PGP_KEY_ID=fixture STORAGEPRIMS_GPG_HOMEDIR="$scratch" run; }
refused 'private key block' run_pgp
echo '[ok] key export: committed tagged-commit blobs only (not working tree, env path or keyring); refusals for a missing pin, a missing tag and a private key block'
