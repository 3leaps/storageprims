#!/usr/bin/env bash
# One-cut, read-only verification for a draft recovery from an immutable tag.
set -euo pipefail

recovery_die() {
	echo "error: $*" >&2
	return 1
}

recover_annotated_ref() {
	local tag="$1" expected_object="$2" expected_commit="$3" remote_object
	remote_object="$(git ls-remote --exit-code origin "refs/tags/$tag" | awk '{print $1}')" || {
		recovery_die 'remote signed tag is absent or unreadable'
		return 1
	}
	[[ "$remote_object" == "$expected_object" ]] || {
		recovery_die 'remote signed tag object differs from approved object'
		return 1
	}
	# actions/checkout may fetch the peeled commit into refs/tags/<name> after
	# fetching all tags. Repair only this runner-local ref from the checked remote.
	git fetch --quiet --no-tags origin "+refs/tags/$tag:refs/tags/$tag" || return 1
	[[ "$(git rev-parse "refs/tags/$tag")" == "$expected_object" &&
	"$(git cat-file -t "refs/tags/$tag")" == tag &&
	"$(git rev-parse "refs/tags/$tag^{}")" == "$expected_commit" ]] || {
		recovery_die 'runner-local tag is not the approved annotated object'
		return 1
	}
}

require_absent_release() {
	local tag="$1" probe
	probe="$(mktemp)"
	if gh api -i "repos/3leaps/storageprims/releases/tags/$tag" >"$probe" 2>&1; then
		rm -f "$probe"
		recovery_die 'release already exists; refusing to overwrite it'
		return 1
	fi
	if ! grep -Eq '^HTTP/[0-9.]+ 404( |$)' "$probe"; then
		rm -f "$probe"
		recovery_die 'release absence could not be confirmed'
		return 1
	fi
	rm -f "$probe"
}

main() {
	local mode="${1:-}" tag=v0.1.2
	local object=326257266c6def26e49fd4336ad03c3f8fcac532
	local commit=1666a2ee3166f2b764514fef36794dfd514967d1
	[[ "$mode" == verify || "$mode" == ref-only ]] || recovery_die 'expected verify or ref-only mode'
	[[ "${GITHUB_EVENT_NAME:-}" == workflow_dispatch && "${GITHUB_REF:-}" == refs/heads/main ]] ||
		recovery_die 'recovery requires a default-branch manual dispatch'
	[[ "${STORAGEPRIMS_RELEASE_TAG:-}" == "$tag" &&
		"${STORAGEPRIMS_EXPECTED_TAG_OBJECT:-}" == "$object" &&
		"${STORAGEPRIMS_EXPECTED_COMMIT:-}" == "$commit" ]] ||
		recovery_die 'recovery inputs differ from the approved signed cut'
	[[ "$(git rev-parse HEAD)" == "$commit" && "$(cat VERSION)" == 0.1.2 ]] ||
		recovery_die 'checkout differs from the approved tagged tree'
	[[ -z "$(git symbolic-ref -q HEAD || true)" ]] || recovery_die 'checkout must be detached'
	case "$(git config --get remote.origin.url)" in
	https://github.com/3leaps/storageprims | https://github.com/3leaps/storageprims.git) ;;
	*) recovery_die 'origin must identify github.com/3leaps/storageprims' ;;
	esac
	if [[ "$mode" == verify ]]; then
		[[ -z "$(git status --porcelain --untracked-files=normal)" ]] ||
			recovery_die 'tagged checkout must be clean before asset generation'
	fi
	recover_annotated_ref "$tag" "$object" "$commit"
	if [[ "$mode" == verify ]]; then
		STORAGEPRIMS_RELEASE_TAG="$tag" ./scripts/verify-pinned-tag.sh
		local tag_json
		tag_json="$(gh api "repos/3leaps/storageprims/git/tags/$object")" || return 1
		jq -e --arg tag "$tag" --arg commit "$commit" \
			'.tag == $tag and .object.type == "commit" and .object.sha == $commit and
			 .verification.verified == true and .verification.reason == "valid"' \
			<<<"$tag_json" >/dev/null || recovery_die 'GitHub tag verification differs from the approved signed object'
	else
		require_absent_release "$tag"
	fi
	echo "[ok] recovery $mode verified immutable $tag"
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
	main "$@"
fi
