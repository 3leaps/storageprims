#!/usr/bin/env bash
# Maintainer-only: inspect the approved public exports without changing the pin.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd -P)"
cd "$root"
# shellcheck source=scripts/release-decernor.sh
source "$root/scripts/release-decernor.sh"
stop() {
	echo "STOP: $1" >&2
	exit 1
}

: "${STORAGEPRIMS_PGP_KEY_ID:?load the approved signing-subkey selector}"
: "${STORAGEPRIMS_GPG_SIGNING_FINGERPRINT:?load the approved primary fingerprint}"
: "${STORAGEPRIMS_MINISIGN_PUB:?load the approved minisign public export}"
[[ "$STORAGEPRIMS_PGP_KEY_ID" == *'!' ]] || stop 'signing-subkey selector must end with !'
resolve_release_decernor ceremony || stop 'trusted Decernor identity/version check failed'

pin=docs/security/release-signing-keys.asc
[[ -f "$pin" && -s "$pin" && ! -L "$pin" ]] || stop 'public GPG pin must be a nonempty regular file, not a symlink'
[[ -f "$STORAGEPRIMS_MINISIGN_PUB" && -s "$STORAGEPRIMS_MINISIGN_PUB" && ! -L "$STORAGEPRIMS_MINISIGN_PUB" ]] ||
	stop 'minisign public export must be a nonempty regular file, not a symlink'

require_public_only() {
	local rc
	if "$RELEASE_DECERNOR_BIN" fingerprint "$1" --kind "$2" \
		--class private --fail-on-empty --path-mode none >/dev/null; then
		stop "private material in $2 public export"
	else
		rc=$?
		[[ "$rc" -eq 3 ]] || stop "$2 private-material check failed"
	fi
}
require_public_only "$pin" gpg
require_public_only "$STORAGEPRIMS_MINISIGN_PUB" minisign
grep -q '^untrusted comment:' "$STORAGEPRIMS_MINISIGN_PUB" || stop 'minisign public export lacks its header'

verify_tmp="$(mktemp -d)"
trap 'rm -rf "$verify_tmp"' EXIT
chmod 700 "$verify_tmp"
"$RELEASE_DECERNOR_BIN" fingerprint "$pin" --kind gpg \
	--class public --fail-on-empty --path-mode none >"$verify_tmp/gpg.ndjson" ||
	stop 'public GPG fingerprint extraction failed'
python3 - "$verify_tmp/gpg.ndjson" "$STORAGEPRIMS_GPG_SIGNING_FINGERPRINT" \
	"$STORAGEPRIMS_PGP_KEY_ID" <<'PY'
import json
import pathlib
import re
import sys

records = [json.loads(line) for line in pathlib.Path(sys.argv[1]).read_text().splitlines()]
primary, selector = sys.argv[2:]
if not re.fullmatch('[0-9A-F]{40}', primary) or not re.fullmatch('[0-9A-F]{40}!', selector):
    raise SystemExit('STOP: invalid configured primary or signing-subkey fingerprint')
if len(records) != 2 or any(r.get('kind') != 'gpg' or r.get('class') != 'public' for r in records):
    raise SystemExit('STOP: expected exactly one public primary and signing subkey')
by_role = {r.get('key_role'): r.get('fingerprint') for r in records}
if by_role != {'primary': primary, 'subkey': selector[:-1]}:
    raise SystemExit('STOP: public pin differs from approved primary or signing subkey')
PY
gpg --homedir "$verify_tmp" --batch --show-keys \
	--fingerprint --with-subkey-fingerprint "$pin" || stop 'isolated public GPG display failed'
"$RELEASE_DECERNOR_BIN" scan docs/security --fail-on unsafe || stop 'public pin scan failed'
echo '[ok] Public signing pin validated; inspect displayed expiry and revocation before proceeding'
