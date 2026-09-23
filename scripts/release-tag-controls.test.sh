#!/usr/bin/env bash
# shellcheck disable=SC2016
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd -P)"
while IFS= read -r target; do
	grep -Eq "^${target}:" "$root/Makefile" || {
		echo 'error: release checklist names a missing make target' >&2
		exit 1
	}
done < <(grep -oE 'make [a-z][a-z0-9-]+' "$root/RELEASE_CHECKLIST.md" | awk '{print $2}' | sort -u)
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
mkdir -p "$scratch/anchor-stage" "$scratch/anchor-dest"
for ext in txt ndjson; do
	printf 'new\n' >"$scratch/anchor-stage/expected-fingerprints.$ext"
	printf 'old\n' >"$scratch/anchor-dest/expected-fingerprints.$ext"
done
if STORAGEPRIMS_TEST_FAIL_ANCHOR_INSTALL=1 "$root/scripts/install-release-anchors.sh" \
	"$scratch/anchor-stage" "$scratch/anchor-dest" >/dev/null 2>&1; then
	echo 'error: expected pair rollback' >&2
	exit 1
fi
for ext in txt ndjson; do
	[[ "$(cat "$scratch/anchor-dest/expected-fingerprints.$ext")" == old ]]
done
"$root/scripts/install-release-anchors.sh" "$scratch/anchor-stage" "$scratch/anchor-dest"
for ext in txt ndjson; do
	[[ "$(cat "$scratch/anchor-dest/expected-fingerprints.$ext")" == new ]]
done
mkdir -p "$scratch/v1.2.3"
printf 'Release v1.2.3\n' >"$scratch/v1.2.3/message.txt"
export STORAGEPRIMS_RELEASE_TAG=v1.2.3
export STORAGEPRIMS_TAG_MESSAGE_DIR="$scratch/v1.2.3"
export STORAGEPRIMS_TAGGER_NAME='3 Leaps Infosec Team'
export STORAGEPRIMS_TAGGER_EMAIL='infosec@3leaps.net'
source "$root/scripts/release-tag-common.sh"
expect_fail() { if "$@" >/dev/null 2>&1; then
	echo "expected rejection: $*" >&2
	exit 1
fi; }
tag_identity
[[ "$(tag_message_file)" == "$scratch/v1.2.3/message.txt" ]]
printf 'Release v1.2.3' >"$scratch/v1.2.3/message.txt"
expect_fail tag_message_file
printf 'Release v1.2.3  \n' >"$scratch/v1.2.3/message.txt"
expect_fail tag_message_file
printf 'Release v1.2.3\n' >"$scratch/v1.2.3/message.txt"
expect_fail env STORAGEPRIMS_TAGGER_NAME=$'bad\nname' bash -c 'source "$1"; tag_identity' _ "$root/scripts/release-tag-common.sh"
expect_fail env STORAGEPRIMS_TAGGER_EMAIL='a<b>' bash -c 'source "$1"; tag_identity' _ "$root/scripts/release-tag-common.sh"
expect_fail env -u STORAGEPRIMS_TAG_MESSAGE_DIR bash -c 'source "$1"; tag_message_file' _ "$root/scripts/release-tag-common.sh"
expect_fail env STORAGEPRIMS_TAG_MESSAGE_DIR="$scratch" bash -c 'source "$1"; tag_message_file' _ "$root/scripts/release-tag-common.sh"
export STORAGEPRIMS_PGP_KEY_ID=0123456789ABCDEF
expect_fail tag_selector_shape
unset STORAGEPRIMS_PGP_KEY_ID
expect_fail tag_selector_shape
git init -q -b main "$scratch/repo"
git -C "$scratch/repo" config user.name fixture
git -C "$scratch/repo" config user.email fixture@example.invalid
printf 'x\n' >"$scratch/repo/file"
git -C "$scratch/repo" add file
git -C "$scratch/repo" commit -qm fixture
git -C "$scratch/repo" tag v1.2.3
export STORAGEPRIMS_PGP_KEY_ID=0123456789ABCDEF0123456789ABCDEF01234567!
(
	cd "$scratch/repo"
	expect_fail tag_verify_object refs/tags/v1.2.3 'Release v1.2.3'
)
mkdir -p "$scratch/bin"
cat >"$scratch/bin/gh" <<'SH'
#!/usr/bin/env bash
if [[ "$*" == *'rulesets?per_page=100'* ]]; then
	printf '[[{"id":7,"name":"Tag Publish Protection"}]]\n'
else
	cat "$RULESET_FIXTURE"
