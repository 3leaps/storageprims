#!/usr/bin/env bash
# Check whether draft creation is safe, or verify the created exact draft.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=release-common.sh
# shellcheck disable=SC1091
source "$SCRIPT_DIR/release-common.sh"

mode="${1:?usage: check-draft-release.sh before-or-after}"
tag="$(release_tag)"

if [[ -n "${GH_REPO+x}" ]]; then
	echo "error: GH_REPO must be unset; repository authority is fixed" >&2
	exit 1
fi

case "$mode" in
before)
	releases="$(mktemp "${TMPDIR:-/tmp}/storageprims-release-list.XXXXXX")"
	trap 'rm -f "$releases"' EXIT
	gh release list --repo "$STORAGEPRIMS_REPOSITORY" --limit 100 \
		--json tagName >"$releases"
	if jq -e --arg tag "$tag" \
		'any(.[]; .tagName == $tag)' "$releases" >/dev/null; then
		assert_github_release_state release_base_assets
	else
		[[ "$(gh repo view "$STORAGEPRIMS_REPOSITORY" \
			--json nameWithOwner --jq .nameWithOwner)" == "$STORAGEPRIMS_REPOSITORY" ]] || {
			echo "error: GitHub repository identity mismatch" >&2
			exit 1
		}
	fi
	;;
after)
	assert_github_release_state release_base_assets
	;;
*)
	echo "error: mode must be before or after" >&2
	exit 1
	;;
esac

echo "[ok] draft release state is safe for $mode"
