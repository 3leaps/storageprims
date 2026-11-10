#!/usr/bin/env bash
# release-fetch-ci-artifacts.sh stages artifacts only from one successful push
# run of release.yml at the verified tag commit, with a matching release
# identity and the tagged commit's licenses, without contacting GitHub.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd -P)"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
repo="$scratch/repo"
mkdir -p "$repo/scripts" "$repo/config/release" "$scratch/bin" "$scratch/artifacts"
cp "$root/scripts/release-fetch-ci-artifacts.sh" "$repo/scripts/"
cat >"$repo/scripts/release-common.sh" <<'SH'
STORAGEPRIMS_REPOSITORY="3leaps/storageprims"
release_repo_root() { git rev-parse --show-toplevel; }
release_tag() { printf 'v1.2.3\n'; }
require_release_guard() {
	[[ "$1" == 1 && "${FIXTURE_GUARD:-ok}" == ok ]] || { echo 'error: strict guard fixture refused' >&2; return 1; }
}
SH
# shellcheck disable=SC2016 # expanded by the stub at run time
printf '#!/usr/bin/env bash\n[[ "${FIXTURE_PIN:-ok}" == ok ]]\n' >"$repo/scripts/verify-pinned-tag.sh"
cat >"$repo/scripts/validate-release-assets.sh" <<'SH'
#!/usr/bin/env bash
[[ "$2" == base ]]
diff <(ls "$1") <(printf '%s\n' LICENSE-APACHE LICENSE-MIT sbom-1.2.3.cdx.json storageprims-ffi-1.2.3-linux-amd64.tar.gz) >/dev/null || {
	echo 'error: base inventory mismatch' >&2
	exit 1
}
SH
chmod +x "$repo/scripts/"*.sh
printf 'linux-amd64 libstorageprims_ffi.so libstorageprims_ffi.a\n' >"$repo/config/release/ffi-platforms.txt"
printf 'MIT fixture\n' >"$repo/LICENSE-MIT"
printf 'Apache fixture\n' >"$repo/LICENSE-APACHE"
git -C "$repo" init -q
git -C "$repo" config user.name 'Fixture Author'
git -C "$repo" config user.email 'fixture@example.invalid'
git -C "$repo" add -A
git -C "$repo" commit -qm fixture
git -C "$repo" tag -am 'Fixture release' v1.2.3
commit="$(git -C "$repo" rev-parse HEAD)"
object="$(git -C "$repo" rev-parse refs/tags/v1.2.3)"

# Artifact fixtures, one directory per artifact name.
art="$scratch/artifacts"
mkdir -p "$art/release-identity" "$art/sbom" "$art/ffi-linux-amd64"
printf '{"tag":"v1.2.3","object":"%s","commit":"%s"}\n' "$object" "$commit" >"$art/release-identity/release-identity.json"
cp "$repo/LICENSE-MIT" "$repo/LICENSE-APACHE" "$art/release-identity/"
printf '{}\n' >"$art/sbom/sbom-1.2.3.cdx.json"
printf 'ffi\n' >"$art/ffi-linux-amd64/storageprims-ffi-1.2.3-linux-amd64.tar.gz"

# Runs the API reports: the wanted one (11), plus a failed run, a run at
# another commit and a pull_request run, which must never be selected.
cat >"$scratch/runs.json" <<JSON
[{"databaseId":11,"headSha":"$commit","conclusion":"success","event":"push"},
 {"databaseId":12,"headSha":"$commit","conclusion":"failure","event":"push"},
 {"databaseId":13,"headSha":"0000000000000000000000000000000000000000","conclusion":"success","event":"push"},
 {"databaseId":14,"headSha":"$commit","conclusion":"success","event":"pull_request"}]
