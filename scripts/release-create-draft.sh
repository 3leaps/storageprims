#!/usr/bin/env bash
# Maintainer-only: create a new unsigned draft from the staged CI artifacts.
# Never update an existing release or asset.
set -euo pipefail

main() {
	local tag commit object root record
	local directory=dist/release response id notes name asset
	source scripts/release-common.sh
	# Annotated, local object equals origin's, and tag, HEAD and origin/main
	# are one commit.
	require_release_guard 1 >/dev/null
	tag="$(release_tag)"
	root="$(release_repo_root)"
	[[ -z "${GH_REPO+x}" ]] || {
		echo 'error: GH_REPO must be unset' >&2
		exit 1
	}
	object="$(git rev-parse "refs/tags/$tag")"
	commit="$(git rev-parse "refs/tags/$tag^{commit}")"
	# The staged set must come from the fetch for this same tag object.
	record="$root/$directory.identity"
	[[ -f "$record" && ! -L "$record" ]] || {
		echo 'error: staged artifact record is absent; run make release-fetch' >&2
		exit 1
	}
	[[ "$(awk -F= '$1=="tag" {print $2}' "$record")" == "$tag" &&
	"$(awk -F= '$1=="object" {print $2}' "$record")" == "$object" &&
	"$(awk -F= '$1=="commit" {print $2}' "$record")" == "$commit" ]] || {
		echo 'error: staged artifacts were fetched for another tag object' >&2
		exit 1
	}
	./scripts/validate-release-assets.sh "$directory" base
	# A 404 is the only absence proof; transport or auth failures are not.
	if gh api "repos/3leaps/storageprims/releases/tags/$tag" >/dev/null 2>"$root/$directory.draft-error"; then
		rm -f "$root/$directory.draft-error"
		echo 'error: a release for this tag already exists' >&2
		exit 1
	fi
	grep -q 'HTTP 404' "$root/$directory.draft-error" || {
		rm -f "$root/$directory.draft-error"
		echo 'error: release absence is unknown' >&2
		exit 1
	}
	rm -f "$root/$directory.draft-error"
	notes="docs/releases/$tag.md"
	[[ -s "$notes" && ! -L "$notes" ]] || {
		echo 'error: tagged release notes are absent' >&2
		exit 1
	}

	# POST /releases creates a new release or fails. Unlike action-gh-release,
	# it has no update-on-existing path. Do not retry a failed or partial create.
	response="$(gh api --method POST repos/3leaps/storageprims/releases \
		-f "tag_name=$tag" -f "target_commitish=$commit" \
		-f "body=$(cat "$notes")" -F draft=true -F prerelease=false)" || {
		echo 'error: create-only draft request failed; inspect remote state before any retry' >&2
		exit 1
	}
	id="$(jq -er --arg tag "$tag" --arg commit "$commit" '
  select(.tag_name == $tag and .target_commitish == $commit and
         .draft == true and .prerelease == false and
         (.id | type) == "number" and .id > 0) | .id' <<<"$response")" || {
		echo 'error: created release response did not match the requested draft; inspect remote state' >&2
		exit 1
	}

	# Upload only the exact, previously validated base inventory. Uploading by
	# release ID cannot target a different release. No --clobber or overwrite path.
	while IFS= read -r name; do
		[[ "$name" =~ ^[A-Za-z0-9._-]+$ && -f "$directory/$name" && ! -L "$directory/$name" ]] || {
			echo 'error: unsafe release asset; draft may be partial' >&2
			exit 1
		}
		asset="$(gh api --method POST \
			-H 'Content-Type: application/octet-stream' \
			"https://uploads.github.com/repos/3leaps/storageprims/releases/$id/assets?name=$name" \
			--input "$directory/$name")" || {
			echo 'error: asset upload failed; draft may be partial, inspect before proceeding' >&2
			exit 1
		}
		jq -e --arg name "$name" 'select(.name == $name and .state == "uploaded")' \
			<<<"$asset" >/dev/null || {
			echo 'error: asset upload response invalid; draft may be partial' >&2
			exit 1
		}
	done < <(release_base_assets)
	assert_github_release_state release_base_assets
	echo '[ok] create-only unsigned draft assets uploaded'
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
	main "$@"
fi
