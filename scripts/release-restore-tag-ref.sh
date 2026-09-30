#!/usr/bin/env bash
# Restore the remote annotated tag ref after actions/checkout peels it locally.
set -euo pipefail

tag="${STORAGEPRIMS_RELEASE_TAG:-}"
[[ "$tag" =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] || {
	echo 'error: canonical release tag required' >&2
	exit 1
}
case "$(git config --get remote.origin.url)" in
https://github.com/3leaps/storageprims | https://github.com/3leaps/storageprims.git) ;;
*)
	echo 'error: unexpected release origin' >&2
	exit 1
	;;
esac
remote_object="$(git ls-remote --exit-code origin "refs/tags/$tag" | awk '{print $1}')" || {
	echo 'error: remote release tag is absent' >&2
	exit 1
}
[[ "$remote_object" =~ ^[0-9a-f]{40}$ ]] || {
	echo 'error: remote tag object is invalid' >&2
	exit 1
}
if [[ -n "${STORAGEPRIMS_EXPECTED_TAG_OBJECT+x}" ]]; then
	[[ "$STORAGEPRIMS_EXPECTED_TAG_OBJECT" =~ ^[0-9a-f]{40}$ &&
		"$remote_object" == "$STORAGEPRIMS_EXPECTED_TAG_OBJECT" ]] || {
		echo 'error: remote tag object differs from validated object' >&2
		exit 1
	}
fi
# This fetch changes only the runner-local ref. The existing strict guard and
# pinned-signature job separately verify the object, target, and signer.
git fetch --quiet --no-tags origin "+refs/tags/$tag:refs/tags/$tag"
[[ "$(git rev-parse "refs/tags/$tag")" == "$remote_object" &&
"$(git cat-file -t "refs/tags/$tag")" == tag &&
"$(git rev-parse "refs/tags/$tag^{}")" == "$(git rev-parse HEAD)" ]] || {
	echo 'error: runner-local tag is not the remote annotated tag at HEAD' >&2
	exit 1
}
echo "[ok] runner-local annotated tag restored for $tag"
