#!/usr/bin/env bash
# Exercise publication with a local GitHub stub; never contact a remote.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd -P)"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
repo="$scratch/repo"
mkdir -p "$repo/scripts" "$repo/dist/release" "$scratch/bin"
cp "$root/scripts/release-publish.sh" "$repo/scripts/"
cat >"$repo/scripts/release-common.sh" <<'STUB'
STORAGEPRIMS_REPOSITORY=3leaps/storageprims
release_repo_root() { git rev-parse --show-toplevel; }
release_tag() { echo v1.2.3; }
require_release_guard() { :; }
release_signed_assets() { printf '%s\n' LICENSE-MIT SHA256SUMS; }
STUB
for name in validate-release-assets verify-checksums verify-public-keys verify-signatures release-verify-draft release-github-preflight release-verify-remote-tag; do
	cat >"$repo/scripts/$name.sh" <<'STUB'
#!/usr/bin/env bash
name="$(basename "$0" .sh)"
echo "$name" >>"$MOCK_CALLS"
[[ "${MOCK_FAIL:-}" != "$name" ]] || exit 1
if [[ "$name" == release-verify-draft ]]; then
 [[ "$2" == signed && "$STORAGEPRIMS_EXPECTED_DRAFT_ID" == 55 ]] || exit 1
fi
STUB
done
chmod +x "$repo/scripts/"*.sh
git -C "$repo" init -q
git -C "$repo" config user.name 'Fixture Author'
git -C "$repo" config user.email 'fixture@example.invalid'
git -C "$repo" commit -q --allow-empty -m fixture
git -C "$repo" tag -am fixture v1.2.3
FIXTURE_COMMIT="$(git -C "$repo" rev-parse HEAD)"
export FIXTURE_COMMIT
cat >"$scratch/bin/gh" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
state() {
 jq -n --arg commit "$FIXTURE_COMMIT" --arg tag "${MOCK_TAG:-v1.2.3}"   --arg body "${MOCK_BODY:-Fixture notes}" --argjson prerelease "${MOCK_PRERELEASE:-false}" --argjson immutable "${MOCK_IMMUTABLE:-true}" --argjson id "${MOCK_ID:-55}"   '{id:$id,tag_name:$tag,target_commitish:$commit,draft:false,immutable:$immutable,prerelease:$prerelease,body:($body+"\n")}'
}
case "$*" in
'api --method PATCH repos/3leaps/storageprims/releases/55 --input -')
 body="$(cat)"
 jq -e '.tag_name == "v1.2.3" and .draft == false and .make_latest == "true" and .prerelease == false and .body == "Fixture notes\n"' <<<"$body" >/dev/null
 echo patch >>"$MOCK_CALLS"
 [[ "${MOCK_PATCH:-ok}" == ok ]] || exit 1
 state ;;
'api repos/3leaps/storageprims/releases/tags/v1.2.3')
 echo get >>"$MOCK_CALLS"
 [[ "${MOCK_GET:-ok}" == ok ]] || exit 1
 state ;;
'api --paginate repos/3leaps/storageprims/releases/55/assets?per_page=100 --jq .[].name')
 echo assets >>"$MOCK_CALLS"
 printf '%s\n' LICENSE-MIT
 [[ "${MOCK_ASSETS:-ok}" != missing ]] && echo SHA256SUMS
 [[ "${MOCK_ASSETS:-ok}" != duplicate ]] || echo SHA256SUMS
 [[ "${MOCK_ASSETS:-ok}" != extra ]] || echo extra
 ;;
*) echo "unexpected gh: $*" >&2; exit 1 ;;
esac
STUB
chmod +x "$scratch/bin/gh"
export PATH="$scratch/bin:$PATH" MOCK_CALLS="$scratch/calls" repo scratch
run() {
	: >"$MOCK_CALLS"
	(cd "$repo" && bash scripts/release-publish.sh) >"$scratch/out" 2>&1
}
export -f run
refused() {
	if env "$@" bash -c run; then
		echo "error: publication accepted: $*" >&2
		exit 1
	fi
}
printf 'Fixture notes\n' >"$repo/dist/release/release-notes-v1.2.3.md"
printf 'id=55\n' >"$repo/dist/release.draft"
run || {
	cat "$scratch/out" >&2
	exit 1
}
expected=$'validate-release-assets\nverify-checksums\nverify-public-keys\nverify-signatures\nrelease-verify-draft\nrelease-github-preflight\nrelease-verify-remote-tag\npatch\nget\nassets'
[[ "$(cat "$MOCK_CALLS")" == "$expected" ]]
for gate in validate-release-assets verify-checksums verify-public-keys verify-signatures release-verify-draft release-github-preflight release-verify-remote-tag; do
	refused MOCK_FAIL="$gate"
	if grep -qx patch "$MOCK_CALLS"; then
		echo "error: mutation after failed gate" >&2
		exit 1
	fi
done
refused MOCK_PRERELEASE=true
refused MOCK_BODY=wrong
refused MOCK_IMMUTABLE=false
refused MOCK_ID=56
refused MOCK_TAG=untagged-fixture
refused MOCK_PATCH=fail
refused MOCK_GET=fail
refused MOCK_ASSETS=missing
refused MOCK_ASSETS=duplicate
refused MOCK_ASSETS=extra
refused GH_REPO=example/other
printf 'id=bad\n' >"$repo/dist/release.draft"
refused
rm "$repo/dist/release.draft"
refused
echo '[ok] publish: signed draft and remote tag verified before tag-bound PATCH; immutable identity and exact inventory required; failures stop publication'
