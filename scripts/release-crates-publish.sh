#!/usr/bin/env bash
# Package or publish the library crates in registry dependency order.
#   (no flag)  per-crate `cargo publish --dry-run`; nothing is uploaded
#   --publish  one maintainer-cued zero-compile upload per crate, in order,
#              gated by scripts/release-cargo-provider.py
# Both run only from a clean detached checkout of the annotated release tag.
# --publish also requires every earlier crate to be on the crates.io index at
# this version before it uploads the next. CI never holds a registry token.
# Test-only: --registry-index URL --registry-api URL --registry-dl URL select a
# local test registry; a release run never passes them.
set -euo pipefail
# Python-based authority gates also run before packaging. Remove interpreter
# startup/module overrides before entering any helper or its subprocesses.
for name in $(compgen -e); do
	[[ "$name" != PYTHON* ]] || unset "$name"
done
root="$(cd "$(dirname "$0")/.." && pwd -P)"
cd "$root"
die() {
	echo "error: $*" >&2
	exit 1
}
publish=0
index_url="https://github.com/rust-lang/crates.io-index"
crates_io_index="$index_url"
download="https://static.crates.io/crates"
test_api=""
test_options=0
while [[ $# -gt 0 ]]; do
	case "$1" in
	--publish) publish=1 ;;
	--registry-index)
		index_url="${2:?}"
		test_options=$((test_options + 1))
		shift
		;;
	--registry-api)
		test_api="${2:?}"
		test_options=$((test_options + 1))
		shift
		;;
	--registry-dl)
		download="${2:?}"
		test_options=$((test_options + 1))
		shift
		;;
	*) die 'usage: release-crates-publish.sh [--publish]' ;;
	esac
	shift
done
# No inherited git variable (GIT_DIR, GIT_WORK_TREE, GIT_INDEX_FILE,
# GIT_CONFIG_*, ...) may steer any git or cargo command below.
for name in $(compgen -e); do
	[[ "$name" != GIT_* ]] || unset "$name"
done
[[ "$publish" == 0 || -z "${CI:-}" ]] || die 'crates.io publication is a local maintainer step, never CI'
if [[ "$publish" == 1 ]]; then
	[[ -z "${CARGO_REGISTRY_TOKEN+x}${CARGO_REGISTRIES_CRATES_IO_TOKEN+x}" ]] ||
		die 'unset CARGO_REGISTRY_TOKEN: the token reaches Cargo only through the release provider'
fi

