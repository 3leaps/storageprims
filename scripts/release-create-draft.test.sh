#!/usr/bin/env bash
# Maintainer-side create-only draft: bound to the staged fetch record for the
# same tag object, refuses an existing or unknown release, and fails closed on
# a create conflict or partial upload, without contacting GitHub.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd -P)"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
repo="$scratch/repo"
mkdir -p "$repo/scripts" "$repo/docs/releases" "$repo/dist/release" "$scratch/bin"
printf 'Release notes\n' >"$repo/docs/releases/v1.2.3.md"
printf 'fixture\n' >"$repo/dist/release/asset.txt"
cat >"$repo/scripts/validate-release-assets.sh" <<'SH'
#!/usr/bin/env bash
[[ "$1" == dist/release && "$2" == base && -f "$1/asset.txt" ]]
SH
cat >"$repo/scripts/release-common.sh" <<'SH'
release_repo_root() { git rev-parse --show-toplevel; }
release_tag() { printf 'v1.2.3\n'; }
release_base_assets() { printf 'asset.txt\n'; }
require_release_guard() { [[ "$1" == 1 ]] && printf 'guard\n' >>"$MOCK_CALLS"; }
assert_github_release_state() {
	printf 'state\n' >>"$MOCK_CALLS"
	[[ "${MOCK_STATE:-ok}" == ok ]] || { echo 'error: remote draft asset inventory mismatch' >&2; return 1; }
}
SH
cat >"$scratch/bin/gh" <<'SH'
#!/usr/bin/env bash
case "$*" in
'api repos/3leaps/storageprims/releases/tags/v1.2.3')
	printf 'lookup\n' >>"$MOCK_CALLS"
	case "${MOCK_LOOKUP:-absent}" in
	absent) printf 'gh: Not Found (HTTP 404)\n' >&2; exit 1 ;;
	present) printf '{"id":1}\n' ;;
	unknown) printf 'gh: Bad credentials (HTTP 401)\n' >&2; exit 1 ;;
	esac ;;
*'--method POST repos/3leaps/storageprims/releases '*)
	[[ "$*" == *'-F draft=true'* && "$*" == *'-F prerelease=false'* && "$*" == *"target_commitish=$FIXTURE_COMMIT"* ]] || exit 1
	printf 'create\n' >>"$MOCK_CALLS"
	case "${MOCK_CREATE:-ok}" in
	ok) printf '{"id":123,"tag_name":"v1.2.3","target_commitish":"%s","draft":true,"prerelease":false}\n' "$FIXTURE_COMMIT" ;;
	conflict) printf 'HTTP 422: already exists\n' >&2; exit 1 ;;
	esac ;;
*'--method POST '*'uploads.github.com/repos/3leaps/storageprims/releases/123/assets?name=asset.txt'*)
	[[ "$*" == *'--input dist/release/asset.txt'* ]] || exit 1
	printf 'upload\n' >>"$MOCK_CALLS"
	[[ "${MOCK_UPLOAD:-ok}" == ok ]] || exit 1
	printf '{"name":"asset.txt","state":"uploaded"}\n' ;;
*) printf 'unexpected gh call: %s\n' "$*" >&2; exit 1 ;;
esac
SH
chmod +x "$scratch/bin/gh" "$repo/scripts/validate-release-assets.sh"
export PATH="$scratch/bin:$PATH" MOCK_CALLS="$scratch/calls"
git -C "$repo" init -q
git -C "$repo" config user.name 'Fixture Author'
git -C "$repo" config user.email 'fixture@example.invalid'
git -C "$repo" commit -q --allow-empty -m fixture
git -C "$repo" tag -am 'Fixture release' v1.2.3
FIXTURE_COMMIT="$(git -C "$repo" rev-parse HEAD)"
object="$(git -C "$repo" rev-parse refs/tags/v1.2.3)"
export FIXTURE_COMMIT STORAGEPRIMS_RELEASE_TAG=v1.2.3 repo root scratch
record() { printf 'tag=v1.2.3\nobject=%s\ncommit=%s\nrun=7\nattempt=1\n' "$1" "$FIXTURE_COMMIT" >"$repo/dist/release.identity"; }
run() {
	: >"$MOCK_CALLS"
	(cd "$repo" && bash "$root/scripts/release-create-draft.sh") >"$scratch/out" 2>&1
}
refused() {
	local reason="$1" calls="$2"
	shift 2
	if "$@"; then
		echo "error: accepted, expected: $reason" >&2
		cat "$scratch/out" >&2
		exit 1
	fi
	grep -q -- "$reason" "$scratch/out" || {
		echo "error: refused for another reason than '$reason'" >&2
		cat "$scratch/out" >&2
		exit 1
	}
	[[ "$(cat "$MOCK_CALLS")" == "$calls" ]] || {
		printf 'error: calls for %s were:\n%s\n' "$reason" "$(cat "$MOCK_CALLS")" >&2
		exit 1
	}
}

refused 'staged artifact record is absent' 'guard' run
record 0000000000000000000000000000000000000000
refused 'fetched for another tag object' 'guard' run
record "$object"
refused 'a release for this tag already exists' $'guard\nlookup' env MOCK_LOOKUP=present bash -c "$(declare -f run); run"
refused 'release absence is unknown' $'guard\nlookup' env MOCK_LOOKUP=unknown bash -c "$(declare -f run); run"
refused 'create-only draft request failed' $'guard\nlookup\ncreate' env MOCK_CREATE=conflict bash -c "$(declare -f run); run"
refused 'draft may be partial' $'guard\nlookup\ncreate\nupload' env MOCK_UPLOAD=fail bash -c "$(declare -f run); run"
refused 'remote draft asset inventory mismatch' $'guard\nlookup\ncreate\nupload\nstate' env MOCK_STATE=bad bash -c "$(declare -f run); run"
run || {
	cat "$scratch/out" >&2
	exit 1
}
[[ "$(cat "$MOCK_CALLS")" == $'guard\nlookup\ncreate\nupload\nstate' ]]
[[ ! -e "$repo/dist/release.draft-error" ]]
echo '[ok] create-only draft: fetch-record binding, existing or unknown release refused, create conflict, partial upload and final state controls passed'
