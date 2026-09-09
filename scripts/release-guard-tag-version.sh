#!/usr/bin/env bash
# Validate the sole release tag input and, in strict mode, bind it to origin/main.

set -euo pipefail

main() {
	local root version expected tag require_tag
	root="$(git rev-parse --show-toplevel)"
	cd "$root"

	if [[ -n "${RELEASE_TAG+x}" || -n "${STORAGEPRIMS_RELEASE_KEY+x}" ]]; then
		echo "error: generic or legacy release tag aliases are forbidden" >&2
		exit 1
	fi

	version="$(cat VERSION)"
	if [[ ! "$version" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]; then
		echo "error: VERSION must contain one stable semantic version" >&2
		exit 1
	fi
	expected="v${version}"
	tag="${STORAGEPRIMS_RELEASE_TAG:-$expected}"
	require_tag="${STORAGEPRIMS_REQUIRE_TAG:-0}"

	if [[ ! "$tag" =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]; then
		echo "error: release tag must be canonical vX.Y.Z" >&2
		exit 1
	fi
	if [[ "$tag" != "$expected" ]]; then
		echo "error: release tag does not match VERSION" >&2
		exit 1
	fi

	if [[ "$require_tag" == "1" ]]; then
		if [[ -z "${STORAGEPRIMS_RELEASE_TAG:-}" ]]; then
			echo "error: strict release guard requires STORAGEPRIMS_RELEASE_TAG" >&2
			exit 1
		fi
		if git symbolic-ref -q HEAD >/dev/null; then
			echo "error: strict release work must use a detached checkout" >&2
			exit 1
		fi
		if [[ -n "$(git status --porcelain --untracked-files=normal)" ]]; then
			echo "error: strict release work requires a clean checkout" >&2
			exit 1
		fi
		local origin_url
		origin_url="$(git config --get remote.origin.url || true)"
		case "$origin_url" in
		https://github.com/3leaps/storageprims | \
			https://github.com/3leaps/storageprims.git) ;;
		git@*:3leaps/storageprims | git@*:3leaps/storageprims.git)
			local ssh_host resolved_host
			ssh_host="${origin_url#git@}"
			ssh_host="${ssh_host%%:*}"
			resolved_host="$(ssh -G "$ssh_host" 2>/dev/null |
				awk '$1 == "hostname" { print $2; exit }')"
			[[ "$resolved_host" == "github.com" ]] || {
				echo "error: origin SSH host must resolve to github.com" >&2
				exit 1
			}
			;;
		*)
			echo "error: origin must identify github.com/3leaps/storageprims" >&2
			exit 1
			;;
		esac
		git fetch --quiet origin \
			"+refs/heads/main:refs/remotes/origin/main" \
			"+refs/tags/${tag}:refs/tags/${tag}"
		if [[ "$(git cat-file -t "refs/tags/$tag" 2>/dev/null || true)" != "tag" ]]; then
			echo "error: strict release tag must be annotated" >&2
			exit 1
		fi
		local tag_commit head_commit main_commit
		tag_commit="$(git rev-parse "refs/tags/${tag}^{}")"
		head_commit="$(git rev-parse HEAD)"
		main_commit="$(git rev-parse refs/remotes/origin/main)"
		if [[ "$tag_commit" != "$head_commit" ]]; then
			echo "error: release tag and HEAD must be identical" >&2
			exit 1
		fi
		if [[ "$tag_commit" != "$main_commit" ]]; then
			echo "error: release tag and origin/main must be identical" >&2
			exit 1
		fi
	fi

	echo "[ok] release guard passed for $tag"
}

main "$@"
