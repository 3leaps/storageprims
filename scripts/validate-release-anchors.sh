#!/usr/bin/env bash
# Cross-check the decernor pair and the pinned public primary.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd -P)"
cd "$root"
python3 - keys/expected-fingerprints.txt keys/expected-fingerprints.ndjson <<'PY'
import json
import pathlib
import re
import sys
txt, ndjson = (pathlib.Path(path) for path in sys.argv[1:])
if not txt.is_file() or not ndjson.is_file() or txt.is_symlink() or ndjson.is_symlink():
    raise SystemExit('error: committed fingerprint pair missing')
lines = txt.read_text().splitlines()
if len(lines) != 2:
    raise SystemExit('error: expected two fingerprint lines')
records = [json.loads(line) for line in ndjson.read_text().splitlines() if line]
if len(records) != 2:
    raise SystemExit('error: expected two fingerprint records')
g, m = records
if g.get('fingerprint_scheme') != 'openpgp-fingerprint-v1' or g.get('key_role') != 'primary':
    raise SystemExit('error: expected a primary GPG record')
if m.get('fingerprint_scheme') != 'minisign-public-blob-sha256-v1':
    raise SystemExit('error: expected a minisign blob record')
if not re.fullmatch('[0-9A-F]{40}', g.get('fingerprint', '')) or not re.fullmatch('[0-9a-f]{64}', m.get('fingerprint', '')):
    raise SystemExit('error: malformed public fingerprint')
if lines != ['gpg ' + g['fingerprint'], 'minisign ' + m['fingerprint']]:
    raise SystemExit('error: fingerprint text and records differ')
PY
pin=docs/security/release-signing-keys.asc
[[ -s "$pin" && ! -L "$pin" ]] || {
	echo 'error: committed public pin missing' >&2
	exit 1
}
temp="$(mktemp -d)"
trap 'rm -rf "$temp"' EXIT
chmod 700 "$temp"
GNUPGHOME="$temp" gpg --batch --quiet --import "$pin"
listing="$(GNUPGHOME="$temp" gpg --batch --with-colons --fingerprint --list-keys)"
[[ "$(awk -F: '$1=="pub" {n++} END {print n+0}' <<<"$listing")" == 1 ]] || {
	echo 'error: pin must contain exactly one public primary' >&2
	exit 1
}
[[ "$(awk -F: '$1=="fpr" {print $10;exit}' <<<"$listing")" == "$(awk '$1=="gpg" {print $2}' keys/expected-fingerprints.txt)" ]] || {
	echo 'error: primary pin differs from fingerprint pair' >&2
	exit 1
}
echo '[ok] public pin and decernor anchors agree'
