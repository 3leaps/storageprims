#!/usr/bin/env bash

set -euo pipefail

root="$(git rev-parse --show-toplevel)"
cd "$root"
command -v rg >/dev/null || {
	echo 'error: ripgrep is required for release controls' >&2
	exit 1
}

fail_if_found() {
	local pattern="$1"
	shift
	if rg -n "$pattern" "$@" >/dev/null; then
		echo "error: forbidden release content detected" >&2
		rg -n "$pattern" "$@" >&2
		exit 1
	else
		status=$?
		if [[ "$status" != 1 ]]; then
			echo "error: release control scan failed ($status)" >&2
			exit "$status"
		fi
	fi
}

fail_if_found \
	'cargo[[:space:]]+(publish|login|yank|owner)|CARGO_REGISTRY_TOKEN|CARGO_REGISTRIES_[A-Z0-9_]+_TOKEN|secrets\.|STORAGEPRIMS_(MINISIGN_KEY|PGP_KEY_ID|GPG_HOMEDIR)' \
	"${1:-.github/workflows}"
# Workflows hold no write authority, keep no checkout credentials, and never
# create or change a GitHub release: the maintainer does that locally.
fail_if_found \
	'^[[:space:]]*[a-z-]+:[[:space:]]*write([[:space:]]|$)|permissions:[[:space:]]*write-all|persist-credentials:[[:space:]]*true|gh[[:space:]]+release[[:space:]]|repos/[^[:space:]]+/releases|action-gh-release' \
	"${1:-.github/workflows}"
fail_if_found \
	'WAITPRIMS_|SYSPRIMS_' \
	--glob '!release-negative-controls.test.sh' \
	Makefile scripts .github/workflows/release.yml
# Every action is a full commit SHA (local ./ actions excepted), the Node 20
# action SHAs are refused, and container images are digest-pinned.
if rg -n --pcre2 '^\s*(-\s+)?uses:\s*(?!\./)(?![^@\s]+@[0-9a-f]{40}(\s|$))\S' "${1:-.github/workflows}" >&2; then
	echo 'error: workflow action is not pinned to a full commit SHA' >&2
	exit 1
fi
fail_if_found \
	'11d5960a326750d5838078e36cf38b85af677262|ea165f8d65b6e75b540449e92b4886f43607fa02' \
	"${1:-.github/workflows}"
if rg -n --pcre2 '^\s*image:\s*(?!\S+@sha256:[0-9a-f]{64}\s*$)\S' "${1:-.github/workflows}" >&2; then
	echo 'error: workflow container image is not digest-pinned' >&2
	exit 1
fi
# A docker run command, with its backslash continuations, names a digest.
while IFS= read -r -d '' workflow; do
	awk '
		/docker[[:space:]]+run([[:space:]]|$)/ {cmd = ""; open = 1}
		open {cmd = cmd $0 "\n"; if ($0 !~ /\\[[:space:]]*$/) {open = 0; if (cmd !~ /@sha256:[0-9a-f]{64}/) bad = 1}}
		END {exit bad}
	' "$workflow" || {
		echo "error: docker run image is not digest-pinned in $workflow" >&2
		exit 1
	}
done < <(find "${1:-.github/workflows}" -type f \( -name '*.yml' -o -name '*.yaml' \) -print0)
"$root/scripts/check-public-source.sh"

if [[ "$#" == 0 ]]; then
	workflow=.github/workflows/release.yml
	awk '
		/^  verify-signature:/ {signature=1; seen=1; next}
		/^  [a-z-]+:/ && signature {signature=0}
		signature && /contents: write|secrets\./ {exit 1}
		END {if (!seen) exit 1}
	' "$workflow" || {
		echo 'error: signed-tag verification must be read-only' >&2
		exit 1
	}
fi

mutation_count="$(rg -U -o \
	'gh release (upload|edit)[^\n]*(\n[^\n]*){0,2}--repo "\$STORAGEPRIMS_REPOSITORY"' \
	scripts/upload-release-assets.sh | rg -o 'gh release' | wc -l | tr -d ' ')"
if [[ "$mutation_count" != "2" ]]; then
	echo "error: every GitHub mutation must pin the repository" >&2
	exit 1
fi
if rg -n --glob '!release-negative-controls.test.sh' \
	-- '--draft=false|draft:[[:space:]]*false' \
	scripts .github/workflows/release.yml >/dev/null; then
	echo "error: release tooling must not auto-publish" >&2
	exit 1
fi

if [[ "$#" == 0 ]]; then
	"$root/scripts/public-source-controls.test.sh"
	fixture="$(mktemp -d "${TMPDIR:-/tmp}/storageprims-workflow-control.XXXXXX")"
	trap 'rm -rf "$fixture"' EXIT
	for forbidden in 'cargo publish' 'cargo login' 'cargo yank' 'cargo owner' \
		'CARGO_REGISTRY_TOKEN' 'CARGO_REGISTRIES_CRATES_IO_TOKEN' 'secrets.REGISTRY' \
		'  contents: write' '      id-token: write' 'permissions: write-all' \
		'          persist-credentials: true' 'run: gh release create v1.2.3' \
		'run: gh api --method POST repos/o/r/releases' \
		'uses: softprops/action-gh-release@0000000000000000000000000000000000000000' \
		'      - uses: actions/checkout@v4' '        uses: dtolnay/rust-toolchain@stable' \
		'        uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262' \
		'        uses: actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02' \
		'      image: ghcr.io/fulmenhq/goneat-tools-runner-glibc:v0.5.6' \
		'          docker run --rm anchore/syft:latest dir:.'; do
		printf '%s\n' "$forbidden" >"$fixture/fixture.yml"
		if bash "$0" "$fixture" >/dev/null 2>&1; then
			echo "error: workflow negative control missed $forbidden" >&2
			exit 1
		fi
	done
fi

echo "[ok] release negative controls passed"
