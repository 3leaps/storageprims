#!/usr/bin/env bash
# Test the public-pin precursor targets with synthetic keys and exports only.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd -P)"
fixture="$(mktemp -d)"
trap 'rm -rf "$fixture"' EXIT
mkdir -p "$fixture/repo/scripts" "$fixture/repo/docs" "$fixture/home" "$fixture/other-home/.gnupg"
chmod 700 "$fixture/home" "$fixture/other-home" "$fixture/other-home/.gnupg"
cp "$root/Makefile" "$root/VERSION" "$fixture/repo/"
cp "$root/scripts/"{release-decernor.sh,release-export-pin.sh,release-validate-pin.sh,release-insert-anchors.sh} "$fixture/repo/scripts/"
bin="${STORAGEPRIMS_DECERNOR_BIN:-$(command -v decernor)}"
export STORAGEPRIMS_DECERNOR_BIN="$bin"
python3 - "$fixture/public.pub" "$fixture/private.pub" <<'PY'
import base64
import pathlib
import sys
pathlib.Path(sys.argv[1]).write_text('untrusted comment: synthetic public key\n' + base64.b64encode(b'Ed' + b'\x01' * 40).decode() + '\n')
pathlib.Path(sys.argv[2]).write_text('untrusted comment: minisign encrypted secret key\n' + base64.b64encode(b'\x02' * 128).decode() + '\n')
PY
export STORAGEPRIMS_MINISIGN_PUB="$fixture/public.pub"
export STORAGEPRIMS_GPG_HOMEDIR="$fixture/home"
gpg --homedir "$fixture/home" --batch --pinentry-mode loopback --passphrase '' \
	--quick-gen-key 'Synthetic signing <signing@example.invalid>' ed25519 cert 1d >/dev/null 2>&1
primary="$(gpg --homedir "$fixture/home" --batch --with-colons --fingerprint --list-keys | awk -F: '$1 == "fpr" { print $10; exit }')"
gpg --homedir "$fixture/home" --batch --pinentry-mode loopback --passphrase '' \
	--quick-add-key "$primary" ed25519 sign 1d >/dev/null 2>&1
selector="$(gpg --homedir "$fixture/home" --batch --with-colons --with-subkey-fingerprint --list-keys | awk -F: '$1 == "sub" { subkey=1; next } subkey && $1 == "fpr" { print $10 "!"; exit }')"
export STORAGEPRIMS_GPG_SIGNING_FINGERPRINT="$primary" STORAGEPRIMS_PGP_KEY_ID="$selector"
unset STORAGEPRIMS_RELEASE_TAG STORAGEPRIMS_TAG_MESSAGE_DIR || true

run() { make --no-print-directory -s -C "$fixture/repo" "$@"; }
pin_times() {
	if [[ "$(uname -s)" == Darwin ]]; then
		stat -f '%m:%c' "$1"
	else
		stat -c '%Y:%Z' "$1"
	fi
}
fail() {
	local reason="$1"
	shift
	if "$@" >"$fixture/output" 2>&1; then
		echo "error: expected rejection: $reason" >&2
		exit 1
	fi
	grep -q "$reason" "$fixture/output" || {
		echo "error: missing named rejection: $reason" >&2
		exit 1
	}
}
pin="$fixture/repo/docs/security/release-signing-keys.asc"
fail 'public GPG pin' run release-validate-pin
fail 'minisign public export' env STORAGEPRIMS_MINISIGN_PUB="$fixture/missing.pub" make --no-print-directory -s -C "$fixture/repo" release-export-pin
[[ ! -e "$pin" ]]
bash -c 'make --no-print-directory -s -C "$1" release-export-pin' bash "$fixture/repo" >"$fixture/output"
[[ -s "$pin" ]]
# Distinct old mtime catches a same-second touch despite stat's second resolution.
touch -t 202001010000 "$pin"
cp "$pin" "$fixture/pin-copy"
before="$(pin_times "$pin")"
env -u STORAGEPRIMS_GPG_HOMEDIR make --no-print-directory -s -C "$fixture/repo" release-validate-pin >"$fixture/output"
bash -c 'make --no-print-directory -s -C "$1" release-validate-pin' bash "$fixture/repo" >"$fixture/output"
if command -v zsh >/dev/null 2>&1; then
	zsh -c 'make --no-print-directory -s -C "$1" release-validate-pin' zsh "$fixture/repo" >"$fixture/output"
	# shellcheck disable=SC2016 # The nested zsh expands $1.
	fail 'public pin already exists' zsh -c 'make --no-print-directory -s -C "$1" release-export-pin' zsh "$fixture/repo"
