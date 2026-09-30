#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=release-crates-registry.sh
# shellcheck disable=SC1091
source "$SCRIPT_DIR/release-crates-registry.sh"
# Called indirectly by registry_wait before the failure override below.
# shellcheck disable=SC2329
cargo() { [[ "$*" == 'info --registry crates-io storageprims-core@0.1.2' ]]; }
curl() {
	[[ "$*" == *'--fail --silent --show-error --retry 3 --user-agent storageprims-release-verification https://crates.io/api/v1/crates/'* ]] || return 1
	case "${response_mode:-ok}" in
	ok) printf '%s\n' '{"version":{"num":"0.1.2","crate":"storageprims-core","yanked":false}}' ;;
	yanked) printf '%s\n' '{"version":{"num":"0.1.2","crate":"storageprims-core","yanked":true}}' ;;
	malformed) printf 'not JSON\n' ;;
	fail)
		# A failing transport must not pass even if stdout looks like a valid response.
		printf '%s\n' '{"version":{"num":"0.1.2","crate":"storageprims-core","yanked":false}}'
		return 22
		;;
	esac
}
registry_wait storageprims-core 0.1.2
registry_api_check storageprims-core 0.1.2
if registry_api_check storageprims-core 0.1.3 >/dev/null 2>&1; then
	echo 'error: mismatched version passed API check' >&2
	exit 1
fi
if registry_api_check storageprims-s3 0.1.2 >/dev/null 2>&1; then
	echo 'error: mismatched crate passed API check' >&2
	exit 1
fi
for response_mode in yanked malformed fail; do
	if registry_api_check storageprims-core 0.1.2 >/dev/null 2>&1; then
		echo "error: $response_mode API response was accepted" >&2
		exit 1
	fi
done
cargo() { return 1; }
sleep() { :; }
if registry_wait storageprims-core 0.1.3 >/dev/null 2>&1; then
	echo 'error: missing registry version passed index wait' >&2
	exit 1
fi
echo '[ok] registry index and API checks reject mismatched releases'
