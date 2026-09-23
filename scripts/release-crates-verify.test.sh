#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=release-crates-registry.sh
# shellcheck disable=SC1091
source "$SCRIPT_DIR/release-crates-registry.sh"
cargo() { [[ "$*" == 'info --registry crates-io storageprims-core@0.1.2' ]]; }
curl() { printf '%s\n' '{"version":{"num":"0.1.2","crate":"storageprims-core","yanked":false}}'; }
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
cargo() { return 1; }
sleep() { :; }
if registry_wait storageprims-core 0.1.3 >/dev/null 2>&1; then
	echo 'error: missing registry version passed index wait' >&2
	exit 1
fi
echo '[ok] registry index and API checks reject mismatched releases'
