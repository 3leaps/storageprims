#!/usr/bin/env bash
# release-github-preflight.sh passes only for the required repository settings
# and fails closed on every drift, unreadable or malformed answer, transport
# override, or host override, without contacting GitHub.
set -euo pipefail
unset GH_HOST GH_REPO GH_ENTERPRISE_TOKEN GITHUB_ENTERPRISE_TOKEN
root="$(cd "$(dirname "$0")/.." && pwd -P)"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
api="$scratch/api"
mkdir -p "$api" "$scratch/bin"

pristine() {
	rm -f "$api"/*
	printf '%s\n' '{"default_workflow_permissions":"read","can_approve_pull_request_reviews":false}' \
		>"$api/workflow.json"
	# Ruleset list pages, as `gh api --paginate --slurp` returns them.
	printf '%s\n' '[[{"id":7,"name":"Tag Publish Protection"},{"id":8,"name":"Branch rules"}]]' \
		>"$api/rulesets.json"
	cat >"$api/ruleset-7.json" <<'JSON'
{"name":"Tag Publish Protection","source_type":"Repository","source":"3leaps/storageprims",
 "target":"tag","enforcement":"active",
 "conditions":{"ref_name":{"include":["refs/tags/v*"],"exclude":[]}},
 "rules":[{"type":"creation"},{"type":"update"},{"type":"deletion"},{"type":"non_fast_forward"}],
 "bypass_actors":[{"actor_id":null,"actor_type":"OrganizationAdmin","bypass_mode":"always"}]}
JSON
	printf '%s\n' '{"enabled":true,"enforced_by_owner":false}' >"$api/immutable.json"
	: >"$api/socket"
}

cat >"$scratch/bin/gh" <<SH
#!/usr/bin/env bash
set -euo pipefail
if [[ "\$1 \$2 \$3" == 'config get http_unix_socket' ]]; then
	cat "$api/socket"
	exit 0
fi
[[ "\$1" == api && "\$GH_HOST" == github.com && "\${*: -2:1}" == --hostname && "\${*: -1}" == github.com ]] || exit 9
args=("\$@")
path="\${args[\$#-3]}"
case "\$path" in
repos/3leaps/storageprims/actions/permissions/workflow) f=workflow.json ;;
'repos/3leaps/storageprims/rulesets?per_page=100')
	[[ "\$*" == *'--paginate --slurp'* ]] || exit 8
	f=rulesets.json ;;
repos/3leaps/storageprims/rulesets/7) f=ruleset-7.json ;;
repos/3leaps/storageprims/immutable-releases) f=immutable.json ;;
*) exit 4 ;;
esac
[[ -f "$api/\$f" ]] || { echo 'HTTP 403: Resource not accessible' >&2; exit 1; }
cat "$api/\$f"
SH
chmod +x "$scratch/bin/gh"
export PATH="$scratch/bin:$PATH"

run() { bash "$root/scripts/release-github-preflight.sh" >"$scratch/out" 2>&1; }
cases=0
refused() {
	local reason="$1"
	cases=$((cases + 1))
	if run; then
		echo "error: preflight accepted, expected: $reason" >&2
		cat "$scratch/out" >&2
		exit 1
	fi
	grep -q -- "$reason" "$scratch/out" || {
		echo "error: refused for another reason than '$reason'" >&2
		cat "$scratch/out" >&2
		exit 1
	}
	pristine
}
# edit FILE JQ: rewrite one fixture.
edit() { jq -c "$2" "$api/$1" >"$api/$1.new" && mv "$api/$1.new" "$api/$1"; }

pristine
run || {
	cat "$scratch/out" >&2
	exit 1
}
[[ "$(grep -c '^\[ok\]' "$scratch/out")" == 3 ]]

# Actions token defaults.
edit workflow.json '.default_workflow_permissions = "write"'
refused 'default GITHUB_TOKEN must be read-only'
edit workflow.json '.can_approve_pull_request_reviews = true'
refused 'Actions must not approve pull requests'
rm "$api/workflow.json"
refused 'could not read repos/3leaps/storageprims/actions/permissions/workflow'
# gh's own error reaches the maintainer.
grep -q 'gh: HTTP 403: Resource not accessible' "$scratch/out" || {
	echo 'error: gh error detail was not shown' >&2
	exit 1
}

# Tag ruleset lookup, including later pages.
edit rulesets.json '.[0] += [{"id":9,"name":"Tag Publish Protection"}]'
refused "expected exactly one 'Tag Publish Protection' ruleset"
edit rulesets.json '. + [[{"id":9,"name":"Tag Publish Protection"}]]'
refused "expected exactly one 'Tag Publish Protection' ruleset"
edit rulesets.json 'map(map(select(.name != "Tag Publish Protection")))'
refused "expected exactly one 'Tag Publish Protection' ruleset"

# Tag ruleset policy: every field.
for change in '.name = "Other"' '.source_type = "Organization"' '.source = "other/repo"' \
	'.target = "branch"' '.enforcement = "evaluate"' \
	'.conditions.ref_name.include = ["refs/tags/*"]' \
	'.conditions.ref_name.include = ["refs/tags/v*", "refs/tags/*"]' \
	'.conditions.ref_name.exclude = ["refs/tags/v0.*"]' \
	'.rules = [{"type":"creation"},{"type":"update"},{"type":"deletion"}]' \
	'.rules = {"a":{"type":"creation"},"b":{"type":"update"},"c":{"type":"deletion"},"d":{"type":"non_fast_forward"}}' \
	'.bypass_actors += [{"actor_id":5,"actor_type":"Team","bypass_mode":"always"}]'; do
	edit ruleset-7.json "$change"
	refused 'does not match the required tag protection policy'
done
rm "$api/ruleset-7.json"
refused 'could not read repos/3leaps/storageprims/rulesets/7'

# Immutable releases.
edit immutable.json '.enabled = false'
refused 'immutable releases are not enabled'
rm "$api/immutable.json"
refused 'could not read repos/3leaps/storageprims/immutable-releases'

# Malformed answers: a second document, a non-object, or a non-array list.
printf '%s\n%s\n' '{"default_workflow_permissions":"write","can_approve_pull_request_reviews":true}' \
	'{"default_workflow_permissions":"read","can_approve_pull_request_reviews":false}' >"$api/workflow.json"
refused 'unexpected response shape from repos/3leaps/storageprims/actions/permissions/workflow'
printf '%s\n%s\n' '{"enabled":false}' '{"enabled":true}' >"$api/immutable.json"
refused 'unexpected response shape from repos/3leaps/storageprims/immutable-releases'
printf '%s\n' '[{"enabled":true}]' >"$api/immutable.json"
refused 'unexpected response shape from repos/3leaps/storageprims/immutable-releases'
printf '%s\n' '[{"id":7,"name":"Tag Publish Protection"}]' >"$api/rulesets.json"
refused 'unexpected response shape from repos/3leaps/storageprims/rulesets'
printf '%s\n' '[{"a":{"id":7,"name":"Tag Publish Protection"}}]' >"$api/rulesets.json"
refused 'unexpected response shape from repos/3leaps/storageprims/rulesets'

# Transport and host overrides.
printf '%s\n' /tmp/proxy.sock >"$api/socket"
refused 'gh is configured with http_unix_socket'
for name in GH_HOST GH_REPO GH_ENTERPRISE_TOKEN GITHUB_ENTERPRISE_TOKEN; do
	cases=$((cases + 1))
	if env "$name=x" bash "$root/scripts/release-github-preflight.sh" >"$scratch/out" 2>&1; then
		echo "error: preflight accepted $name" >&2
		exit 1
	fi
	grep -q -- "$name is set" "$scratch/out" || {
		cat "$scratch/out" >&2
		exit 1
	}
done
echo "[ok] GitHub preflight: required settings pass; $cases drift, unreadable, malformed, paginated-duplicate, transport and host-override cases refused"
