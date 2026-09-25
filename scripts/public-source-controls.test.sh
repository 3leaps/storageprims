#!/usr/bin/env bash
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
fixture="$(mktemp -d "${TMPDIR:-/tmp}/storageprims-public-source.XXXXXX")"
trap 'rm -rf "$fixture"' EXIT
git init -q -b main "$fixture"
mkdir -p "$fixture/scripts" "$fixture/crates/example/src" "$fixture/docs/decisions"
cp "$root/scripts/check-public-source.sh" "$fixture/scripts/"
printf '/// Selected reads use a guarded range.\n' >"$fixture/crates/example/src/lib.rs"
printf '# Source-guarded read\n' >"$fixture/docs/decisions/DDR-0001.md"
git -C "$fixture" add .

check="$fixture/scripts/check-public-source.sh"
"$check" >/dev/null

expect_violation() {
	local rule="$1" path="$2" value="$3" token="$4" output
	printf '%s\n' "$value" >"$fixture/$path"
	if output="$(cd "$fixture" && "$check" 2>&1)"; then
		echo 'error: public-source check missed an injected reference' >&2
		exit 1
	fi
	[[ "$output" == *"$rule"* && "$output" == *"$path"* && "$output" != *"$token"* ]] || {
		echo 'error: public-source diagnostic omitted its rule/file or exposed matched text' >&2
		exit 1
	}
	git -C "$fixture" checkout-index -f -- "$path"
}

opaque="$(printf '%s%s' QWER- 731)"
home="$(printf '%s%s' /home/ ExampleContributor/projects)/reference"
expect_violation 'opaque references' crates/example/src/lib.rs "/// $opaque selected read" "$opaque"
expect_violation 'opaque references' docs/decisions/DDR-0001.md "$opaque" "$opaque"
expect_violation 'private home paths' crates/example/src/lib.rs "/// $home" 'ExampleContributor'

real_git="$(command -v git)"
mkdir -p "$fixture/broken-bin"
cat >"$fixture/broken-bin/git" <<EOF
#!/usr/bin/env bash
if [[ "\$1" == grep ]]; then exit 128; fi
exec "$real_git" "\$@"
EOF
chmod +x "$fixture/broken-bin/git"
if output="$(cd "$fixture" && PATH="$fixture/broken-bin:$PATH" "$check" 2>&1)"; then
	echo 'error: public-source check ignored a scanner failure' >&2
	exit 1
fi
[[ "$output" == *'scan failed'* ]] || {
	echo 'error: public-source scanner failure was not identified' >&2
	exit 1
}
"$check" >/dev/null
echo '[ok] synthetic public-source controls passed'
