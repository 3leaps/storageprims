#!/usr/bin/env bash
# Exercise the operator boundary without a real signing key or remote.
# shellcheck disable=SC2016
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd -P)"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
mkdir -p "$scratch/repo/scripts" "$scratch/bin" "$scratch/external"
cp "$root/scripts/release-tag-operator.sh" "$scratch/repo/scripts/"
printf '1.2.3\n' >"$scratch/repo/VERSION"
cat >"$scratch/repo/scripts/release-tag-common.sh" <<'SH'
tag_version() { printf 'version\n' >>"$TRACE"; [[ "$STORAGEPRIMS_RELEASE_TAG" == v1.2.3 && "$(pwd -P)" == "$FIXTURE_ROOT" ]]; }
tag_identity() { printf 'identity\n' >>"$TRACE"; [[ "$STORAGEPRIMS_TAGGER_NAME" == approved ]]; }
tag_checkout() { printf 'checkout\n' >>"$TRACE"; [[ "${FAIL_CHECKOUT:-0}" == 0 ]]; }
tag_key_selector() { printf 'pin\n' >>"$TRACE"; [[ "${FAIL_PIN:-0}" == 0 ]]; }
tag_expected_message() { printf 'message-ruleset\n' >>"$TRACE"; [[ "${FAIL_RULESET:-0}" == 0 ]]; }
SH
cat >"$scratch/bin/git" <<'SH'
#!/usr/bin/env bash
case "$*" in
  'symbolic-ref --quiet --short HEAD') printf '%s\n' "${BRANCH:-main}" ;;
  'status --porcelain --untracked-files=all')
    [[ "${DIRTY:-0}" == 0 ]] || printf '?? fixture\n' ;;
  *) exit 1 ;;
esac
SH
for action in tag push-tag; do
	label="$action"
	[[ "$action" == push-tag ]] && label=push
	cat >"$scratch/repo/scripts/release-$action.sh" <<SH
#!/usr/bin/env bash
printf '$label\\n' >>"\$TRACE"
SH
done
chmod +x "$scratch/bin/git" "$scratch/repo/scripts/"*.sh
FIXTURE_ROOT="$(cd "$scratch/repo" && pwd -P)"
export PATH="$scratch/bin:$PATH" TRACE="$scratch/trace" FIXTURE_ROOT
export STORAGEPRIMS_RELEASE_TAG=v1.2.3 STORAGEPRIMS_TAGGER_NAME=approved
operator="$scratch/repo/scripts/release-tag-operator.sh"
expect_fail() {
	: >"$TRACE"
	if "$@" >"$scratch/output" 2>&1; then
		echo 'error: expected operator rejection' >&2
		exit 1
	fi
	! grep -Eq '^(tag|push)$' "$TRACE"
}
expect_fail "$operator" invalid
expect_fail env -u STORAGEPRIMS_RELEASE_TAG "$operator" local-tag
expect_fail env STORAGEPRIMS_RELEASE_TAG=v1.2.4 "$operator" local-tag
expect_fail env STORAGEPRIMS_RELEASE_TAG=v01.2.3 "$operator" local-tag
expect_fail env STORAGEPRIMS_APPROVED_ENV_LOADER=relative "$operator" local-tag
expect_fail env STORAGEPRIMS_APPROVED_ENV_LOADER= "$operator" local-tag
expect_fail env STORAGEPRIMS_APPROVED_ENV_LOADER="$scratch/absent" "$operator" local-tag
expect_fail env STORAGEPRIMS_APPROVED_ENV_LOADER="$scratch/repo/VERSION" "$operator" local-tag
ln -s "$scratch/repo/VERSION" "$scratch/external/link"
expect_fail env STORAGEPRIMS_APPROVED_ENV_LOADER="$scratch/external/link" "$operator" local-tag
printf 'return 7\n' >"$scratch/external/failure"
expect_fail env STORAGEPRIMS_APPROVED_ENV_LOADER="$scratch/external/failure" "$operator" local-tag
printf 'set +e +u\nset +o pipefail\nreturn 7\n' >"$scratch/external/options"
expect_fail env STORAGEPRIMS_APPROVED_ENV_LOADER="$scratch/external/options" "$operator" local-tag
printf 'set +e +u\nset +o pipefail\n' >"$scratch/external/options-ok"
expect_fail env FAIL_PIN=1 STORAGEPRIMS_APPROVED_ENV_LOADER="$scratch/external/options-ok" "$operator" local-tag
printf 'STORAGEPRIMS_RELEASE_TAG=v1.2.4\n' >"$scratch/external/wrong-cut"
expect_fail env STORAGEPRIMS_APPROVED_ENV_LOADER="$scratch/external/wrong-cut" "$operator" local-tag
expect_fail env DIRTY=1 "$operator" local-tag
expect_fail env BRANCH=feature "$operator" local-tag
expect_fail env FAIL_CHECKOUT=1 "$operator" remote-push
expect_fail env FAIL_PIN=1 "$operator" local-tag
expect_fail env FAIL_RULESET=1 "$operator" remote-push
printf 'STORAGEPRIMS_TAGGER_NAME=approved\n' >"$scratch/external/valid"
printf 'cd "$FIXTURE_AWAY"\nSTORAGEPRIMS_TAGGER_NAME=approved\n' >"$scratch/external/change-dir"
export FIXTURE_AWAY="$scratch/external"
for mode in local-tag remote-push; do
	: >"$TRACE"
	env -u STORAGEPRIMS_TAGGER_NAME STORAGEPRIMS_APPROVED_ENV_LOADER="$scratch/external/valid" \
		"$operator" "$mode" >"$scratch/output" 2>&1
	if [[ "$mode" == local-tag ]]; then
		[[ "$(tail -1 "$TRACE")" == tag ]]
		if grep -qx push "$TRACE"; then exit 1; fi
	else
		[[ "$(tail -1 "$TRACE")" == push ]]
		if grep -qx tag "$TRACE"; then exit 1; fi
	fi
	[[ "$(head -n 5 "$TRACE")" == $'version\nidentity\ncheckout\npin\nmessage-ruleset' ]]
	: >"$TRACE"
	env -u STORAGEPRIMS_TAGGER_NAME STORAGEPRIMS_APPROVED_ENV_LOADER="$scratch/external/change-dir" \
		"$operator" "$mode" >"$scratch/output" 2>&1
	if [[ "$mode" == local-tag ]]; then
		[[ "$(tail -1 "$TRACE")" == tag ]]
	else
		[[ "$(tail -1 "$TRACE")" == push ]]
	fi
done
printf 'mv "$FIXTURE_ROOT" "$FIXTURE_ROOT.moved"\n' >"$scratch/external/move-root"
for mode in local-tag remote-push; do
	expect_fail env STORAGEPRIMS_APPROVED_ENV_LOADER="$scratch/external/move-root" "$operator" "$mode"
	mv "$scratch/repo.moved" "$scratch/repo"
done
for shell in bash zsh; do
	if command -v "$shell" >/dev/null 2>&1; then
		: >"$TRACE"
		"$shell" -c '"$1" local-tag' _ "$operator" >"$scratch/output" 2>&1
		[[ "$(tail -1 "$TRACE")" == tag ]]
	fi
done
echo '[ok] operator loader, guards, and separate dispatch controls passed'
