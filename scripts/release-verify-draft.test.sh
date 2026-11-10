#!/usr/bin/env bash
# shellcheck disable=SC2016 # bash -c bodies expand in the child shell
# release-verify-draft.sh accepts only the one draft for the tag, at the tag
# commit, holding exactly the staged unsigned base assets byte for byte,
# without contacting GitHub.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd -P)"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
repo="$scratch/repo"
mkdir -p "$repo/scripts" "$repo/dist/release" "$scratch/bin" "$scratch/remote"
cp "$root/scripts/release-verify-draft.sh" "$repo/scripts/"
cat >"$repo/scripts/release-common.sh" <<'SH'
STORAGEPRIMS_REPOSITORY="3leaps/storageprims"
release_repo_root() { git rev-parse --show-toplevel; }
release_tag() { printf 'v1.2.3\n'; }
release_base_assets() { printf '%s\n' LICENSE-MIT storageprims-ffi-1.2.3-linux-amd64.tar.gz; }
release_signed_assets() { release_base_assets; printf '%s\n' SHA256SUMS; }
require_release_guard() { [[ "$1" == 1 ]]; }
SH
git -C "$repo" init -q
git -C "$repo" config user.name 'Fixture Author'
git -C "$repo" config user.email 'fixture@example.invalid'
git -C "$repo" commit -q --allow-empty -m fixture
git -C "$repo" tag -am 'Fixture release' v1.2.3
commit="$(git -C "$repo" rev-parse HEAD)"
object="$(git -C "$repo" rev-parse refs/tags/v1.2.3)"
printf 'tag=v1.2.3\nobject=%s\ncommit=%s\nrun=7\nattempt=1\n' "$object" "$commit" >"$repo/dist/release.identity"
printf 'MIT fixture\n' >"$repo/dist/release/LICENSE-MIT"
printf 'ffi bytes\n' >"$repo/dist/release/storageprims-ffi-1.2.3-linux-amd64.tar.gz"
# Remote draft assets by id.
cp "$repo/dist/release/LICENSE-MIT" "$scratch/remote/101"
cp "$repo/dist/release/storageprims-ffi-1.2.3-linux-amd64.tar.gz" "$scratch/remote/102"
releases() {
	printf '[{"id":9,"tag_name":"v1.2.2","draft":false,"target_commitish":"main"},'
	printf '{"id":55,"tag_name":"v1.2.3","draft":%s,"target_commitish":"%s"}%s]\n' \
		"${1:-true}" "${2:-$commit}" "${3:-}"
}
export -f releases
export commit
export FIXTURE_REMOTE="$scratch/remote"
# shellcheck disable=SC2089,SC2090 # JSON consumed by the gh stub
assets_json='[{"id":101,"name":"LICENSE-MIT"},{"id":102,"name":"storageprims-ffi-1.2.3-linux-amd64.tar.gz"}]'
# shellcheck disable=SC2090
export assets_json
cat >"$scratch/bin/gh" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
args=("$@")
jq_filter=''
for ((i = 0; i < ${#args[@]}; i++)); do
	[[ "${args[i]}" != --jq ]] || jq_filter="${args[i + 1]}"
done
case "$*" in
'api repos/3leaps/storageprims --jq .permissions.push')
	printf '%s\n' "${MOCK_PUSH:-true}" ;;
'api --paginate repos/3leaps/storageprims/releases?per_page=100 --jq '*)
	[[ "${MOCK_LIST:-ok}" == ok ]] || exit 1
	releases "${MOCK_DRAFT:-true}" "${MOCK_TARGET:-$commit}" "${MOCK_EXTRA:-}" | jq -c "$jq_filter" ;;
'api --paginate repos/3leaps/storageprims/releases/55/assets?per_page=100 --jq '*)
	printf '%s\n' "${MOCK_ASSETS:-$assets_json}" | jq -r "$jq_filter" ;;
'api -H Accept: application/octet-stream repos/3leaps/storageprims/releases/assets/'*)
	cat "$FIXTURE_REMOTE/${args[${#args[@]} - 1]##*/}" ;;
*) printf 'unexpected gh call: %s\n' "$*" >&2; exit 1 ;;
esac
SH
chmod +x "$scratch/bin/gh"
export PATH="$scratch/bin:$PATH" STORAGEPRIMS_RELEASE_TAG=v1.2.3 repo
run() { (cd "$repo" && bash scripts/release-verify-draft.sh dist/release) >"$FIXTURE_OUT" 2>&1; }
export FIXTURE_OUT="$scratch/out"
export -f run
refused() {
	local reason="$1"
	shift
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
}