fi
SH
chmod +x "$scratch/bin/gh"
export PATH="$scratch/bin:$PATH" RULESET_FIXTURE="$scratch/ruleset.json"
cat >"$RULESET_FIXTURE" <<'JSON'
{"name":"Tag Publish Protection","source_type":"Repository","source":"3leaps/storageprims","target":"tag","enforcement":"active","conditions":{"ref_name":{"exclude":[],"include":["refs/tags/v*"]}},"rules":[{"type":"creation"},{"type":"deletion"},{"type":"non_fast_forward"},{"type":"update"}],"bypass_actors":[{"actor_id":null,"actor_type":"OrganizationAdmin","bypass_mode":"always"}]}
JSON
"$root/scripts/release-guard-tag-ruleset.sh" --read-only >/dev/null
for hidden in absent empty; do
	if [[ "$hidden" == absent ]]; then
		jq 'del(.bypass_actors)' "$scratch/ruleset.json" >"$scratch/hidden.json"
	else
		jq '.bypass_actors = []' "$scratch/ruleset.json" >"$scratch/hidden.json"
	fi
	export RULESET_FIXTURE="$scratch/hidden.json"
	"$root/scripts/release-guard-tag-ruleset.sh" --read-only >/dev/null
	expect_fail "$root/scripts/release-guard-tag-ruleset.sh"
done
export RULESET_FIXTURE="$scratch/ruleset.json"
jq '.bypass_actors = [{"actor_id":2,"actor_type":"Team","bypass_mode":"always"}]' "$RULESET_FIXTURE" >"$scratch/wrong-actor.json"
export RULESET_FIXTURE="$scratch/wrong-actor.json"
expect_fail "$root/scripts/release-guard-tag-ruleset.sh" --read-only
export RULESET_FIXTURE="$scratch/ruleset.json"
attestation="$("$root/scripts/release-guard-tag-ruleset.sh" --print-attestation)"
git -C "$scratch/repo" tag -d v1.2.3 >/dev/null
git -C "$scratch/repo" tag -a v1.2.3 -m 'Release v1.2.3' -m "$attestation"
(
	cd "$scratch/repo"
	"$root/scripts/release-guard-tag-ruleset.sh" --verify-tag-attestation refs/tags/v1.2.3 >/dev/null
)
git -C "$scratch/repo" update-ref refs/remotes/origin/main "$(git -C "$scratch/repo" rev-parse HEAD)"
git -C "$scratch/repo" tag -d v1.2.3 >/dev/null
printf 'Release v1.2.3\n\n%s\n' "$attestation" >"$scratch/expected-message"
printf 'Changed message\n\n%s\n' "$attestation" >"$scratch/wrong-message"
(
	cd "$scratch/repo"
	GIT_COMMITTER_NAME="$STORAGEPRIMS_TAGGER_NAME" GIT_COMMITTER_EMAIL="$STORAGEPRIMS_TAGGER_EMAIL" \
		git tag -a --cleanup=verbatim v1.2.3 -F "$scratch/expected-message"
)
(
	cd "$scratch/repo"
	tag_verify_object refs/tags/v1.2.3 "$scratch/expected-message"
)
(
	cd "$scratch/repo"
	expect_fail tag_verify_object refs/tags/v1.2.3 "$scratch/wrong-message"
)
printf 'Release v1.2.3\n\n%s\n\n\n' "$attestation" >"$scratch/extra-blank-lines"
(
	cd "$scratch/repo"
	GIT_COMMITTER_NAME="$STORAGEPRIMS_TAGGER_NAME" GIT_COMMITTER_EMAIL="$STORAGEPRIMS_TAGGER_EMAIL" \
		git tag -fa --cleanup=verbatim v1.2.3 -F "$scratch/extra-blank-lines" >/dev/null
	expect_fail tag_verify_object refs/tags/v1.2.3 "$scratch/expected-message"
	expect_fail "$root/scripts/release-guard-tag-ruleset.sh" --verify-tag-attestation refs/tags/v1.2.3
)
git -C "$scratch/repo" tag -f -a v1.2.3 -m 'Release v1.2.3' -m 'Tag-Publish-Policy-SHA256: stale' >/dev/null
(
	cd "$scratch/repo"
	expect_fail "$root/scripts/release-guard-tag-ruleset.sh" --verify-tag-attestation refs/tags/v1.2.3
)
sed 's/"enforcement":"active"/"enforcement":"disabled"/' "$RULESET_FIXTURE" >"$scratch/bad.json"
export RULESET_FIXTURE="$scratch/bad.json"
expect_fail "$root/scripts/release-guard-tag-ruleset.sh" --read-only
echo '[ok] signed-tag negative controls passed'
