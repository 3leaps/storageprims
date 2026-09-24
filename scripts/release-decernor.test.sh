#!/usr/bin/env bash
# Synthetic public exports only; no operator keyring or estate material.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd -P)"
fixture="$(mktemp -d)"
trap 'rm -rf "$fixture"' EXIT
mkdir -p "$fixture/repo/scripts" "$fixture/repo/docs/security" "$fixture/repo/schemas" "$fixture/repo/keys" "$fixture/home" "$fixture/other-home" "$fixture/export"
chmod 700 "$fixture/home" "$fixture/other-home"
cp "$root/scripts/"{release-decernor.sh,release-insert-anchors.sh,install-release-anchors.sh,verify-public-keys.sh} "$fixture/repo/scripts/"
cp "$root/schemas/fingerprint-record.v0.schema.json" "$fixture/repo/schemas/"
bin="${STORAGEPRIMS_DECERNOR_BIN:-$(command -v decernor)}"
[[ "$bin" == /* ]]
fail() {
	if "$@" >"$fixture/output" 2>&1; then
		echo "error: expected rejection: $*" >&2
		exit 1
	fi
}
gpg --homedir "$fixture/home" --batch --pinentry-mode loopback --passphrase '' \
	--quick-gen-key 'Synthetic release <synthetic@example.invalid>' ed25519 sign 1d >/dev/null 2>&1
gpg --homedir "$fixture/home" --batch --armor --export >"$fixture/repo/docs/security/release-signing-keys.asc"
python3 - "$fixture/public.pub" "$fixture/other.pub" <<'PY'
import base64
import pathlib
import sys
for path, byte in zip(sys.argv[1:], (1, 2)):
    blob = b'Ed' + bytes([byte]) * 8 + bytes([byte]) * 32
    pathlib.Path(path).write_text('untrusted comment: synthetic public key\n' + base64.b64encode(blob).decode() + '\n')
PY
export STORAGEPRIMS_MINISIGN_PUB="$fixture/public.pub"
export STORAGEPRIMS_DECERNOR_BIN="$bin"
(
	# General resolver precedence is distinct from ceremony's explicit binding.
	source "$root/scripts/release-decernor.sh"
	resolve_release_decernor general
	[[ "$RELEASE_DECERNOR_BIN" == "$bin" ]]
	unset STORAGEPRIMS_DECERNOR_BIN
	DECERNOR_BIN="$bin" resolve_release_decernor general
	[[ "$RELEASE_DECERNOR_BIN" == "$bin" ]]
	unset DECERNOR_BIN
	mkdir -p "$fixture/path"
	cp "$bin" "$fixture/path/decernor"
	PATH="$fixture/path:$PATH"
	resolve_release_decernor general
	[[ "$RELEASE_DECERNOR_BIN" == "$fixture/path/decernor" ]]
)
"$fixture/repo/scripts/release-insert-anchors.sh" >/dev/null
cp "$fixture/repo/keys/expected-fingerprints."{txt,ndjson} "$fixture/export/"
cp "$fixture/repo/docs/security/release-signing-keys.asc" "$fixture/export/storageprims-release-signing-key.asc"
cp "$fixture/public.pub" "$fixture/export/storageprims-minisign.pub"
"$fixture/repo/scripts/verify-public-keys.sh" "$fixture/export" >/dev/null

fail env -u STORAGEPRIMS_DECERNOR_BIN DECERNOR_BIN="$bin" "$fixture/repo/scripts/verify-public-keys.sh" "$fixture/export"
fail env -u STORAGEPRIMS_DECERNOR_BIN DECERNOR_BIN="$bin" "$fixture/repo/scripts/release-insert-anchors.sh"
fail env STORAGEPRIMS_DECERNOR_BIN=decernor "$fixture/repo/scripts/verify-public-keys.sh" "$fixture/export"
fail env STORAGEPRIMS_DECERNOR_BIN="$fixture/missing" "$fixture/repo/scripts/verify-public-keys.sh" "$fixture/export"
cat >"$fixture/false-version" <<'EOF'
#!/usr/bin/env bash
case "$1 $2" in
  'version ') echo 'decernor 0.1.8' ;;
  'version -e') printf 'Version:         0.1.7\nCommit:          fake\nBuild Date:      fake\nGo Version:      fake\nGofulmen:        fake\nCrucible:        fake\n' ;;
esac
EOF
chmod +x "$fixture/false-version"
fail env STORAGEPRIMS_DECERNOR_BIN="$fixture/false-version" "$fixture/repo/scripts/verify-public-keys.sh" "$fixture/export"
sed 's/0\.1\.8/0.1.7/' "$fixture/false-version" >"$fixture/old-version"
chmod +x "$fixture/old-version"
fail env STORAGEPRIMS_DECERNOR_BIN="$fixture/old-version" "$fixture/repo/scripts/verify-public-keys.sh" "$fixture/export"
sed 's/decernor 0.1.8/other 0.1.8/' "$fixture/false-version" >"$fixture/wrong-identity"
chmod +x "$fixture/wrong-identity"
fail env STORAGEPRIMS_DECERNOR_BIN="$fixture/wrong-identity" "$fixture/repo/scripts/verify-public-keys.sh" "$fixture/export"
cp "$fixture/export/expected-fingerprints.txt" "$fixture/good.txt"
sed 's/^minisign ./minisign Z/' "$fixture/good.txt" >"$fixture/export/expected-fingerprints.txt"
fail "$fixture/repo/scripts/verify-public-keys.sh" "$fixture/export"
cp "$fixture/good.txt" "$fixture/export/expected-fingerprints.txt"
printf '\n' >>"$fixture/export/expected-fingerprints.ndjson"
fail "$fixture/repo/scripts/verify-public-keys.sh" "$fixture/export"
cp "$fixture/repo/keys/expected-fingerprints.ndjson" "$fixture/export/expected-fingerprints.ndjson"
rm "$fixture/export/storageprims-minisign.pub"
fail "$fixture/repo/scripts/verify-public-keys.sh" "$fixture/export"
cp "$fixture/public.pub" "$fixture/export/storageprims-minisign.pub"
cp "$fixture/other.pub" "$fixture/export/storageprims-minisign.pub"
fail "$fixture/repo/scripts/verify-public-keys.sh" "$fixture/export"
cp "$fixture/public.pub" "$fixture/export/storageprims-minisign.pub"
gpg --homedir "$fixture/other-home" --batch --pinentry-mode loopback --passphrase '' \
	--quick-gen-key 'Other synthetic <other@example.invalid>' ed25519 sign 1d >/dev/null 2>&1
gpg --homedir "$fixture/other-home" --batch --armor --export >"$fixture/export/storageprims-release-signing-key.asc"
fail "$fixture/repo/scripts/verify-public-keys.sh" "$fixture/export"
gpg --homedir "$fixture/home" --batch --import "$fixture/export/storageprims-release-signing-key.asc" >/dev/null 2>&1
gpg --homedir "$fixture/home" --batch --armor --export >"$fixture/export/storageprims-release-signing-key.asc"
fail "$fixture/repo/scripts/verify-public-keys.sh" "$fixture/export"
cp "$fixture/repo/docs/security/release-signing-keys.asc" "$fixture/export/storageprims-release-signing-key.asc"
truncate -s 0 "$fixture/export/storageprims-release-signing-key.asc"
fail "$fixture/repo/scripts/verify-public-keys.sh" "$fixture/export"
cp "$fixture/repo/docs/security/release-signing-keys.asc" "$fixture/export/storageprims-release-signing-key.asc"

# A verifier failure after install must restore both old files, including their bytes.
cat >"$fixture/reject-verify" <<EOF
#!/usr/bin/env bash
if [[ "\$1 \${2:-}" == 'fingerprint verify' ]]; then exit 1; fi
exec "$bin" "\$@"
EOF
chmod +x "$fixture/reject-verify"
cp "$fixture/repo/keys/expected-fingerprints.txt" "$fixture/old.txt"
cp "$fixture/repo/keys/expected-fingerprints.ndjson" "$fixture/old.ndjson"
fail env STORAGEPRIMS_DECERNOR_BIN="$fixture/reject-verify" "$fixture/repo/scripts/release-insert-anchors.sh"
cmp "$fixture/old.txt" "$fixture/repo/keys/expected-fingerprints.txt"
cmp "$fixture/old.ndjson" "$fixture/repo/keys/expected-fingerprints.ndjson"
fail "$fixture/repo/scripts/install-release-anchors.sh" "$fixture/repo/keys" "$fixture/new-keys" false
[[ ! -e "$fixture/new-keys/expected-fingerprints.txt" && ! -e "$fixture/new-keys/expected-fingerprints.ndjson" ]]
echo '[ok] Decernor ceremony identity, export verification, and rollback controls passed'
