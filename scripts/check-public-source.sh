#!/usr/bin/env bash
# Check tracked text for common forms of non-public references.
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
cd "$root"

check_class() {
	local rule="$1" pattern="$2" files rc
	if files="$(git grep -l -I -E "$pattern" -- .)"; then
		echo "error: public-source check found $rule in tracked files:" >&2
		printf '%s\n' "$files" >&2
		return 1
	else
		rc=$?
		if [[ "$rc" != 1 ]]; then
			echo "error: public-source scan failed for $rule ($rc)" >&2
			return "$rc"
		fi
	fi
}

check_class 'opaque references' '(^|[^[:alnum:]_])[A-Z]{4,8}-[0-9]{3}([^0-9]|$)'
check_class 'private home paths' "(/Users/|/home/)[A-Za-z0-9._-]+/|[A-Za-z]:\\\\Users\\\\[A-Za-z0-9._-]+\\\\"

echo '[ok] tracked public-source checks passed'
