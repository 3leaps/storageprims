#!/usr/bin/env bash
# Maintainer-only: explicitly export an approved public key to an absent pin.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd -P)"
cd "$root"
# shellcheck source=scripts/release-decernor.sh
source "$root/scripts/release-decernor.sh"
stop() {
	echo "STOP: $1" >&2
	exit 1
}

: "${STORAGEPRIMS_GPG_HOMEDIR:?load the approved GPG home}"
: "${STORAGEPRIMS_PGP_KEY_ID:?load the approved signing-subkey selector}"
: "${STORAGEPRIMS_GPG_SIGNING_FINGERPRINT:?load the approved primary fingerprint}"
: "${STORAGEPRIMS_MINISIGN_PUB:?load the approved minisign public export}"
[[ "$STORAGEPRIMS_GPG_HOMEDIR" == /* && -d "$STORAGEPRIMS_GPG_HOMEDIR" ]] || stop 'approved GPG home must be an absolute directory'
[[ -f "$STORAGEPRIMS_MINISIGN_PUB" && -s "$STORAGEPRIMS_MINISIGN_PUB" && ! -L "$STORAGEPRIMS_MINISIGN_PUB" ]] ||
	stop 'minisign public export must be a nonempty regular file, not a symlink'
python3 - "$STORAGEPRIMS_GPG_HOMEDIR" "$HOME/.gnupg" <<'PY'
from pathlib import Path
import sys

if Path(sys.argv[1]).resolve() == Path(sys.argv[2]).resolve():
    raise SystemExit('STOP: default GPG home is not approved for this export')
PY
[[ "$STORAGEPRIMS_PGP_KEY_ID" == *'!' ]] || stop 'signing-subkey selector must end with !'
resolve_release_decernor ceremony || stop 'trusted Decernor identity/version check failed'
pin=docs/security/release-signing-keys.asc
[[ ! -e "$pin" && ! -L "$pin" ]] || stop 'public pin already exists; use make release-validate-pin'
[[ ! -L docs/security && (! -e docs/security || -d docs/security) ]] ||
	stop 'public pin directory must be a regular directory, not a symlink'
mkdir -p docs/security || stop 'cannot create public pin directory'
set -C
gpg --homedir "$STORAGEPRIMS_GPG_HOMEDIR" --batch --armor \
	--export "$STORAGEPRIMS_PGP_KEY_ID" >"$pin" || stop 'public GPG export failed; inspect the newly created file before retrying'
"$root/scripts/release-validate-pin.sh"
