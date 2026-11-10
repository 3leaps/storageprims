#!/usr/bin/env bash
# upload-release-assets.sh verifies the draft's unsigned bytes first, uploads
# only the signed additions and the notes to that verified release id (never
# by tag), and re-verifies the complete signed set under the same id, without
# contacting GitHub.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd -P)"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
repo="$scratch/repo"
mkdir -p "$repo/scripts" "$repo/dist/release" "$scratch/bin"
cp "$root/scripts/upload-release-assets.sh" "$repo/scripts/"
cat >"$repo/scripts/release-common.sh" <<'SH'
STORAGEPRIMS_REPOSITORY="3leaps/storageprims"
release_repo_root() { git rev-parse --show-toplevel; }
release_tag() { printf 'v1.2.3\n'; }
release_signed_assets() { printf '%s\n' LICENSE-MIT storageprims-ffi-1.2.3-linux-amd64.tar.gz SHA256SUMS SHA256SUMS.minisig; }
require_release_guard() { [[ "$1" == 1 ]]; }
SH
for stub in validate-release-assets.sh verify-checksums.sh verify-public-keys.sh verify-signatures.sh; do
	printf '#!/usr/bin/env bash\nexit 0\n' >"$repo/scripts/$stub"
done
cat >"$repo/scripts/release-verify-draft.sh" <<'SH'
#!/usr/bin/env bash
printf 'verify %s %s\n' "$2" "${STORAGEPRIMS_EXPECTED_DRAFT_ID:-}" >>"$MOCK_CALLS"
[[ "$2" != base || "${MOCK_VERIFY_BASE:-ok}" == ok ]] || { echo 'error: draft asset differs' >&2; exit 1; }
[[ "$2" != signed || "${MOCK_VERIFY_SIGNED:-ok}" == ok ]] || { echo 'error: signed set differs' >&2; exit 1; }
[[ "$2" != base || "${MOCK_NO_RECORD:-no}" == yes ]] || printf 'id=55\n' >"$(git rev-parse --show-toplevel)/dist/release.draft"
SH
chmod +x "$repo/scripts/"*.sh
cat >"$scratch/bin/gh" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
case "$*" in
*'--method POST -H Content-Type: application/octet-stream https://uploads.github.com/repos/3leaps/storageprims/releases/55/assets?name='*)
	name=''
	for arg in "$@"; do [[ "$arg" != *'?name='* ]] || name="${arg##*\?name=}"; done
	printf 'upload %s\n' "$name" >>"$MOCK_CALLS"
	[[ "${MOCK_UPLOAD:-ok}" != fail ]] || exit 1
	[[ "${MOCK_UPLOAD:-ok}" != partial ]] || {
		printf '{"name":"%s","state":"starter"}\n' "$name"
		exit 0
	}
	printf '{"name":"%s","state":"uploaded"}\n' "$name" ;;
'api --method PATCH repos/3leaps/storageprims/releases/55 --input -')
	body="$(cat)"
	jq -e '.draft == true and (.body | test("Fixture notes"))' <<<"$body" >/dev/null || exit 1
	printf 'notes\n' >>"$MOCK_CALLS"
	printf '{"id":55,"draft":%s}\n' "${MOCK_PATCH_DRAFT:-true}" ;;
*) printf 'unexpected gh call: %s\n' "$*" >&2; exit 1 ;;
esac
SH
chmod +x "$scratch/bin/gh"
for name in LICENSE-MIT storageprims-ffi-1.2.3-linux-amd64.tar.gz SHA256SUMS SHA256SUMS.minisig; do
	printf '%s\n' "$name" >"$repo/dist/release/$name"
done
printf 'Fixture notes\n' >"$repo/dist/release/release-notes-v1.2.3.md"
git -C "$repo" init -q
export PATH="$scratch/bin:$PATH" MOCK_CALLS="$scratch/calls" STORAGEPRIMS_RELEASE_TAG=v1.2.3
run() {
	: >"$MOCK_CALLS"
	rm -f "$repo/dist/release.draft"
	(cd "$repo" && bash scripts/upload-release-assets.sh dist/release) >"$scratch/out" 2>&1
}
refused() {
	local reason="$1" calls="$2"
	shift 2
	if env "$@" bash -c "$(declare -f run); run"; then
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
export -f run
export repo scratch

full=$'verify base \nupload SHA256SUMS\nupload SHA256SUMS.minisig\nnotes\nverify signed 55'
run || {
	cat "$scratch/out" >&2
	exit 1
}
[[ "$(cat "$MOCK_CALLS")" == "$full" ]] || {
	printf 'error: unexpected call order:\n%s\n' "$(cat "$MOCK_CALLS")" >&2
	exit 1
}
# Nothing is mutated unless the draft still holds the verified unsigned bytes.
refused 'draft asset differs' 'verify base ' MOCK_VERIFY_BASE=bad
refused 'verified draft id is unavailable' 'verify base ' MOCK_NO_RECORD=yes
refused 'signed asset upload failed' $'verify base \nupload SHA256SUMS' MOCK_UPLOAD=fail
refused 'signed asset upload response invalid' $'verify base \nupload SHA256SUMS' MOCK_UPLOAD=partial
refused 'did not leave the verified draft' $'verify base \nupload SHA256SUMS\nupload SHA256SUMS.minisig\nnotes' MOCK_PATCH_DRAFT=false
refused 'signed set differs' "$full" MOCK_VERIFY_SIGNED=bad
echo '[ok] signed upload: base bytes verified first, signed additions and notes sent to the verified release id only, final signed-set verification bound to that id; refusals stop before or at the failing mutation'
