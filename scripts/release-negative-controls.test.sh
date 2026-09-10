#!/usr/bin/env bash

set -euo pipefail

root="$(git rev-parse --show-toplevel)"
cd "$root"

fail_if_found() {
	local pattern="$1"
	shift
	if rg -n "$pattern" "$@" >/dev/null; then
		echo "error: forbidden release content detected" >&2
		rg -n "$pattern" "$@" >&2
		exit 1
	fi
}

fail_if_found \
	'cargo[[:space:]]+publish|CARGO_REGISTRY_TOKEN|STORAGEPRIMS_(MINISIGN_KEY|PGP_KEY_ID|GPG_HOMEDIR)' \
	.github/workflows/release.yml
fail_if_found \
	'WAITPRIMS_|SYSPRIMS_' \
	--glob '!release-negative-controls.test.sh' \
	Makefile scripts .github/workflows/release.yml
fail_if_found \
	'STGP-|brief-stgp|release-storageprims-v010|org-3leaps' \
	README.md CHANGELOG.md RELEASE_NOTES.md RELEASE_CHECKLIST.md docs/releases

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

echo "[ok] release negative controls passed"
