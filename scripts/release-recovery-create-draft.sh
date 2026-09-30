#!/usr/bin/env bash
# Create a new unsigned draft only; never update an existing release or asset.
set -euo pipefail

main() {
	local tag=v0.1.2 commit=1666a2ee3166f2b764514fef36794dfd514967d1
	local directory=dist/release response id notes name asset
	[[ "${STORAGEPRIMS_RELEASE_TAG:-}" == "$tag" &&
		"${STORAGEPRIMS_EXPECTED_COMMIT:-}" == "$commit" ]] || {
		echo 'error: recovery cut does not match the approved tag and commit' >&2
		exit 1
	}
	[[ -z "${GH_REPO+x}" ]] || {
		echo 'error: GH_REPO must be unset' >&2
		exit 1
	}
	./scripts/validate-release-assets.sh "$directory" base
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
	source scripts/release-common.sh
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
	echo '[ok] create-only unsigned recovery draft assets uploaded'
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
	main "$@"
fi
