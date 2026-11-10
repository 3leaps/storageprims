#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# release-restore-tag-ref.sh resolves the tag through the GitHub API with the
# job token (a stub curl answers from a fixture remote), so it works where an
# anonymous ls-remote cannot (a private repository). It repairs the peeled ref
# actions/checkout leaves, and refuses an absent, lightweight, replaced or
# misdirected tag, a missing token, and a malformed API answer. origin is asked
# even when the object is local, and must agree with the API answer.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd -P)"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
git init -q --bare -b main "$scratch/remote.git"
git init -q -b main "$scratch/source"
git -C "$scratch/source" config user.name 'Fixture Author'
git -C "$scratch/source" config user.email 'fixture@example.invalid'
printf 'fixture\n' >"$scratch/source/file"
printf '1.2.3\n' >"$scratch/source/VERSION"
cat >"$scratch/source/guard.sh" <<'SH'
#!/usr/bin/env bash
# The strict check every release step applies: the tag is annotated and is HEAD.
set -euo pipefail
[[ "$(git cat-file -t "refs/tags/$STORAGEPRIMS_RELEASE_TAG")" == tag ]]
[[ "$(git rev-parse "refs/tags/$STORAGEPRIMS_RELEASE_TAG^{}")" == "$(git rev-parse HEAD)" ]]
SH
git -C "$scratch/source" add file VERSION guard.sh
git -C "$scratch/source" commit -qm 'fixture commit'
git -C "$scratch/source" tag -am 'Fixture release' v1.2.3
commit="$(git -C "$scratch/source" rev-parse HEAD)"
object="$(git -C "$scratch/source" rev-parse refs/tags/v1.2.3)"
git -C "$scratch/source" remote add origin "$scratch/remote.git"
git -C "$scratch/source" push -q origin main refs/tags/v1.2.3
git clone -q "$scratch/remote.git" "$scratch/checkout"
git -C "$scratch/checkout" checkout -q --detach "$commit"
git -C "$scratch/checkout" fetch -q --no-tags origin "+$commit:refs/tags/v1.2.3"
[[ "$(git -C "$scratch/checkout" cat-file -t refs/tags/v1.2.3)" == commit ]]

# A stub curl answers the API lookup from the fixture remote, and records the
# token it was given so the test can check it arrived on stdin, not in argv.
mkdir -p "$scratch/bin"
cat >"$scratch/bin/curl" <<SH
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "\$*" >>"$scratch/curl.argv"
cat >"$scratch/curl.config"
url="\${!#}"
tag="\${url##*/}"
[[ "\${FIXTURE_API_MODE:-ok}" != fail ]] || exit 22
if [[ "\${FIXTURE_API_MODE:-ok}" == garbage ]]; then printf 'not json'; exit 0; fi
line="\$(git -C "$scratch/remote.git" for-each-ref --format='%(objectname) %(objecttype)' "refs/tags/\$tag")"
[[ -n "\$line" ]] || exit 22
sha="\${line%% *}" type="\${line##* }"
sha="\${FIXTURE_API_SHA:-\$sha}"
ref="refs/tags/\$tag"
[[ "\${FIXTURE_API_MODE:-ok}" != otherref ]] || ref="refs/tags/v9.9.9"
printf '{"ref":"%s","object":{"type":"%s","sha":"%s"}}' "\$ref" "\$type" "\$sha"
SH
chmod +x "$scratch/bin/curl"
export PATH="$scratch/bin:$PATH" GITHUB_TOKEN=fixture-job-token
# The suite runs inside GitHub Actions too; each case sets GITHUB_ACTIONS itself.
unset GITHUB_ACTIONS
restore() { STORAGEPRIMS_RELEASE_TAG="$1" bash "$root/scripts/release-restore-tag-ref.sh" >"$scratch/out" 2>&1; }
# run_env VAR=... ...: the restore script under extra environment, captured.
run_env() { env "$@" bash "$root/scripts/release-restore-tag-ref.sh" >"$scratch/out" 2>&1; }
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
cd "$scratch/checkout"
# A clone does not inherit the source's identity; the fixture's annotated tags
# need one even on a host with no global git identity.
git config user.name 'Fixture Author'
git config user.email 'fixture@example.invalid'
git remote set-url origin https://github.com/3leaps/storageprims.git
# Redirect only the fixture's fetch transport, not the origin identity check.
git config url."$scratch/remote.git".insteadOf https://github.com/3leaps/storageprims.git
if STORAGEPRIMS_RELEASE_TAG=v1.2.3 bash ./guard.sh >/dev/null 2>&1; then
	echo 'error: strict guard accepted the checkout peeled-ref rewrite' >&2
	exit 1
fi

