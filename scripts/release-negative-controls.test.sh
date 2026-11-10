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
# Workflow write authority, credentials, pins, images and release commands are
# checked on the parsed YAML (see release-workflow-policy.test.sh).
if [[ "$#" == 0 ]]; then
	workflows=()
	while IFS= read -r -d '' file; do workflows+=("$file"); done < <(find .github/workflows -type f \( -name '*.yml' -o -name '*.yaml' \) -print0)
	python3 -I "$root/scripts/release-workflow-policy.py" "${workflows[@]}"
fi
fail_if_found \
	'WAITPRIMS_|SYSPRIMS_' \
	--glob '!release-negative-controls.test.sh' \
	Makefile scripts .github/workflows/release.yml
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

# Signed-set mutations address the verified release id, never the tag.
if rg -n 'gh[[:space:]]+release[[:space:]]+(upload|edit|delete)' scripts/upload-release-assets.sh >&2; then
	echo "error: signed-set mutations must address the verified release id" >&2
	exit 1
fi
[[ "$(rg -c 'releases/\$id' scripts/upload-release-assets.sh)" == 2 ]] || {
	echo "error: signed-set mutations must address the verified release id" >&2
	exit 1
}
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
		'CARGO_REGISTRY_TOKEN' 'CARGO_REGISTRIES_CRATES_IO_TOKEN' 'secrets.REGISTRY'; do
		printf '%s\n' "$forbidden" >"$fixture/fixture.yml"
		if bash "$0" "$fixture" >/dev/null 2>&1; then
			echo "error: workflow negative control missed $forbidden" >&2
			exit 1
		fi
	done
fi

echo "[ok] release negative controls passed"
