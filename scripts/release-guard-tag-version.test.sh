#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
fixture="$(mktemp -d "${TMPDIR:-/tmp}/storageprims-release-guard.XXXXXX")"
remote="$(mktemp -d "${TMPDIR:-/tmp}/storageprims-release-remote.XXXXXX")"
trap 'rm -rf "$fixture" "$remote"' EXIT

cp "$SCRIPT_DIR/release-guard-tag-version.sh" "$fixture/guard.sh"
printf '1.2.3\n' >"$fixture/VERSION"
git init -q -b main "$fixture"
git init -q --bare "$remote"
git -C "$fixture" config user.name "release guard test"
git -C "$fixture" config user.email "noreply@example.invalid"
git -C "$fixture" add VERSION guard.sh
git -C "$fixture" commit -qm "fixture"
git -C "$fixture" tag -a v1.2.3 -m "fixture"
git -C "$fixture" remote add origin https://github.com/3leaps/storageprims.git
git -C "$fixture" config "url.$remote.insteadOf" \
	https://github.com/3leaps/storageprims.git
git -C "$fixture" push -q origin main refs/tags/v1.2.3

run_guard() {
	(
		cd "$fixture"
		env -u STORAGEPRIMS_RELEASE_TAG \
			-u STORAGEPRIMS_REQUIRE_TAG \
			-u STORAGEPRIMS_RELEASE_KEY \
			-u RELEASE_TAG \
			"$@"
	)
}

expect_fail() {
	if "$@" >/dev/null 2>&1; then
		echo "expected failure: $*" >&2
		exit 1
	fi
}

run_guard ./guard.sh >/dev/null
run_guard STORAGEPRIMS_RELEASE_TAG=v1.2.3 ./guard.sh >/dev/null
for invalid in 1.2.3 vv1.2.3 "v1.2.3 " v1.2.3-rc.1 v1.2.3+build; do
	expect_fail run_guard STORAGEPRIMS_RELEASE_TAG="$invalid" ./guard.sh
done
expect_fail run_guard STORAGEPRIMS_RELEASE_TAG=v1.2.4 ./guard.sh
expect_fail run_guard RELEASE_TAG=v1.2.3 ./guard.sh
expect_fail run_guard STORAGEPRIMS_RELEASE_KEY=v1.2.3 ./guard.sh
expect_fail run_guard STORAGEPRIMS_RELEASE_TAG=v1.2.3 \
	STORAGEPRIMS_REQUIRE_TAG=1 ./guard.sh

git -C "$fixture" checkout -q --detach v1.2.3
run_guard STORAGEPRIMS_RELEASE_TAG=v1.2.3 \
	STORAGEPRIMS_REQUIRE_TAG=1 ./guard.sh >/dev/null
original_tag="$(git -C "$fixture" rev-parse refs/tags/v1.2.3)"
git -C "$fixture" tag -d v1.2.3 >/dev/null
expect_fail run_guard STORAGEPRIMS_RELEASE_TAG=v1.2.3 \
	STORAGEPRIMS_REQUIRE_TAG=1 ./guard.sh
git -C "$fixture" update-ref refs/tags/v1.2.3 "$original_tag"
git -C "$fixture" tag -fa v1.2.3 -m 'different object' HEAD
git -C "$fixture" push -q --force origin refs/tags/v1.2.3
git -C "$fixture" update-ref refs/tags/v1.2.3 "$original_tag"
expect_fail run_guard STORAGEPRIMS_RELEASE_TAG=v1.2.3 \
	STORAGEPRIMS_REQUIRE_TAG=1 ./guard.sh

git -C "$fixture" checkout -q main
printf 'next\n' >"$fixture/next"
git -C "$fixture" add next
git -C "$fixture" commit -qm "move main"
git -C "$fixture" push -q origin main
git -C "$fixture" checkout -q --detach v1.2.3
expect_fail run_guard STORAGEPRIMS_RELEASE_TAG=v1.2.3 \
	STORAGEPRIMS_REQUIRE_TAG=1 ./guard.sh

echo "[ok] release tag guard tests passed"
