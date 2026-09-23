#!/usr/bin/env bash
# Read-only index and crates.io API checks, also used between manual publishes.
registry_wait() {
	local crate="$1" version="$2" attempt
	for attempt in {1..30}; do
		if cargo info --registry crates-io "${crate}@${version}" >/dev/null 2>&1; then
			return 0
		fi
		sleep 10
	done
	echo "error: crates.io index did not expose ${crate}@${version} after ${attempt} attempts" >&2
	return 1
}

registry_api_check() {
	local crate="$1" version="$2" response
	response="$(curl --fail --silent --show-error --retry 3 \
		"https://crates.io/api/v1/crates/${crate}/${version}")"
	jq -e --arg name "$crate" --arg version "$version" \
		'.version.num == $version and .version.crate == $name and .version.yanked == false' \
		<<<"$response" >/dev/null || {
		echo "error: crates.io API did not confirm ${crate}@${version}" >&2
		return 1
	}
}
