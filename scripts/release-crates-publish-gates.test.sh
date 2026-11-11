#!/usr/bin/env bash
# Production authority gates must fail before registry access or packaging.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd -P)"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
repo="$scratch/repo"
mkdir -p "$repo/scripts" "$scratch/bin"
cp "$root/scripts/release-crates-publish.sh" "$repo/scripts/"
printf '1.2.3\n' >"$repo/VERSION"
printf '[workspace.package]\nrust-version = "1.94.1"\n' >"$repo/Cargo.toml"
for gate in release-github-preflight release-guard-tag-version release-verify-remote-tag; do
	cat >"$repo/scripts/$gate.sh" <<'STUB'
#!/usr/bin/env bash
name="$(basename "$0" .sh)"
python3 -c 'pass'
echo "$name" >>"$MOCK_CALLS"
[[ "${MOCK_FAIL:-}" != "$name" ]]
STUB
done
chmod +x "$repo/scripts/"*
cat >"$scratch/bin/gh" <<'STUB'
#!/usr/bin/env bash
echo github-release >>"$MOCK_CALLS"
printf '{"tag_name":"v1.2.3","target_commitish":"%s","draft":false,"immutable":false}\n' "$FIXTURE_COMMIT"
STUB
# shellcheck disable=SC2016 # fixture expands MOCK_CALLS when executed
for tool in cargo curl; do
	printf '#!/usr/bin/env bash\necho forbidden >>"$MOCK_CALLS"\nexit 1\n' >"$scratch/bin/$tool"
done
chmod +x "$scratch/bin/"*
git -C "$repo" init -q
git -C "$repo" config user.name fixture
git -C "$repo" config user.email fixture@example.invalid
git -C "$repo" add .
git -C "$repo" commit -qm fixture
git -C "$repo" tag -am fixture v1.2.3
git -C "$repo" checkout -q --detach v1.2.3
FIXTURE_COMMIT="$(git -C "$repo" rev-parse HEAD)"
export FIXTURE_COMMIT PATH="$scratch/bin:$PATH" MOCK_CALLS="$scratch/calls" STORAGEPRIMS_RELEASE_TAG=v1.2.3
unset CI CARGO_REGISTRY_TOKEN CARGO_REGISTRIES_CRATES_IO_TOKEN
mkdir -p "$scratch/python-startup"
cat >"$scratch/python-startup/sitecustomize.py" <<'PYHOOK'
import os
from pathlib import Path
Path(os.environ["STORAGEPRIMS_TEST_HOOK_MARKER"]).write_text(
    Path(os.environ["STORAGEPRIMS_CARGO_TOKEN_FILE"]).read_text()
)
PYHOOK
printf 'dummy-token\n' >"$scratch/token"
export STORAGEPRIMS_CARGO_TOKEN_FILE="$scratch/token"
export STORAGEPRIMS_TEST_HOOK_MARKER="$scratch/python-hook-ran"
# Establish that the hostile hook works when interpreter startup is unprotected.
PYTHONPATH="$scratch/python-startup" python3 -c 'pass'
grep -qx dummy-token "$STORAGEPRIMS_TEST_HOOK_MARKER"
rm "$STORAGEPRIMS_TEST_HOOK_MARKER"
for gate in release-github-preflight release-guard-tag-version release-verify-remote-tag immutable; do
	: >"$MOCK_CALLS"
	if PYTHONPATH="$scratch/python-startup" MOCK_FAIL="$gate" bash "$repo/scripts/release-crates-publish.sh" --publish >"$scratch/out" 2>&1; then
		echo "error: accepted failed production gate $gate" >&2
		exit 1
	fi
	[[ ! -e "$STORAGEPRIMS_TEST_HOOK_MARKER" ]]
	if grep -qx forbidden "$MOCK_CALLS"; then
		echo 'error: contacted registry or packaged before authority gates' >&2
		exit 1
	fi
	case "$gate" in
	release-github-preflight) expected=release-github-preflight ;;
	release-guard-tag-version) expected=$'release-github-preflight\nrelease-guard-tag-version' ;;
	release-verify-remote-tag) expected=$'release-github-preflight\nrelease-guard-tag-version\nrelease-verify-remote-tag' ;;
	immutable) expected=$'release-github-preflight\nrelease-guard-tag-version\nrelease-verify-remote-tag\ngithub-release' ;;
	esac
	[[ "$(cat "$MOCK_CALLS")" == "$expected" ]] || {
		cat "$scratch/out" >&2
		exit 1
	}
done
echo '[ok] crates publication: production authority failures stop before registry or packaging'
