#!/usr/bin/env bash
# Create-only conflict and partial-upload controls without contacting GitHub.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd -P)"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
mkdir -p "$scratch/repo/scripts" "$scratch/repo/docs/releases" "$scratch/repo/dist/release" "$scratch/bin"
printf 'Release notes\n' >"$scratch/repo/docs/releases/v0.1.2.md"
printf 'fixture\n' >"$scratch/repo/dist/release/asset.txt"
cat >"$scratch/repo/scripts/validate-release-assets.sh" <<'SH'
#!/usr/bin/env bash
[[ "$1" == dist/release && "$2" == base && -f "$1/asset.txt" ]]
SH
cat >"$scratch/repo/scripts/release-common.sh" <<'SH'
release_base_assets() { printf 'asset.txt\n'; }
SH
cat >"$scratch/bin/gh" <<'SH'
#!/usr/bin/env bash
case "$*" in
*'--method POST repos/3leaps/storageprims/releases '* | *'--method POST repos/3leaps/storageprims/releases')
	[[ "$*" == *'-F draft=true'* && "$*" == *'-F prerelease=false'* ]] || exit 1
	printf 'create\n' >>"$MOCK_CALLS"
	case "${MOCK_CREATE:-ok}" in
	ok) printf '{"id":123,"tag_name":"v0.1.2","target_commitish":"1666a2ee3166f2b764514fef36794dfd514967d1","draft":true,"prerelease":false}\n' ;;
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
chmod +x "$scratch/bin/gh" "$scratch/repo/scripts/validate-release-assets.sh"
export PATH="$scratch/bin:$PATH" MOCK_CALLS="$scratch/calls"
export STORAGEPRIMS_RELEASE_TAG=v0.1.2
export STORAGEPRIMS_EXPECTED_COMMIT=1666a2ee3166f2b764514fef36794dfd514967d1
cd "$scratch/repo"
if MOCK_CREATE=conflict bash "$root/scripts/release-recovery-create-draft.sh" >/dev/null 2>&1; then
	echo 'error: create conflict was accepted' >&2
	exit 1
fi
[[ "$(cat "$MOCK_CALLS")" == create ]]
: >"$MOCK_CALLS"
if MOCK_UPLOAD=fail bash "$root/scripts/release-recovery-create-draft.sh" >/dev/null 2>&1; then
	echo 'error: partial draft upload was accepted' >&2
	exit 1
fi
[[ "$(cat "$MOCK_CALLS")" == $'create\nupload' ]]
: >"$MOCK_CALLS"
bash "$root/scripts/release-recovery-create-draft.sh" >/dev/null
[[ "$(cat "$MOCK_CALLS")" == $'create\nupload' ]]
echo '[ok] create-only draft conflict and partial upload controls passed'