elif [[ "$(uname -s)" == Darwin ]]; then
	echo 'error: zsh required for macOS release precursor test' >&2
	exit 1
else
	echo '[skip] zsh unavailable; bash-to-make coverage passed'
fi
cmp "$pin" "$fixture/pin-copy"
after="$(pin_times "$pin")"
[[ "$before" == "$after" ]] || {
	echo 'error: existing public pin timestamp changed' >&2
	exit 1
}
fail 'public pin already exists' run release-export-pin
cmp "$pin" "$fixture/pin-copy"
fail 'trusted Decernor identity' env STORAGEPRIMS_DECERNOR_BIN=decernor make --no-print-directory -s -C "$fixture/repo" release-validate-pin
fail 'trusted Decernor' env -u STORAGEPRIMS_DECERNOR_BIN DECERNOR_BIN="$bin" make --no-print-directory -s -C "$fixture/repo" release-validate-pin
fail 'trusted Decernor' env -u STORAGEPRIMS_DECERNOR_BIN DECERNOR_BIN="$bin" make --no-print-directory -s -C "$fixture/repo" release-export-pin
fail 'private material in minisign' env STORAGEPRIMS_MINISIGN_PUB="$fixture/private.pub" make --no-print-directory -s -C "$fixture/repo" release-validate-pin
other_digit=A
[[ "${primary: -1}" == A && "${selector: -2:1}" == A ]] && other_digit=B
wrong_primary="${primary%?}${other_digit}"
[[ "$wrong_primary" != "$primary" ]] || wrong_primary="${primary%?}B"
wrong_subkey="${selector%??}${other_digit}!"
[[ "$wrong_subkey" != "$selector" ]] || wrong_subkey="${selector%??}B!"
fail 'approved primary' env STORAGEPRIMS_GPG_SIGNING_FINGERPRINT="$wrong_primary" make --no-print-directory -s -C "$fixture/repo" release-validate-pin
fail 'signing subkey' env STORAGEPRIMS_PGP_KEY_ID="$wrong_subkey" make --no-print-directory -s -C "$fixture/repo" release-validate-pin
gpg --homedir "$fixture/home" --batch --armor --export-secret-keys "$selector" >"$pin"
fail 'private material in gpg' run release-validate-pin
fail 'private material in gpg' run release-insert-anchors
[[ ! -e "$fixture/repo/keys/expected-fingerprints.txt" ]]
cp "$fixture/pin-copy" "$pin"
gpg --homedir "$fixture/home" --batch --armor --export-secret-keys "$selector" >"$fixture/repo/docs/security/unexpected.asc"
fail 'public pin scan failed' run release-validate-pin
rm "$fixture/repo/docs/security/unexpected.asc"
rm "$pin"
fail 'default GPG home' env HOME="$fixture/other-home" STORAGEPRIMS_GPG_HOMEDIR="$fixture/other-home/.gnupg" make --no-print-directory -s -C "$fixture/repo" release-export-pin
ln -s "$fixture/pin-copy" "$pin"
fail 'public pin already exists' run release-export-pin
fail 'public GPG pin' run release-validate-pin
echo '[ok] Synthetic pin export, existing-pin no-write, and precursor guards passed'
