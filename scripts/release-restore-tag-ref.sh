#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Restore the remote annotated tag ref after actions/checkout peels it locally.
#
# The tag object is resolved through the GitHub REST API with the job token
# (GITHUB_TOKEN, read-only in every workflow), not by anonymous `git ls-remote`,
# so the lookup works on a private repository without persisting checkout
# credentials. The release workflows run no GitHub CLI, so this uses curl.
# origin's tag ref is then fetched with the same token, even when the object is
# already local, and must name the object the API answered before it becomes
# the runner-local ref and is bound to HEAD.
set -euo pipefail

die() {
	echo "error: $*" >&2
	exit 1
}

repo=3leaps/storageprims
tag="${STORAGEPRIMS_RELEASE_TAG:-}"
[[ "$tag" =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] || die 'canonical release tag required'
[[ -n "${GITHUB_TOKEN:-}" ]] || die 'GITHUB_TOKEN (the job token) is required for the tag lookup'
case "$(git config --get remote.origin.url)" in
"https://github.com/$repo" | "https://github.com/$repo.git") ;;
*) die 'unexpected release origin' ;;
esac
# The API override exists for local tests; a workflow always asks GitHub.
if [[ -n "${GITHUB_ACTIONS:-}" && -n "${STORAGEPRIMS_GITHUB_API+x}" ]]; then
	die 'STORAGEPRIMS_GITHUB_API is not accepted under GitHub Actions'
fi
api="${STORAGEPRIMS_GITHUB_API:-https://api.github.com}"
[[ "$api" == https://* ]] || die 'the GitHub API must be reached over https'

# The token travels in a curl config on stdin, never in argv or a file.
ref_json="$(printf 'header = "Authorization: Bearer %s"\n' "$GITHUB_TOKEN" |
	curl --proto '=https' --tlsv1.2 --fail --silent --show-error --retry 3 --config - \
		--header 'Accept: application/vnd.github+json' \
		"$api/repos/$repo/git/ref/tags/$tag")" || die 'remote release tag is absent'
remote_type="$(jq -r '.object.type // empty' <<<"$ref_json")" || die 'remote tag lookup returned invalid JSON'
remote_object="$(jq -r '.object.sha // empty' <<<"$ref_json")"
[[ "$(jq -r '.ref // empty' <<<"$ref_json")" == "refs/tags/$tag" ]] || die 'remote tag lookup named another ref'
[[ "$remote_type" == tag ]] || die 'remote release tag must be annotated'
[[ "$remote_object" =~ ^[0-9a-f]{40}$ ]] || die 'remote tag object is invalid'
if [[ -n "${STORAGEPRIMS_EXPECTED_TAG_OBJECT+x}" ]]; then
	[[ "$STORAGEPRIMS_EXPECTED_TAG_OBJECT" =~ ^[0-9a-f]{40}$ &&
		"$remote_object" == "$STORAGEPRIMS_EXPECTED_TAG_OBJECT" ]] ||
		die 'remote tag object differs from validated object'
fi

# Ask origin as well, even when a full-history checkout already holds the
# object, so the API answer and origin's ref must agree. The fetch writes only
# FETCH_HEAD (--refmap= drops any configured fetch mapping, which would
# otherwise update local tags); the runner-local ref changes after the
# comparison. The job token
# serves this one command; checkout credentials stay unpersisted.
# The header travels in the environment, not argv, and its encoded form is
# masked because the runner masks only the raw token.
auth="$(printf 'x-access-token:%s' "$GITHUB_TOKEN" | base64 | tr -d '\n')"
[[ -z "${GITHUB_ACTIONS:-}" ]] || echo "::add-mask::$auth"
GIT_CONFIG_COUNT=1 \
	GIT_CONFIG_KEY_0='http.https://github.com/.extraheader' \
	GIT_CONFIG_VALUE_0="AUTHORIZATION: basic $auth" \
	git fetch --quiet --no-tags --refmap= origin "refs/tags/$tag" || die 'origin release tag could not be fetched'
[[ "$(git rev-parse --verify --quiet FETCH_HEAD)" == "$remote_object" ]] ||
	die "origin's tag object differs from the API answer"
git update-ref "refs/tags/$tag" "$remote_object"
[[ "$(git rev-parse "refs/tags/$tag")" == "$remote_object" &&
"$(git cat-file -t "refs/tags/$tag")" == tag &&
"$(git rev-parse "refs/tags/$tag^{}")" == "$(git rev-parse HEAD)" ]] ||
	die 'runner-local tag is not the remote annotated tag at HEAD'
echo "[ok] runner-local annotated tag restored for $tag"
