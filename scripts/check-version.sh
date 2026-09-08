#!/usr/bin/env bash
# Validate the storageprims VERSION, workspace, members, and path dependencies.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
VERSION_FILE="$PROJECT_ROOT/VERSION"
CARGO_TOML="$PROJECT_ROOT/Cargo.toml"

error() {
	printf '[ERROR] %s\n' "$*" >&2
}

ok() {
	printf '[OK] %s\n' "$*"
}

if [[ ! -f "$VERSION_FILE" ]]; then
	error "VERSION file not found: $VERSION_FILE"
	exit 1
fi

version=$(tr -d '[:space:]' <"$VERSION_FILE")
if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9.-]+)?(\+[a-zA-Z0-9.-]+)?$ ]]; then
	error "VERSION contains invalid semver: $version"
	exit 1
fi
ok "VERSION file: $version"

workspace_version=$(
	awk '
		/^\[workspace\.package\]$/ { in_package = 1; next }
		/^\[/ { in_package = 0 }
		in_package && /^version[[:space:]]*=/ {
			gsub(/^[^"]*"|"[[:space:]]*$/, "")
			print
			exit
		}
	' "$CARGO_TOML"
)
if [[ "$workspace_version" != "$version" ]]; then
	error "workspace version $workspace_version does not match VERSION $version"
	exit 1
fi
ok "workspace version matches"

failed=0
while IFS= read -r manifest; do
	crate_dir=$(dirname "$manifest")
	if grep -A 12 '^\[package\]' "$manifest" | grep -Eq '^version\.workspace[[:space:]]*=[[:space:]]*true'; then
		ok "${crate_dir#"$PROJECT_ROOT/"} uses workspace version"
	else
		error "${crate_dir#"$PROJECT_ROOT/"} must set version.workspace = true"
		failed=1
	fi
done < <(
	find "$PROJECT_ROOT/crates" "$PROJECT_ROOT/ffi" -mindepth 2 -maxdepth 2 \
		-name Cargo.toml -type f | sort
)

while IFS= read -r line; do
	dependency_version=$(sed -E 's/.*version = "([^"]*)".*/\1/' <<<"$line")
	if [[ "$dependency_version" != "$version" ]]; then
		error "workspace dependency version $dependency_version does not match VERSION $version"
		error "$line"
		failed=1
	fi
done < <(
	grep -E '^storageprims-(core|ops|s3|ffi) = \{ version = "' "$CARGO_TOML" || true
)

if [[ "$failed" -ne 0 ]]; then
	error "version consistency check failed"
	exit 1
fi

ok "workspace path-dependency versions match"
ok "version consistency check passed"