JSON
cat >"$scratch/bin/gh" <<SH
#!/usr/bin/env bash
set -euo pipefail
args=("\$@")
jq_filter=''
for ((i = 0; i < \${#args[@]}; i++)); do
	[[ "\${args[i]}" != --jq ]] || jq_filter="\${args[i + 1]}"
done
case "\$1 \$2" in
'run list')
	[[ "\$*" == *'--workflow release.yml'* && "\$*" == *'--branch v1.2.3'* && "\$*" == *'--repo 3leaps/storageprims'* ]] || exit 1
	runs="\${FIXTURE_RUNS:-$scratch/runs.json}"
	jq -c "\$jq_filter" "\$runs" ;;
'api repos/3leaps/storageprims/actions/runs/'*)
	count_file="$scratch/api-count"
	count=\$((\$(cat "\$count_file" 2>/dev/null || echo 0) + 1))
	echo "\$count" >"\$count_file"
	attempt=1
	[[ "\${FIXTURE_RERUN:-no}" != yes || "\$count" -lt 2 ]] || attempt=2
	json="\$(printf '{"status":"%s","conclusion":"%s","event":"%s","head_sha":"%s","head_branch":"%s","path":"%s","run_attempt":%s}' "\${FIXTURE_STATUS:-completed}" "\${FIXTURE_CONCLUSION:-success}" "\${FIXTURE_EVENT:-push}" "\${FIXTURE_HEAD_SHA:-$commit}" "\${FIXTURE_HEAD_BRANCH:-v1.2.3}" "\${FIXTURE_PATH:-.github/workflows/release.yml}" "\$attempt")"
	if [[ -n "\$jq_filter" ]]; then jq -r "\$jq_filter" <<<"\$json"; else printf '%s\n' "\$json"; fi ;;
'run download')
	name='' dir=''
	for ((i = 0; i < \${#args[@]}; i++)); do
		[[ "\${args[i]}" != --name ]] || name="\${args[i + 1]}"
		[[ "\${args[i]}" != --dir ]] || dir="\${args[i + 1]}"
	done
	[[ "\$3" == "\${FIXTURE_EXPECT_RUN:-11}" ]] || exit 1
	mkdir -p "\$dir"
	cp -RP "$art/\$name/"* "\$dir/" ;;
*) printf 'unexpected gh call: %s\n' "\$*" >&2; exit 1 ;;
esac
SH
chmod +x "$scratch/bin/gh"
export PATH="$scratch/bin:$PATH" STORAGEPRIMS_RELEASE_TAG=v1.2.3
run() {
	rm -rf "$repo/dist" "$scratch/api-count"
	(cd "$repo" && bash scripts/release-fetch-ci-artifacts.sh) >"$scratch/out" 2>&1
}
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
export -f run
export repo scratch

# The one successful push run at the tag commit is staged, with its record.
run || {
	cat "$scratch/out" >&2
	exit 1
}
[[ "$(find "$repo/dist/release" -mindepth 1 -exec basename {} \; | LC_ALL=C sort | tr '\n' ' ')" == 'LICENSE-APACHE LICENSE-MIT sbom-1.2.3.cdx.json storageprims-ffi-1.2.3-linux-amd64.tar.gz ' ]]
grep -qx "object=$object" "$repo/dist/release.identity"
grep -qx 'run=11' "$repo/dist/release.identity"
grep -qx 'attempt=1' "$repo/dist/release.identity"

# shellcheck disable=SC2016 # expanded by the child shell
refused 'destination must be empty' bash -c 'mkdir -p "$repo/dist/release" && touch "$repo/dist/release/stale" && cd "$repo" && bash scripts/release-fetch-ci-artifacts.sh >"$scratch/out" 2>&1'
rm -rf "$repo/dist"
refused 'strict guard fixture refused' env FIXTURE_GUARD=bad bash -c run
refused 'does not verify against the committed pin' env FIXTURE_PIN=bad bash -c run
refused 'GH_REPO must be unset' env GH_REPO=other/repo bash -c run
# A selected run must be one of the successful push runs at the tag commit.
refused 'selected run is not a successful push run' env STORAGEPRIMS_RELEASE_RUN_ID=12 bash -c run
refused 'selected run is not a successful push run' env STORAGEPRIMS_RELEASE_RUN_ID=14 bash -c run
refused 'invalid selected run' env STORAGEPRIMS_RELEASE_RUN_ID=0 bash -c run
# Two candidates require an explicit choice; none is refused.
jq --arg c "$commit" '. + [{"databaseId":15,"headSha":$c,"conclusion":"success","event":"push"}]' "$scratch/runs.json" >"$scratch/two.json"
refused 'select one exact successful run' env FIXTURE_RUNS="$scratch/two.json" bash -c run
FIXTURE_RUNS="$scratch/two.json" STORAGEPRIMS_RELEASE_RUN_ID=15 FIXTURE_EXPECT_RUN=15 bash -c run || {
	cat "$scratch/out" >&2
	exit 1
}
printf '[]\n' >"$scratch/none.json"
refused 'select one exact successful run' env FIXTURE_RUNS="$scratch/none.json" bash -c run
refused 'workflow run identity mismatch' env FIXTURE_EVENT=workflow_dispatch bash -c run
refused 'workflow run identity mismatch' env FIXTURE_PATH=.github/workflows/other.yml bash -c run
refused 'run attempt changed during download' env FIXTURE_RERUN=yes bash -c run
# Each run field is checked on the run itself, not only by the list filter.
refused 'workflow run identity mismatch' env FIXTURE_STATUS=in_progress bash -c run
refused 'workflow run identity mismatch' env FIXTURE_CONCLUSION=failure bash -c run
refused 'workflow run identity mismatch' env FIXTURE_HEAD_SHA=0000000000000000000000000000000000000000 bash -c run
refused 'workflow run identity mismatch' env FIXTURE_HEAD_BRANCH=main bash -c run
# Artifact content must name the verified tag object and carry its licenses.
cp "$art/release-identity/release-identity.json" "$scratch/identity.keep"
printf '{"tag":"v1.2.3","object":"0000000000000000000000000000000000000000","commit":"%s"}\n' "$commit" >"$art/release-identity/release-identity.json"
refused 'artifact tag object or commit mismatch' bash -c run
printf '{"tag":"v1.2.3","object":"%s","commit":"%s","extra":1}\n' "$object" "$commit" >"$art/release-identity/release-identity.json"
refused 'artifact tag object or commit mismatch' bash -c run
cp "$scratch/identity.keep" "$art/release-identity/release-identity.json"
printf 'changed\n' >"$art/release-identity/LICENSE-MIT"
refused 'LICENSE-MIT differs from the tagged commit' bash -c run
cp "$repo/LICENSE-MIT" "$art/release-identity/LICENSE-MIT"
printf 'dup\n' >"$art/sbom/LICENSE-MIT"
refused 'duplicate artifact basename' bash -c run
rm "$art/sbom/LICENSE-MIT"
ln -s /etc/hosts "$art/sbom/linked.json"
refused 'unsafe artifact entry' bash -c run
rm "$art/sbom/linked.json"
# A second identity record, an unsafe name, an inventory the validator
# refuses, and a symlinked destination are refused.
cp "$art/release-identity/release-identity.json" "$art/sbom/release-identity.json"
mkdir -p "$art/sbom/nested" && mv "$art/sbom/release-identity.json" "$art/sbom/nested/"
refused 'duplicate artifact release identity' bash -c run
rm -rf "$art/sbom/nested"
printf 'x\n' >"$art/sbom/bad name.json"
refused 'unsafe artifact name' bash -c run
rm "$art/sbom/bad name.json"
printf 'x\n' >"$art/sbom/extra.json"
refused 'base inventory mismatch' bash -c run
rm "$art/sbom/extra.json"
rm -rf "$repo/dist" && mkdir -p "$scratch/elsewhere" && ln -s "$scratch/elsewhere" "$repo/dist"
# shellcheck disable=SC2016 # expanded by the child shell
refused 'unsafe artifact destination' bash -c 'cd "$repo" && bash scripts/release-fetch-ci-artifacts.sh >"$scratch/out" 2>&1'
rm "$repo/dist"
# A failed re-fetch leaves no record behind.
run
printf 'changed\n' >"$art/release-identity/LICENSE-MIT"
rm -rf "$repo/dist/release" && mkdir -p "$repo/dist/release"
if (cd "$repo" && bash scripts/release-fetch-ci-artifacts.sh) >"$scratch/out" 2>&1; then
	echo 'error: changed license accepted' >&2
	exit 1
fi
[[ ! -e "$repo/dist/release.identity" ]] || {
	echo 'error: a failed fetch left the previous record in place' >&2
	exit 1
}
cp "$repo/LICENSE-MIT" "$art/release-identity/LICENSE-MIT"
run || {
	cat "$scratch/out" >&2
	exit 1
}
echo '[ok] CI artifact fetch: one successful push run at the tag commit, explicit choice among several, stable attempt, matching identity and licenses; refusals for a dirty destination, guard, pin, GH_REPO, wrong run, event, path, rerun, identity, license, duplicate and symlink'
