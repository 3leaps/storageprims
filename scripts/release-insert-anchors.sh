#!/usr/bin/env bash
# Maintainer-only: derive two public anchors from approved public exports.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd -P)"
cd "$root"
: "${STORAGEPRIMS_MINISIGN_PUB:?public minisign export required}"
[[ -s docs/security/release-signing-keys.asc && -s "$STORAGEPRIMS_MINISIGN_PUB" ]] || {
	echo 'error: both public exports required' >&2
	exit 1
}
if [[ -n "${DECERNOR_BIN:-}" ]]; then
	[[ "$DECERNOR_BIN" == /* && -x "$DECERNOR_BIN" ]] || {
		echo 'error: DECERNOR_BIN must be absolute and executable' >&2
		exit 1
	}
else
	DECERNOR_BIN="$(command -v decernor)" || {
		echo 'error: decernor required on PATH' >&2
		exit 1
	}
fi
version="$("$DECERNOR_BIN" version -e | sed -n 's/^Version:[[:space:]]*//p' | head -1)"
python3 - "$version" <<'PY'
import sys
try:
    version = tuple(int(x) for x in sys.argv[1].split('.'))
except ValueError:
    raise SystemExit('error: invalid decernor version')
if len(version) != 3 or version < (0, 1, 7):
    raise SystemExit('error: decernor >= 0.1.7 required')
PY
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
"$DECERNOR_BIN" fingerprint docs/security/release-signing-keys.asc --class public --kind gpg \
	--format ndjson --path-mode none --gpg-role primary >"$scratch/gpg.ndjson"
"$DECERNOR_BIN" fingerprint "$STORAGEPRIMS_MINISIGN_PUB" --class public --kind minisign \
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
PY
mkdir -p keys
cp "$scratch/expected-fingerprints.txt" "$scratch/expected-fingerprints.ndjson" keys/
echo '[ok] generated public fingerprint anchors for review'
