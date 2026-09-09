#!/usr/bin/env bash
# Shared release asset and repository invariants.

set -euo pipefail

STORAGEPRIMS_REPOSITORY="3leaps/storageprims"

release_repo_root() {
	git rev-parse --show-toplevel
}

release_version() {
	local root
	root="$(release_repo_root)"
	local version
	version="$(cat "$root/VERSION")"
	if [[ ! "$version" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]; then
		echo "error: VERSION must contain one stable semantic version" >&2
		return 1
	fi
	printf '%s\n' "$version"
}

release_tag() {
	local tag="${1:-${STORAGEPRIMS_RELEASE_TAG:-}}"
	if [[ -z "$tag" ]]; then
		echo "error: STORAGEPRIMS_RELEASE_TAG is required" >&2
		return 1
	fi
	if [[ ! "$tag" =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]; then
		echo "error: release tag must be canonical vX.Y.Z" >&2
		return 1
	fi
	if [[ "$tag" != "v$(release_version)" ]]; then
		echo "error: release tag does not match VERSION" >&2
		return 1
	fi
	printf '%s\n' "$tag"
}

require_release_guard() {
	local strict="${1:-0}"
	local root
	root="$(release_repo_root)"
	if [[ "$strict" == "1" ]]; then
		STORAGEPRIMS_REQUIRE_TAG=1 "$root/scripts/release-guard-tag-version.sh"
	else
		"$root/scripts/release-guard-tag-version.sh"
	fi
}

release_base_assets() {
	local version
	version="$(release_version)"
	printf '%s\n' \
		"LICENSE-APACHE" \
		"LICENSE-MIT" \
		"sbom-${version}.cdx.json" \
		"storageprims-ffi-${version}-darwin-arm64.tar.gz" \
		"storageprims-ffi-${version}-linux-amd64.tar.gz" \
		"storageprims-ffi-${version}-linux-arm64.tar.gz"
}

release_signable_assets() {
	local tag
	tag="$(release_tag)"
	release_base_assets
	printf 'release-notes-%s.md\n' "$tag"
}

release_provenance_assets() {
	printf '%s\n' \
		"SHA256SUMS" \
		"SHA256SUMS.minisig" \
		"SHA512SUMS" \
		"SHA512SUMS.minisig" \
		"storageprims-minisign.pub"
	if [[ -n "${STORAGEPRIMS_PGP_KEY_ID:-}" || -n "${STORAGEPRIMS_GPG_HOMEDIR:-}" ]]; then
		require_complete_pgp_config
		printf '%s\n' \
			"SHA256SUMS.asc" \
			"SHA512SUMS.asc" \
			"storageprims-release-signing-key.asc"
	fi
}

release_checksummed_assets() {
	release_signable_assets
	printf '%s\n' SHA256SUMS SHA512SUMS
}

release_signed_without_keys_assets() {
	release_checksummed_assets
	printf '%s\n' SHA256SUMS.minisig SHA512SUMS.minisig
	if [[ -n "${STORAGEPRIMS_PGP_KEY_ID:-}" || -n "${STORAGEPRIMS_GPG_HOMEDIR:-}" ]]; then
		require_complete_pgp_config
		printf '%s\n' SHA256SUMS.asc SHA512SUMS.asc
	fi
}

release_signed_assets() {
	release_signed_without_keys_assets
	printf '%s\n' storageprims-minisign.pub
	if [[ -n "${STORAGEPRIMS_PGP_KEY_ID:-}" || -n "${STORAGEPRIMS_GPG_HOMEDIR:-}" ]]; then
		require_complete_pgp_config
		printf '%s\n' storageprims-release-signing-key.asc
	fi
}

require_complete_pgp_config() {
	if [[ -z "${STORAGEPRIMS_PGP_KEY_ID:-}" || -z "${STORAGEPRIMS_GPG_HOMEDIR:-}" ]]; then
		echo "error: optional PGP signing requires both PGP variables" >&2
		return 1
	fi
}

assert_exact_directory_inventory() (
	local directory="$1"
	local producer="$2"
	if [[ ! -d "$directory" || -L "$directory" ]]; then
		echo "error: release directory is absent or unsafe" >&2
		return 1
	fi

	local expected_file actual_file
	expected_file="$(mktemp "${TMPDIR:-/tmp}/storageprims-expected.XXXXXX")"
	actual_file="$(mktemp "${TMPDIR:-/tmp}/storageprims-actual.XXXXXX")"
	trap 'rm -f "$expected_file" "$actual_file"' EXIT

	"$producer" | LC_ALL=C sort >"$expected_file"
	find "$directory" -mindepth 1 -maxdepth 1 -print |
		while IFS= read -r entry; do basename "$entry"; done |
		LC_ALL=C sort >"$actual_file"

	if ! cmp -s "$expected_file" "$actual_file"; then
		echo "error: release directory inventory mismatch" >&2
		diff -u "$expected_file" "$actual_file" >&2 || true
		return 1
	fi
	while IFS= read -r name; do
		if [[ ! -f "$directory/$name" || -L "$directory/$name" ]]; then
			echo "error: release asset is not a regular file: $name" >&2
			return 1
		fi
	done <"$expected_file"
)

validate_ffi_archive() (
	local archive="$1"
	local filename platform shared
	filename="$(basename "$archive")"
	case "$filename" in
	*-darwin-arm64.tar.gz)
		platform="darwin-arm64"
		shared="libstorageprims_ffi.dylib"
		;;
	*-linux-amd64.tar.gz)
		platform="linux-amd64"
		shared="libstorageprims_ffi.so"
		;;
	*-linux-arm64.tar.gz)
		platform="linux-arm64"
		shared="libstorageprims_ffi.so"
		;;
	*)
		echo "error: unexpected FFI archive name" >&2
		return 1
		;;
	esac

	local expected members listing
	expected="$(mktemp "${TMPDIR:-/tmp}/storageprims-archive-expected.XXXXXX")"
	members="$(mktemp "${TMPDIR:-/tmp}/storageprims-archive-members.XXXXXX")"
	listing="$(mktemp "${TMPDIR:-/tmp}/storageprims-archive-listing.XXXXXX")"
	trap 'rm -f "$expected" "$members" "$listing"' EXIT

	printf '%s\n' \
		"LICENSE-APACHE" \
		"LICENSE-MIT" \
		"libstorageprims_ffi.a" \
		"$shared" \
		"storageprims.h" |
		LC_ALL=C sort >"$expected"
	tar -tzf "$archive" >"$members"
	tar -tvzf "$archive" >"$listing"

	if grep -Eq '(^/|(^|/)\.\.(/|$)|/)' "$members"; then
		echo "error: unsafe or nested archive member in $platform asset" >&2
		return 1
	fi
	if awk 'substr($1, 1, 1) != "-" { exit 1 }' "$listing"; then
		:
	else
		echo "error: FFI archive contains a non-regular member" >&2
		return 1
	fi
	LC_ALL=C sort "$members" -o "$members"
	if ! cmp -s "$expected" "$members"; then
		echo "error: FFI archive member contract mismatch" >&2
		diff -u "$expected" "$members" >&2 || true
		return 1
	fi
)