# Commands resolve only from absolute directories outside the trusted checkout:
# an empty, dot or relative PATH entry would resolve against the tagged export
# once the steps below change into it.
trusted_path=
IFS=: read -r -a path_entries <<<"$PATH"
for entry in ${path_entries[@]+"${path_entries[@]}"}; do
	[[ "$entry" == /* && -d "$entry" ]] || continue
	resolved="$(cd "$entry" && pwd -P)"
	case "$resolved" in "$root" | "$root"/*) continue ;; esac
	trusted_path="${trusted_path:+$trusted_path:}$resolved"
done
[[ -n "$trusted_path" ]] || die 'no absolute PATH entry'
PATH="$trusted_path"
export PATH
for tool in cargo python3 curl git shasum; do
	command -v "$tool" >/dev/null || die "$tool not found on the trusted PATH"
done
python_bin="$(command -v python3)"
[[ "$python_bin" == /* ]] || die 'python3 must resolve to an absolute path'

# Test transport is exclusively loopback HTTP and cannot alias crates.io.
if [[ "$test_options" != 0 ]]; then
	[[ "$test_options" == 3 ]] || die 'test registry requires index, API and download endpoints'
	"$python_bin" -I - "$index_url" "$test_api" "$download" <<'PYURL'
import sys
from urllib.parse import urlsplit
for raw in sys.argv[1:]:
    url = urlsplit(raw.removeprefix("sparse+"))
    if (url.scheme != "http" or url.hostname not in ("127.0.0.1", "::1")
            or url.username is not None or url.password is not None
            or url.query or url.fragment or not url.port):
        raise SystemExit("error: test registry endpoints must be loopback HTTP with explicit ports")
PYURL
fi

: "${STORAGEPRIMS_RELEASE_TAG:?STORAGEPRIMS_RELEASE_TAG=vX.Y.Z is required}"
[[ "$STORAGEPRIMS_RELEASE_TAG" =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] || die 'STORAGEPRIMS_RELEASE_TAG must be a canonical vX.Y.Z tag'
! git symbolic-ref --quiet HEAD >/dev/null || die 'HEAD must be detached at the release tag'
object="$(git rev-parse --verify --quiet "refs/tags/$STORAGEPRIMS_RELEASE_TAG")" || die "$STORAGEPRIMS_RELEASE_TAG is absent"
[[ "$(git cat-file -t "$object")" == tag ]] || die "$STORAGEPRIMS_RELEASE_TAG must be an annotated tag"
[[ "$(git rev-parse "${object}^{commit}")" == "$(git rev-parse HEAD)" ]] || die 'HEAD is not the release tag commit'
[[ -z "$(git status --porcelain --untracked-files=normal)" ]] || die 'clean checkout required'
# git status does not show skip-worktree or assume-unchanged files. Refuse any
# index entry that is not an ordinary cached file (defence in depth: packaging
# below never reads this checkout's files).
flagged="$(git ls-files -v | awk '$1 != "H" {print}' | head -5)"
[[ -z "$flagged" ]] || die "index entries hide working-tree changes (skip-worktree or assume-unchanged): $flagged"
commit="$(git rev-parse "${object}^{commit}")"
# VERSION at the tagged commit decides the version published.
version="$(git cat-file blob "$commit:VERSION" | tr -d '\r\n')"
[[ "$STORAGEPRIMS_RELEASE_TAG" == "v$version" ]] || die 'STORAGEPRIMS_RELEASE_TAG does not match VERSION at the tagged commit'
msrv="$(git cat-file blob "$commit:Cargo.toml" 2>/dev/null | sed -n 's/^rust-version = "\(.*\)"/\1/p' | head -n 1 || true)"
[[ "$msrv" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die 'tagged workspace rust-version must pin a toolchain'
export RUSTUP_TOOLCHAIN="$msrv"

if [[ "$publish" == 1 && "$index_url" == "$crates_io_index" ]]; then
	"$root/scripts/release-github-preflight.sh"
	STORAGEPRIMS_REQUIRE_TAG=1 "$root/scripts/release-guard-tag-version.sh" >/dev/null
	"$root/scripts/release-verify-remote-tag.sh"
	gh api "repos/3leaps/storageprims/releases/tags/$STORAGEPRIMS_RELEASE_TAG" |
		"$python_bin" -I -c 'import json,sys; r=json.load(sys.stdin); assert r["tag_name"]==sys.argv[1] and r["target_commitish"]==sys.argv[2] and r["draft"] is False and r["immutable"] is True' "$STORAGEPRIMS_RELEASE_TAG" "$commit" ||
		die 'publish the immutable GitHub release before publishing crates'
fi

# Everything that reads crate sources (plan check, dry runs, packaging,
# publication) runs in an export of the tagged commit's blobs: no checkout
# semantics (autocrlf, eol, filters, sparse-checkout, export attributes) apply,
# and every exported file is checked against its blob before cargo runs.
# cargo's target directory is a fresh directory, so the .crate hashed is the
# one just written.
scratch="$(cd "$(mktemp -d "${TMPDIR:-/tmp}/storageprims-publish.XXXXXX")" && pwd -P)"
tree="$scratch/tree"
trap 'rm -rf "$scratch"' EXIT
unset CARGO_BUILD_TARGET_DIR CARGO_TARGET_DIR
if [[ -f "$root/scripts/check-registry-config.py" ]]; then
	CARGO_HOME="${CARGO_HOME:-$scratch/op-cargo-home}" "$python_bin" -I "$root/scripts/check-registry-config.py" >/dev/null
fi
"$python_bin" -I "$root/scripts/release-export-tree.py" "$root" "$commit" "$tree" ||
	die 'cannot export the tagged commit with its exact bytes'
if [[ -f "$root/scripts/check-registry-config.py" ]]; then
	CARGO_HOME="${CARGO_HOME:-$scratch/op-cargo-home}" "$python_bin" -I "$root/scripts/check-registry-config.py" --source-root "$tree" >/dev/null
fi
if [[ -f "$root/scripts/release-source-check.py" ]]; then
	"$python_bin" -I "$root/scripts/release-source-check.py" "$commit" --repo "$root" --worktree "$tree" >/dev/null
fi
export CARGO_TARGET_DIR="$scratch/target"
# cargo finds the tagged commit only through the export's own gitfile, which
# must name the git directory the exporter created in this run's scratch.
cd "$tree"
[[ -f .git && ! -L .git ]] || die 'the export has no gitfile of its own'
gitdir="$(sed -n '1s/^gitdir: //p' .git)"
[[ "$(wc -l <.git)" -eq 1 && "$gitdir" == "$scratch"/tree.gitdir.* && -d "$gitdir" && ! -L "$gitdir" ]] ||
	die 'the export gitfile does not name this run'"'"'s git directory'
[[ "$(git rev-parse --absolute-git-dir)" == "$gitdir" && "$(git rev-parse HEAD)" == "$commit" ]] ||
	die 'the export does not resolve to the tagged commit in its own git directory'
"$python_bin" -I "$tree/scripts/release-crates.py" check

registry_args=()
index_args=()
curl_proto=(--proto '=https' --tlsv1.2)
user_agent='storageprims-release-verify/1.0 (+https://github.com/3leaps/storageprims)'
if [[ "$index_url" != "$crates_io_index" ]]; then
	registry_args=(--registry testreg)
	index_args=(--allow-http)
	registry_config() { printf '[registries.testreg]\nindex = "%s"\n' "$index_url"; }
	sparse_index="$index_url"
	curl_proto=(--proto '=http,https')
else
	registry_config() { :; }
	sparse_index="https://index.crates.io/"
fi

# Publication state on the Cargo sparse index (or configured test registry).
# Prints "absent", or the published checksum; refuses if yanked or unreadable.
registry_checksum() {
	local crate="$1" out rc=0
	out="$("$python_bin" -I "$root/scripts/release-index-entry.py" ${index_args[@]+"${index_args[@]}"} "$sparse_index" "$crate" "$version")" || rc=$?
	case "$rc" in
	0)
		[[ "$out" =~ ^[0-9a-f]{64}$ ]] || die "malformed sparse-index checksum for $crate@$version"
		printf '%s\n' "$out"
		;;
	1) echo absent ;;
	3) die "$crate@$version is yanked on crates.io; stop and investigate" ;;
	*) die "cannot query sparse index for $crate@$version" ;;
	esac
}

# sha256 of the crate packaged from the tagged commit (zero-compile: --no-verify).
# The .crate is removed first so the hash is always of the file this call produced.
local_checksum() {
	local crate="$1" file="$CARGO_TARGET_DIR/package/${1}-${version}.crate" path pkg_home
	rm -f "$file"
	pkg_home="$(mktemp -d "$scratch/pkg-home.XXXXXX")"
	registry_config >"$pkg_home/config.toml"
	# No --allow-dirty: cargo refuses if any file differs from the tagged commit.
	(env -i PATH="$PATH" HOME="$pkg_home" CARGO_HOME="${CARGO_HOME:-$pkg_home}" \
		CARGO_TARGET_DIR="$CARGO_TARGET_DIR" \
		RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" \
		${msrv:+RUSTUP_TOOLCHAIN="$msrv"} \
		${FAKE_CRATE:+FAKE_CRATE="$FAKE_CRATE"} \
		${FAKE_VCS_SHA:+FAKE_VCS_SHA="$FAKE_VCS_SHA"} \
		${FAKE_VCS_PATH:+FAKE_VCS_PATH="$FAKE_VCS_PATH"} \
		${CARGO_CALLS:+CARGO_CALLS="$CARGO_CALLS"} \
		cargo package --locked --no-verify -p "$crate" >/dev/null 2>&1) || die "cannot package $crate"
	[[ -f "$file" ]] || die "packaged crate $file not found"
	path="$("$python_bin" -I "$tree/scripts/release-crates.py" paths | awk -v c="$crate" '$1==c {print $2}')"
	"$python_bin" -I "$tree/scripts/release-crates.py" vcs-info "$file" "$crate" "$version" "$commit" "$path" ||
		die "$crate does not record the tagged commit and its path"
	"$python_bin" -I -c 'import hashlib,sys; print(hashlib.sha256(open(sys.argv[1],"rb").read()).hexdigest())' "$file"
}

# Collect the ordered plan whole and check the producer before iterating.
plan_output="$("$python_bin" -I "$tree/scripts/release-crates.py" paths)" || die 'cannot read the crate publication plan'
crates=()
paths=()
while read -r crate path; do
	[[ -n "$crate" && -n "$path" ]] || die 'malformed crate publication plan'
	crates+=("$crate")
	paths+=("$path")
done <<<"$plan_output"
[[ "${#crates[@]}" -ge 1 ]] || die 'empty crate publication plan'

patches=()
for i in "${!crates[@]}"; do
	crate="${crates[$i]}"
	path="${paths[$i]}"
	if [[ "$publish" == 1 ]]; then
		published="$(registry_checksum "$crate")"
		expected_sum="$(local_checksum "$crate")"
		if [[ "$published" != absent ]]; then
			[[ "$published" == "$expected_sum" ]] ||
				die "$crate@$version is on crates.io with different contents; stop and investigate"
			echo "[--] $crate@$version is already on crates.io with these contents; skipping"
			continue
		fi
		token_file="${STORAGEPRIMS_CARGO_TOKEN_FILE:-}"
		[[ "$token_file" == /* && -f "$token_file" && -s "$token_file" && ! -L "$token_file" ]] ||
			die 'STORAGEPRIMS_CARGO_TOKEN_FILE must name a file holding the registry token'
		if [[ "$index_url" != "$crates_io_index" ]]; then
			[[ "$(cat "$token_file")" == dummy-token ]] ||
				die 'test registry requires the synthetic dummy-token credential'
		fi
		printf 'Publish %s@%s (sha256 %s) to crates.io? Type the crate name to continue: ' "$crate" "$version" "$expected_sum"
		read -r answer </dev/tty || die "publication of $crate not confirmed"
		[[ "$answer" == "$crate" ]] || die "publication of $crate not confirmed"
		upload_home="$(mktemp -d "$scratch/cargo-home.XXXXXX")"
		printf '#!/bin/bash\nexport STORAGEPRIMS_CARGO_APPROVAL=%q STORAGEPRIMS_CARGO_TOKEN_FILE=%q STORAGEPRIMS_CARGO_PROVIDER_LOG=%q\nexec %q -I %q "$@"\n' \
			"$upload_home/approval.json" "$token_file" "${STORAGEPRIMS_CARGO_PROVIDER_LOG:-$upload_home/provider.log}" \
			"$python_bin" "$root/scripts/release-cargo-provider.py" >"$upload_home/provider"
		chmod 0700 "$upload_home/provider"
		"$python_bin" -I - "$upload_home/approval.json" "$index_url" "$crate" "$version" "$expected_sum" <<'PY'
import json, sys
with open(sys.argv[1], "w", encoding="utf-8") as handle:
    json.dump({"index-url": sys.argv[2], "name": sys.argv[3], "vers": sys.argv[4], "cksum": sys.argv[5]}, handle)
PY
		{
			registry_config
			printf '[registry]\nglobal-credential-providers = ["%s"]\n' "$upload_home/provider"
		} >"$upload_home/config.toml"
		if [[ -f "$root/scripts/check-registry-config.py" ]]; then
			env -u CARGO_TARGET_DIR CARGO_HOME="$upload_home/clean-check" "$python_bin" -I "$root/scripts/check-registry-config.py" --source-root "$tree" >/dev/null
		fi
		(env -i PATH="$PATH" HOME="$upload_home" CARGO_HOME="$upload_home" \
			CARGO_TARGET_DIR="$scratch/upload-target" \
			RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" \
			${msrv:+RUSTUP_TOOLCHAIN="$msrv"} \
			CARGO_REGISTRY_CREDENTIAL_PROVIDER="$upload_home/provider" \
			CARGO_REGISTRIES_TESTREG_CREDENTIAL_PROVIDER="$upload_home/provider" \
			cargo publish --no-verify --locked ${registry_args[@]+"${registry_args[@]}"} -p "$crate")
		[[ -z "$(git status --porcelain --untracked-files=all)" ]] || die 'packaging changed the release source'
		echo "[..] waiting for $crate@$version on the crates.io index"
		for _ in $(seq 1 "${STORAGEPRIMS_INDEX_WAIT_TRIES:-60}"); do
			published="$(registry_checksum "$crate")"
			[[ "$published" != absent ]] && break
			sleep "${STORAGEPRIMS_INDEX_WAIT_SECONDS:-10}"
		done
		[[ "$published" != absent ]] ||
			die "$crate@$version did not appear on the crates.io index; stop and investigate"
		[[ "$published" == "$expected_sum" ]] ||
			die "the index checksum for $crate differs from the verified archive"
		curl --disable "${curl_proto[@]}" --fail --silent --show-error --location --retry 3 \
			-A "$user_agent" \
			--output "$scratch/published-$crate.crate" \
			"$download/$crate/$crate-$version.crate" || die "could not download the published $crate archive"
		[[ "$(shasum -a 256 "$scratch/published-$crate.crate" | cut -c1-64)" == "$expected_sum" ]] ||
			die "the published $crate archive differs from the verified one"
		echo "[ok] $crate $version published and verified ($expected_sum)"
	else
		printf 'Dry-run crates.io: %s@%s\n' "$crate" "$version"
		cargo publish --dry-run --locked -p "$crate" ${patches[@]+"${patches[@]}"}
		patches+=(--config "patch.crates-io.${crate}.path=\"${path}\"")
	fi
done
if [[ "$publish" == 1 && "$index_url" == "$crates_io_index" ]]; then
	(cd "$root" && "$root/scripts/release-crates-verify.sh")
fi
if [[ "$publish" == 1 ]]; then
	echo '[ok] every listed crate is on crates.io at this version'
else
	echo '[ok] per-crate registry dry runs passed; nothing was published'
fi