run || {
	cat "$scratch/out" >&2
	exit 1
}
grep -q 'draft 55 holds exactly the verified base assets' "$scratch/out"
grep -qx 'id=55' "$repo/dist/release.draft"

refused 'the GitHub identity cannot see drafts' env MOCK_PUSH=false bash -c run
refused 'the release list could not be read' env MOCK_LIST=fail bash -c run
refused 'expected exactly one release for the tag' env MOCK_EXTRA=',{"id":56,"tag_name":"v1.2.3","draft":true,"target_commitish":"x"}' bash -c run
refused 'is not a draft at the tag commit' env MOCK_DRAFT=false bash -c run
refused 'is not a draft at the tag commit' env MOCK_TARGET=0000000000000000000000000000000000000000 bash -c run
refused 'not exactly the base set' env MOCK_ASSETS='[{"id":101,"name":"LICENSE-MIT"}]' bash -c run
refused 'not exactly the base set' env MOCK_ASSETS='[{"id":101,"name":"LICENSE-MIT"},{"id":102,"name":"storageprims-ffi-1.2.3-linux-amd64.tar.gz"},{"id":103,"name":"SHA256SUMS"}]' bash -c run
# A replaced draft asset is refused even with the right name and size class.
cp "$scratch/remote/102" "$scratch/remote/102.keep"
printf 'ffi BYTES\n' >"$scratch/remote/102"
refused 'differs from the verified local base set' bash -c run
cp "$scratch/remote/102.keep" "$scratch/remote/102"
# Bound to the verified release id; a different draft for the tag is refused,
# and a refused run leaves no id record behind.
refused 'not the verified release' env STORAGEPRIMS_EXPECTED_DRAFT_ID=56 bash -c run
[[ ! -e "$repo/dist/release.draft" ]] || {
	echo 'error: a refused verify left an id record' >&2
	exit 1
}
refused 'invalid expected draft id' env STORAGEPRIMS_EXPECTED_DRAFT_ID=abc bash -c run
STORAGEPRIMS_EXPECTED_DRAFT_ID=55 bash -c run || {
	cat "$scratch/out" >&2
	exit 1
}
# Signed mode compares the complete signed set byte for byte.
refused 'not exactly the signed set' bash -c 'cd "$repo" && bash scripts/release-verify-draft.sh dist/release signed >"$FIXTURE_OUT" 2>&1'
printf 'sums\n' >"$repo/dist/release/SHA256SUMS"
printf 'sums\n' >"$scratch/remote/103"
signed_assets='[{"id":101,"name":"LICENSE-MIT"},{"id":102,"name":"storageprims-ffi-1.2.3-linux-amd64.tar.gz"},{"id":103,"name":"SHA256SUMS"}]'
MOCK_ASSETS="$signed_assets" bash -c 'cd "$repo" && bash scripts/release-verify-draft.sh dist/release signed >"$FIXTURE_OUT" 2>&1' || {
	cat "$scratch/out" >&2
	exit 1
}
printf 'SUMS\n' >"$scratch/remote/103"
refused 'SHA256SUMS differs' env MOCK_ASSETS="$signed_assets" bash -c 'cd "$repo" && bash scripts/release-verify-draft.sh dist/release signed >"$FIXTURE_OUT" 2>&1'
rm "$repo/dist/release/SHA256SUMS"
refused 'mode must be base or signed' bash -c 'cd "$repo" && bash scripts/release-verify-draft.sh dist/release other >"$FIXTURE_OUT" 2>&1'
# The staged record must exist and name this tag object.
mv "$repo/dist/release.identity" "$scratch/identity.keep"
refused 'staged artifact record is absent' bash -c run
printf 'tag=v1.2.3\nobject=0000000000000000000000000000000000000000\ncommit=%s\n' "$commit" >"$repo/dist/release.identity"
refused 'fetched for another tag object' bash -c run
mv "$scratch/identity.keep" "$repo/dist/release.identity"
run || {
	cat "$scratch/out" >&2
	exit 1
}
echo '[ok] draft verify: exactly one draft at the tag commit holding the staged base assets byte for byte; refusals for no draft visibility, unreadable list, two releases, published or misdirected release, missing or extra assets, replaced bytes, and a missing or foreign fetch record'
