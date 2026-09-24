#!/usr/bin/env bash
# Maintainer-only: derive two public anchors from approved public exports.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd -P)"
# shellcheck source=scripts/release-decernor.sh
source "$root/scripts/release-decernor.sh"
cd "$root"
resolve_release_decernor ceremony
: "${STORAGEPRIMS_MINISIGN_PUB:?public minisign export required}"
[[ -s docs/security/release-signing-keys.asc && ! -L docs/security/release-signing-keys.asc &&
	-s "$STORAGEPRIMS_MINISIGN_PUB" && ! -L "$STORAGEPRIMS_MINISIGN_PUB" ]] || {
	echo 'error: both public exports required' >&2
	exit 1
}
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
"$RELEASE_DECERNOR_BIN" fingerprint docs/security/release-signing-keys.asc --class public --kind gpg \
	--format ndjson --path-mode none --gpg-role primary >"$scratch/gpg.ndjson"
"$RELEASE_DECERNOR_BIN" fingerprint "$STORAGEPRIMS_MINISIGN_PUB" --class public --kind minisign \
	--format ndjson --path-mode none >"$scratch/minisign.ndjson"
python3 - "$scratch" <<'PY'
import json
import pathlib
import sys
base = pathlib.Path(sys.argv[1])
g = [json.loads(line) for line in (base / 'gpg.ndjson').read_text().splitlines() if line]
m = [json.loads(line) for line in (base / 'minisign.ndjson').read_text().splitlines() if line and json.loads(line).get('fingerprint_scheme') == 'minisign-public-blob-sha256-v1']
if len(g) != 1 or len(m) != 1 or g[0].get('fingerprint_scheme') != 'openpgp-fingerprint-v1' or g[0].get('key_role') != 'primary':
    raise SystemExit('error: expected exactly one primary GPG and one minisign blob fingerprint')
(base / 'expected-fingerprints.ndjson').write_text(''.join(json.dumps(r, separators=(',', ':')) + '\n' for r in (g[0], m[0])))
(base / 'expected-fingerprints.txt').write_text('gpg ' + g[0]['fingerprint'] + '\nminisign ' + m[0]['fingerprint'] + '\n')
(base / 'gpg.json').write_text(json.dumps(g[0]) + '\n')
(base / 'minisign.json').write_text(json.dumps(m[0]) + '\n')
PY
for kind in gpg minisign; do
	"$RELEASE_DECERNOR_BIN" validate --schema "$root/schemas/fingerprint-record.v0.schema.json" \
		--data "$scratch/$kind.json" >/dev/null
done
"$RELEASE_DECERNOR_BIN" fingerprint docs/security/release-signing-keys.asc --class public --kind gpg \
	--format ndjson --path-mode none --gpg-role primary >"$scratch/gpg.verify.ndjson"
"$RELEASE_DECERNOR_BIN" fingerprint "$STORAGEPRIMS_MINISIGN_PUB" --class public --kind minisign \
	--format ndjson --path-mode none >"$scratch/minisign.verify.ndjson"
cmp "$scratch/gpg.ndjson" "$scratch/gpg.verify.ndjson"
cmp "$scratch/minisign.ndjson" "$scratch/minisign.verify.ndjson"
"$root/scripts/install-release-anchors.sh" "$scratch" "$root/keys" \
	"$RELEASE_DECERNOR_BIN" fingerprint verify \
	--anchors "$root/keys/expected-fingerprints.txt" \
	--anchors-ndjson "$root/keys/expected-fingerprints.ndjson" \
	--gpg "$root/docs/security/release-signing-keys.asc" --minisign "$STORAGEPRIMS_MINISIGN_PUB" >/dev/null
echo '[ok] generated public fingerprint anchors for review'
