#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(dirname "$SCRIPT_DIR")"
# shellcheck source=release-crates-registry.sh
# shellcheck disable=SC1091
source "$SCRIPT_DIR/release-crates-registry.sh"
cd "$root"
STORAGEPRIMS_REQUIRE_TAG=1 "$SCRIPT_DIR/release-guard-tag-version.sh" >/dev/null
"$SCRIPT_DIR/release-crates.py" check
version="$(cat VERSION)"
if [[ -n "${CRATE:-}" ]]; then
	grep -Fxq -- "$CRATE" config/release/publishable-crates.txt || {
		echo 'error: CRATE is not in the publishable crate list' >&2
		exit 1
	}
	registry_wait "$CRATE" "$version"
	registry_api_check "$CRATE" "$version"
else
	while IFS= read -r crate; do
		registry_wait "$crate" "$version"
		registry_api_check "$crate" "$version"
	done <config/release/publishable-crates.txt
fi
echo '[ok] crates.io index and API confirm published version'
