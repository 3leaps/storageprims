#!/usr/bin/env bash
# Synthetic, short-lived signing fixture; all generated material stays outside git.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd -P)"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
export GNUPGHOME="$scratch/gpg"
mkdir -m 700 "$GNUPGHOME"
gpg --batch --pinentry-mode loopback --passphrase '' --quick-generate-key \
	'3 Leaps Infosec Team <infosec@3leaps.net>' ed25519 sign 0 >/dev/null 2>&1
primary="$(gpg --batch --with-colons --fingerprint --list-keys | awk -F: '$1=="fpr" {print $10;exit}')"
gpg --batch --pinentry-mode loopback --passphrase '' --quick-add-key "$primary" ed25519 sign 1d >/dev/null 2>&1
subkey="$(gpg --batch --with-colons --with-subkey-fingerprint --list-keys "$primary" | awk -F: '$1=="sub" {s=1;next} s && $1=="fpr" {print $10;exit}')"
fixture="$scratch/repo"
git init -q -b main "$fixture"
mkdir -p "$fixture/docs/security" "$fixture/keys" "$fixture/scripts" "$fixture/config/release"
cp "$root/scripts/verify-pinned-tag.sh" "$root/scripts/validate-release-anchors.sh" "$fixture/scripts/"
cp "$root/config/release/tagger-identity.txt" "$fixture/config/release/"
gpg --batch --armor --export "$primary" >"$fixture/docs/security/release-signing-keys.asc"
printf 'gpg %s\nminisign %064d\n' "$primary" 0 >"$fixture/keys/expected-fingerprints.txt"
python3 - "$primary" "$fixture/keys/expected-fingerprints.ndjson" <<'PY'
import json
import pathlib
import sys
out = pathlib.Path(sys.argv[2])
out.write_text(json.dumps({'fingerprint_scheme':'openpgp-fingerprint-v1','key_role':'primary','fingerprint':sys.argv[1]})+'\n'+json.dumps({'fingerprint_scheme':'minisign-public-blob-sha256-v1','fingerprint':'0'*64})+'\n')
PY
printf 'fixture\n' >"$fixture/file"
printf 'Fixture release\n' >"$scratch/message.txt"
git -C "$fixture" config user.name '3 Leaps Infosec Team'
git -C "$fixture" config user.email infosec@3leaps.net
git -C "$fixture" add file
git -C "$fixture" commit -qm fixture
export STORAGEPRIMS_RELEASE_TAG=v1.2.3
export STORAGEPRIMS_TAGGER_NAME='3 Leaps Infosec Team'
export STORAGEPRIMS_TAGGER_EMAIL=infosec@3leaps.net
(
	cd "$fixture"
	GIT_COMMITTER_NAME='3 Leaps Infosec Team' GIT_COMMITTER_EMAIL=infosec@3leaps.net \
		git tag -s -a --cleanup=verbatim -u "$subkey!" -F "$scratch/message.txt" "$STORAGEPRIMS_RELEASE_TAG"
)
git -C "$fixture" cat-file tag v1.2.3 | grep -q -- '^-----BEGIN PGP SIGNATURE-----$' || {
	echo 'error: fixture tag not signed' >&2
	exit 1
}
expect_fail() { if "$@" >/dev/null 2>&1; then
	echo 'expected signature control failure' >&2
	exit 1
fi; }
verify() { (
	cd "$fixture"
	./scripts/verify-pinned-tag.sh
); }
verify >/dev/null
STORAGEPRIMS_PGP_KEY_ID="$subkey!" verify >/dev/null
cp "$fixture/keys/expected-fingerprints.txt" "$scratch/anchors.good"
printf 'gpg %s\nminisign %064d\n' "$primary" 1 >"$fixture/keys/expected-fingerprints.txt"
expect_fail verify
cp "$scratch/anchors.good" "$fixture/keys/expected-fingerprints.txt"
git -C "$fixture" update-ref refs/remotes/origin/main "$(git -C "$fixture" rev-parse HEAD)"
# shellcheck source=scripts/release-tag-common.sh
source "$root/scripts/release-tag-common.sh"
(
	cd "$fixture"
	tag_verify_object refs/tags/v1.2.3 'Fixture release'
)
(
	cd "$fixture"
	expect_fail tag_verify_object refs/tags/v1.2.3 'Tampered release'
)
printf '# Heading\nFixture release\n' >"$scratch/message.txt"
(
	cd "$fixture"
	GIT_COMMITTER_NAME='3 Leaps Infosec Team' GIT_COMMITTER_EMAIL=infosec@3leaps.net \
		git tag -fs -a --cleanup=verbatim -u "$subkey!" -F "$scratch/message.txt" "$STORAGEPRIMS_RELEASE_TAG"
	tag_verify_object refs/tags/v1.2.3 $'# Heading\nFixture release'
)
verify >/dev/null
printf 'Fixture release\n' >"$scratch/message.txt"
(
	cd "$fixture"
	GIT_COMMITTER_NAME='3 Leaps Infosec Team' GIT_COMMITTER_EMAIL=infosec@3leaps.net \
		git tag -fs -a --cleanup=verbatim -u "$subkey!" -F "$scratch/message.txt" "$STORAGEPRIMS_RELEASE_TAG"
)
mv "$fixture/docs/security/release-signing-keys.asc" "$scratch/approved.asc"
expect_fail verify
cp "$scratch/approved.asc" "$fixture/docs/security/release-signing-keys.asc"
gpg --batch --pinentry-mode loopback --passphrase '' --quick-generate-key \
	'Fixture Extra <extra@example.invalid>' ed25519 cert 1d >/dev/null 2>&1
