#!/usr/bin/env bash
# Run per-crate publish dry runs in registry dependency order, never upload.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(dirname "$SCRIPT_DIR")"
cd "$root"
STORAGEPRIMS_REQUIRE_TAG=1 "$SCRIPT_DIR/release-guard-tag-version.sh" >/dev/null
"$SCRIPT_DIR/release-crates.py" check
version="$(cat VERSION)"
patches=()
while IFS= read -r crate; do
	printf 'Dry-run crates.io: %s@%s\n' "$crate" "$version"
	cargo publish --dry-run --locked -p "$crate" ${patches[@]+"${patches[@]}"}
	patches+=(--config "patch.crates-io.${crate}.path=\"crates/${crate}\"")
done <"$root/config/release/publishable-crates.txt"
echo '[ok] Per-crate registry dry runs passed; nothing was published'