# The peeled ref is repaired from the API answer, and the guard then passes.
restore v1.2.3 || {
	cat "$scratch/out" >&2
	exit 1
}
[[ "$(git rev-parse refs/tags/v1.2.3)" == "$object" && "$(git cat-file -t refs/tags/v1.2.3)" == tag ]]
STORAGEPRIMS_RELEASE_TAG=v1.2.3 bash ./guard.sh >/dev/null
# The token reached curl on its config (stdin), never in argv.
grep -q 'Authorization: Bearer fixture-job-token' "$scratch/curl.config"
# Default curl configuration and environment proxies are never consulted.
[[ "$(head -1 "$scratch/curl.argv" | cut -d' ' -f1)" == --disable ]] || {
	echo 'error: curl --disable is not the first argument' >&2
	exit 1
}
grep -q -- "--noproxy \\*" "$scratch/curl.argv" || {
	echo 'error: curl does not refuse environment proxies' >&2
	exit 1
}
! grep -q 'fixture-job-token' "$scratch/curl.argv" || {
	echo 'error: the token appeared in curl argv' >&2
	exit 1
}

# No anonymous lookup: without the job token it refuses, even though a public
# repository would answer an anonymous ls-remote.
refused 'GITHUB_TOKEN (the job token) is required' run_env -u GITHUB_TOKEN STORAGEPRIMS_RELEASE_TAG=v1.2.3
refused 'remote release tag is absent' restore v1.2.4
refused 'remote release tag is absent' run_env FIXTURE_API_MODE=fail STORAGEPRIMS_RELEASE_TAG=v1.2.3
refused 'invalid JSON' run_env FIXTURE_API_MODE=garbage STORAGEPRIMS_RELEASE_TAG=v1.2.3
refused 'named another ref' run_env FIXTURE_API_MODE=otherref STORAGEPRIMS_RELEASE_TAG=v1.2.3
refused 'must be reached over https' run_env STORAGEPRIMS_GITHUB_API=http://api.example.invalid STORAGEPRIMS_RELEASE_TAG=v1.2.3
# A workflow cannot redirect the lookup, even to another https host.
refused 'not accepted under GitHub Actions' run_env GITHUB_ACTIONS=true STORAGEPRIMS_GITHUB_API=https://api.example.invalid STORAGEPRIMS_RELEASE_TAG=v1.2.3
# The API names another annotated object at HEAD that the checkout already
# holds; origin's ref still names the released object, so it is refused. A
# configured tag fetch mapping must not move the local ref before the
# comparison: start from the peeled ref actions/checkout leaves.
git config --add remote.origin.fetch '+refs/tags/*:refs/tags/*'
git tag -a -m 'Local substitute' local-substitute HEAD
substitute="$(git rev-parse refs/tags/local-substitute)"
git update-ref refs/tags/v1.2.3 "$commit"
refused "origin's tag object differs from the API answer" run_env FIXTURE_API_SHA="$substitute" STORAGEPRIMS_RELEASE_TAG=v1.2.3
[[ "$(git rev-parse refs/tags/v1.2.3)" == "$commit" ]] || {
	echo 'error: a fetch mapping moved the local tag before the comparison' >&2
	exit 1
}
git tag -d local-substitute >/dev/null
# With that mapping, an agreeing origin still restores the tag.
restore v1.2.3 || {
	cat "$scratch/out" >&2
	exit 1
}
[[ "$(git rev-parse refs/tags/v1.2.3)" == "$object" ]]
git config --unset-all remote.origin.fetch '\+refs/tags/\*:refs/tags/\*'
# The object is local, but origin cannot be read: refused, not trusted from cache.
git config --unset url."$scratch/remote.git".insteadOf
git config url."$scratch/absent.git".insteadOf https://github.com/3leaps/storageprims.git
refused 'origin release tag could not be fetched' restore v1.2.3
git config --unset url."$scratch/absent.git".insteadOf
git config url."$scratch/remote.git".insteadOf https://github.com/3leaps/storageprims.git
cd "$root"

# A different annotated object at the same commit is refused against the
# validated object.
git -C "$scratch/source" tag -fa -m 'Different signature fixture' v1.2.3
git -C "$scratch/source" push -q --force origin refs/tags/v1.2.3
(
	cd "$scratch/checkout"
	refused 'differs from validated object' run_env STORAGEPRIMS_EXPECTED_TAG_OBJECT="$object" STORAGEPRIMS_RELEASE_TAG=v1.2.3
	[[ "$(git rev-parse refs/tags/v1.2.3)" == "$object" ]]
)
# A lightweight remote tag is refused.
git -C "$scratch/remote.git" update-ref refs/tags/v1.2.3 "$commit"
(
	cd "$scratch/checkout"
	refused 'must be annotated' restore v1.2.3
)
# A tag targeting another commit is refused (the object is fetched with the
# job token).
printf 'changed\n' >>"$scratch/source/file"
git -C "$scratch/source" add file
git -C "$scratch/source" commit -qm 'another fixture commit'
git -C "$scratch/source" tag -fa -m 'Other fixture release' v1.2.3
git -C "$scratch/source" push -q --force origin main refs/tags/v1.2.3
(
	cd "$scratch/checkout"
	refused 'not the remote annotated tag at HEAD' restore v1.2.3
)
echo '[ok] tag restore: job-token API lookup (token on stdin, never argv), peeled-ref repair, and refusals for no token, absent, failed, malformed or misdirected answers, http, an API override under Actions, an origin that disagrees with the API or cannot be read, replaced, lightweight and other-commit tags'
