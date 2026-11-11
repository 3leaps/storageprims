#!/usr/bin/env bash
set -euo pipefail
for name in $(compgen -e); do
	[[ "$name" != PYTHON* ]] || unset "$name"
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(dirname "$SCRIPT_DIR")"
# shellcheck source=release-crates-registry.sh
# shellcheck disable=SC1091
source "$SCRIPT_DIR/release-crates-registry.sh"
cd "$root"
python_bin="$(command -v python3)"
[[ "$python_bin" == /* ]] || {
	echo 'error: python3 must resolve to an absolute path' >&2
	exit 1
}
STORAGEPRIMS_REQUIRE_TAG=1 "$SCRIPT_DIR/release-guard-tag-version.sh" >/dev/null
"$python_bin" -I "$SCRIPT_DIR/release-crates.py" check
version="$(cat VERSION)"
commit="$(git rev-parse "refs/tags/$STORAGEPRIMS_RELEASE_TAG^{commit}")"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
verify_crate() {
	local crate="$1"
	registry_wait "$crate" "$version"
	registry_api_check "$crate" "$version"
	curl --disable --proto '=https' --tlsv1.2 --fail --silent --show-error --retry 3 \
		"https://crates.io/api/v1/crates/$crate/$version" >"$scratch/api.json"
	curl --disable --proto '=https' --proto-redir '=https' --tlsv1.2 --fail --silent --show-error --location --retry 3 \
		"https://static.crates.io/crates/$crate/$crate-$version.crate" >"$scratch/$crate-$version.crate"
	"$python_bin" -I "$SCRIPT_DIR/release-crates-audit.py" "$scratch/$crate-$version.crate" "$crate" "$version" "$commit" "$scratch/api.json"
	curl --disable --proto '=https' --tlsv1.2 --fail --silent --show-error --retry 3 \
		"https://docs.rs/crate/$crate/$version/status.json" >"$scratch/docs.json"
	jq -e --arg version "$version" '.doc_status == true and .version == $version' "$scratch/docs.json" >/dev/null || {
		echo "error: docs.rs has not completed $crate@$version; rerun verification after the build" >&2
		return 1
	}
	echo "[ok] docs.rs $crate@$version"
}
if [[ -n "${CRATE:-}" ]]; then
	grep -Fxq -- "$CRATE" config/release/publishable-crates.txt || {
		echo 'error: CRATE is not in the publishable crate list' >&2
		exit 1
	}
	verify_crate "$CRATE"
else
	while IFS= read -r crate; do verify_crate "$crate"; done <config/release/publishable-crates.txt
fi
echo '[ok] crates.io archives and docs.rs verified'
