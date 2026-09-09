#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
fixture="$(mktemp -d "${TMPDIR:-/tmp}/storageprims-github-state.XXXXXX")"
fake_bin="$fixture/bin"
trap 'rm -rf "$fixture"' EXIT
mkdir -p "$fixture/scripts" "$fake_bin"
cp "$SCRIPT_DIR/release-common.sh" "$fixture/scripts/"
printf '0.1.0\n' >"$fixture/VERSION"
git init -q -b main "$fixture"
git -C "$fixture" config user.name "release state test"
git -C "$fixture" config user.email "noreply@example.invalid"
git -C "$fixture" add VERSION scripts/release-common.sh
git -C "$fixture" commit -qm "fixture"
git -C "$fixture" tag -a v0.1.0 -m "fixture"
git -C "$fixture" checkout -q --detach v0.1.0
fixture_commit="$(git -C "$fixture" rev-parse HEAD)"
original_path="$PATH"

cat >"$fake_bin/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
case "${FAKE_RELEASE_STATE:?}" in
  wrong-repo)
    if [[ "$1 $2" == "repo view" ]]; then
      printf 'other/storageprims\n'
      exit 0
    fi
    ;;
esac
if [[ "$1 $2" == "repo view" ]]; then
	printf '3leaps/storageprims\n'
	exit 0
fi
if [[ "$1 $2" == "release list" ]]; then
  printf '[{"tagName":"v0.1.0"}]\n'
  exit 0
fi
if [[ "$1 $2" != "release view" ]]; then
  exit 1
fi
draft=true
target="$FAKE_COMMIT"
extra=""
case "$FAKE_RELEASE_STATE" in
  valid) ;;
  published) draft=false ;;
  wrong-target) target=0000000000000000000000000000000000000000 ;;
  extra-asset) extra=',{"name":"foreign.txt"}' ;;
  *) exit 1 ;;
esac
printf '{"tagName":"v0.1.0","targetCommitish":"%s","isDraft":%s,"assets":[' \
  "$target" "$draft"
printf '%s' \
  '{"name":"LICENSE-APACHE"},' \
  '{"name":"LICENSE-MIT"},' \
  '{"name":"sbom-0.1.0.cdx.json"},' \
  '{"name":"storageprims-ffi-0.1.0-darwin-arm64.tar.gz"},' \
  '{"name":"storageprims-ffi-0.1.0-linux-amd64.tar.gz"},' \
  '{"name":"storageprims-ffi-0.1.0-linux-arm64.tar.gz"}'
printf '%s]}\n' "$extra"
EOF
chmod +x "$fake_bin/gh"

expect_fail() {
	if "$@" >/dev/null 2>&1; then
		echo "expected failure: $*" >&2
		exit 1
	fi
}

run_check() {
	local state="$1"
	(
		cd "$fixture"
		export PATH="$fake_bin:$original_path"
		export STORAGEPRIMS_RELEASE_TAG=v0.1.0
		export FAKE_COMMIT="$fixture_commit"
		export FAKE_RELEASE_STATE="$state"
		unset GH_REPO
		# shellcheck source=/dev/null
		source scripts/release-common.sh
		assert_github_release_state release_base_assets
	)
}

run_check valid
expect_fail run_check published
expect_fail run_check wrong-target
expect_fail run_check wrong-repo
expect_fail run_check extra-asset
expect_fail env GH_REPO=other/repository \
	PATH="$fake_bin:$original_path" \
	STORAGEPRIMS_RELEASE_TAG=v0.1.0 \
	FAKE_COMMIT="$fixture_commit" \
	FAKE_RELEASE_STATE=valid \
	bash -c "cd '$fixture'; source scripts/release-common.sh; assert_github_release_state release_base_assets"

echo "[ok] GitHub repository, target, draft, and inventory controls passed"