validate_all_ffi_archives() {
	local directory="$1"
	local version
	version="$(release_version)"
	local platform
	for platform in darwin-arm64 linux-amd64 linux-arm64; do
		validate_ffi_archive \
			"$directory/storageprims-ffi-${version}-${platform}.tar.gz"
	done
}

assert_github_release_state() (
	local expected_assets_producer="$1"
	local tag root tag_commit
	tag="$(release_tag)"
	root="$(release_repo_root)"
	tag_commit="$(git -C "$root" rev-parse "refs/tags/${tag}^{}")"

	if [[ -n "${GH_REPO+x}" ]]; then
		echo "error: GH_REPO must be unset; repository authority is fixed" >&2
		return 1
	fi
	for command_name in gh jq; do
		command -v "$command_name" >/dev/null 2>&1 || {
			echo "error: required release command is unavailable" >&2
			return 1
		}
	done
	if [[ "$(gh repo view "$STORAGEPRIMS_REPOSITORY" \
		--json nameWithOwner --jq .nameWithOwner)" != "$STORAGEPRIMS_REPOSITORY" ]]; then
		echo "error: GitHub repository identity mismatch" >&2
		return 1
	fi

	local state expected actual
	state="$(mktemp "${TMPDIR:-/tmp}/storageprims-release-state.XXXXXX")"
	expected="$(mktemp "${TMPDIR:-/tmp}/storageprims-remote-expected.XXXXXX")"
	actual="$(mktemp "${TMPDIR:-/tmp}/storageprims-remote-actual.XXXXXX")"
	trap 'rm -f "$state" "$expected" "$actual"' EXIT
	gh release view "$tag" --repo "$STORAGEPRIMS_REPOSITORY" \
		--json tagName,targetCommitish,isDraft,assets >"$state"

	if [[ "$(jq -r .tagName "$state")" != "$tag" ||
	"$(jq -r .targetCommitish "$state")" != "$tag_commit" ||
	"$(jq -r .isDraft "$state")" != "true" ]]; then
		echo "error: GitHub release tag, target, or draft state mismatch" >&2
		return 1
	fi
	"$expected_assets_producer" | LC_ALL=C sort >"$expected"
	jq -r '.assets[].name' "$state" | LC_ALL=C sort >"$actual"
	if ! cmp -s "$expected" "$actual"; then
		echo "error: remote draft asset inventory mismatch" >&2
		diff -u "$expected" "$actual" >&2 || true
		return 1
	fi
)
