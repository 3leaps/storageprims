#!/usr/bin/env bash
# Synthetic local registry fixtures; no operator credentials.
# shellcheck disable=SC2034 # consumed by sourcing tests
fixture_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
fixture_dir="$(cd "$(mktemp -d)" && pwd -P)"
fixture_cleanup() { rm -rf "$fixture_dir"; }
trap fixture_cleanup EXIT
expect_fail() {
	if "$@" >"$fixture_dir/last.out" 2>&1; then
		echo "error: expected rejection: $*" >&2
		cat "$fixture_dir/last.out" >&2
		exit 1
	fi
}

expect_fail_with() {
	local pattern="$1"
	shift
	expect_fail "$@"
	grep -Eq -- "$pattern" "$fixture_dir/last.out" || {
		echo "error: rejection for '$*' did not mention: $pattern" >&2
		cat "$fixture_dir/last.out" >&2
		exit 1
	}
}

fixture_repo() {
	local repo="$1" version="$2"
	mkdir -p "$repo/scripts" "$repo/config/release"
	printf '%s\n' "$version" >"$repo/VERSION"
	git -C "$repo" init -q -b main
	git -C "$repo" config user.name fixture
	git -C "$repo" config user.email fixture@example.invalid
	git -C "$repo" config commit.gpgsign false
	git -C "$repo" config tag.gpgsign false
	git -C "$repo" remote add origin https://github.com/3leaps/storageprims.git
}
fixture_commit() {
	git -C "$1" add -A
	git -C "$1" commit -qm "${2:-fixture}"
}