extra="$(gpg --batch --with-colons --fingerprint --list-keys 'Fixture Extra' | awk -F: '$1=="fpr" {print $10;exit}')"
gpg --batch --armor --export "$extra" >>"$fixture/docs/security/release-signing-keys.asc"
expect_fail verify
gpg --batch --armor --export "$extra" >"$fixture/docs/security/release-signing-keys.asc"
expect_fail verify
cp "$scratch/approved.asc" "$fixture/docs/security/release-signing-keys.asc"
(
	cd "$fixture"
	GIT_COMMITTER_NAME='3 Leaps Infosec Team' GIT_COMMITTER_EMAIL=infosec@3leaps.net \
		git tag -fs -a --cleanup=verbatim -u "$primary!" -F "$scratch/message.txt" "$STORAGEPRIMS_RELEASE_TAG"
)
expect_fail verify
(
	cd "$fixture"
	GIT_COMMITTER_NAME='3 Leaps Infosec Team' GIT_COMMITTER_EMAIL=infosec@3leaps.net \
		git tag -fs -a --cleanup=verbatim -u "$subkey!" -F "$scratch/message.txt" "$STORAGEPRIMS_RELEASE_TAG"
)
verify >/dev/null
real_gpg="$(command -v gpg)"
mkdir -p "$scratch/bin"
cat >"$scratch/bin/gpg" <<'SH'
#!/usr/bin/env bash
if [[ -n "${FIXTURE_FUTURE_TIME:-}" ]]; then
	exec "$FIXTURE_REAL_GPG" --faked-system-time "$FIXTURE_FUTURE_TIME" "$@"
fi
exec "$FIXTURE_REAL_GPG" "$@"
SH
chmod +x "$scratch/bin/gpg"
export FIXTURE_REAL_GPG="$real_gpg" PATH="$scratch/bin:$PATH"
export FIXTURE_FUTURE_TIME="$(($(date +%s) + 172800))"
mkdir -m 700 "$scratch/expired-ring"
GNUPGHOME="$scratch/expired-ring" gpg --batch --quiet --import "$fixture/docs/security/release-signing-keys.asc"
(
	cd "$fixture"
	GNUPGHOME="$scratch/expired-ring" git verify-tag --raw v1.2.3 >"$scratch/expired-status" 2>&1
) || true
grep -q '^\[GNUPG:\] VALIDSIG ' "$scratch/expired-status"
grep -q '^\[GNUPG:\] EXPKEYSIG ' "$scratch/expired-status"
expect_fail verify
unset FIXTURE_FUTURE_TIME
verify >/dev/null
git -C "$fixture" tag -d "$STORAGEPRIMS_RELEASE_TAG" >/dev/null
git -C "$fixture" tag "$STORAGEPRIMS_RELEASE_TAG"
expect_fail verify
echo '[ok] isolated pinned-tag signature controls passed'
