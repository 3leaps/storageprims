#!/usr/bin/env bash
# Create-only conflict and partial-upload controls without contacting GitHub.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd -P)"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
mkdir -p "$scratch/repo/scripts" "$scratch/repo/docs/releases" "$scratch/repo/dist/release" "$scratch/bin"
printf 'Release notes\n' >"$scratch/repo/docs/releases/v1.2.3.md"
printf 'fixture\n' >"$scratch/repo/dist/release/asset.txt"
cat >"$scratch/repo/scripts/validate-release-assets.sh" <<'SH'
#!/usr/bin/env bash
[[ "$1" == dist/release && "$2" == base && -f "$1/asset.txt" ]]
SH
cat >"$scratch/repo/scripts/release-restore-tag-ref.sh" <<'SH'
#!/usr/bin/env bash
[[ "$STORAGEPRIMS_EXPECTED_TAG_OBJECT" == 0000000000000000000000000000000000000000 ]] || exit 1
printf 'recheck\n' >>"$MOCK_CALLS"
SH
cat >"$scratch/repo/scripts/release-common.sh" <<'SH'
release_tag() { printf 'v1.2.3\n'; }
release_base_assets() { printf 'asset.txt\n'; }
SH
cat >"$scratch/bin/gh" <<'SH'
#!/usr/bin/env bash
case "$*" in
*'--method POST repos/3leaps/storageprims/releases '* | *'--method POST repos/3leaps/storageprims/releases')
	[[ "$*" == *'-F draft=true'* && "$*" == *'-F prerelease=false'* ]] || exit 1
	printf 'create\n' >>"$MOCK_CALLS"
	case "${MOCK_CREATE:-ok}" in
	ok) printf '{"id":123,"tag_name":"v1.2.3","target_commitish":"%s","draft":true,"prerelease":false}\n' "$STORAGEPRIMS_EXPECTED_COMMIT" ;;
	conflict) printf 'HTTP 422: already exists\n' >&2; exit 1 ;;
	esac ;;
*'--method POST '*'uploads.github.com/repos/3leaps/storageprims/releases/123/assets?name=asset.txt'*)
	[[ "$*" == *'--input dist/release/asset.txt'* ]] || exit 1
	printf 'upload\n' >>"$MOCK_CALLS"
	[[ "${MOCK_UPLOAD:-ok}" == ok ]] || exit 1
	printf '{"name":"asset.txt","state":"uploaded"}\n' ;;
*) exit 1 ;;
esac
SH
chmod +x "$scratch/bin/gh" "$scratch/repo/scripts/validate-release-assets.sh" "$scratch/repo/scripts/release-restore-tag-ref.sh"
export PATH="$scratch/bin:$PATH" MOCK_CALLS="$scratch/calls"
git -C "$scratch/repo" init -q
git -C "$scratch/repo" -c user.name='Fixture Author' -c user.email='fixture@example.invalid' commit -q --allow-empty -m fixture
export STORAGEPRIMS_RELEASE_TAG=v1.2.3
STORAGEPRIMS_EXPECTED_COMMIT="$(git -C "$scratch/repo" rev-parse HEAD)"
export STORAGEPRIMS_EXPECTED_COMMIT
export STORAGEPRIMS_EXPECTED_TAG_OBJECT=0000000000000000000000000000000000000000
cd "$scratch/repo"
if MOCK_CREATE=conflict bash "$root/scripts/release-create-draft.sh" >/dev/null 2>&1; then
	echo 'error: create conflict was accepted' >&2
	exit 1
fi
[[ "$(cat "$MOCK_CALLS")" == $'recheck\ncreate' ]]
: >"$MOCK_CALLS"
if MOCK_UPLOAD=fail bash "$root/scripts/release-create-draft.sh" >/dev/null 2>&1; then
	echo 'error: partial draft upload was accepted' >&2
	exit 1
fi
[[ "$(cat "$MOCK_CALLS")" == $'recheck\ncreate\nupload' ]]
: >"$MOCK_CALLS"
bash "$root/scripts/release-create-draft.sh" >/dev/null
[[ "$(cat "$MOCK_CALLS")" == $'recheck\ncreate\nupload' ]]
echo '[ok] create-only draft conflict and partial upload controls passed'
