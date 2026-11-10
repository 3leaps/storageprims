#!/usr/bin/env bash
# release-github-preflight.sh passes only for the required repository settings
# and fails closed on every drift or unreadable setting, without contacting
# GitHub.
set -euo pipefail
unset GH_HOST GH_REPO GH_ENTERPRISE_TOKEN GITHUB_ENTERPRISE_TOKEN
root="$(cd "$(dirname "$0")/.." && pwd -P)"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
api="$scratch/api"
mkdir -p "$api" "$scratch/bin"

pristine() {
	printf '%s\n' '{"default_workflow_permissions":"read","can_approve_pull_request_reviews":false}' \
		>"$api/workflow.json"
	printf '%s\n' '[{"id":7,"name":"Tag Publish Protection"},{"id":8,"name":"Branch rules"}]' \
		>"$api/rulesets.json"
	cat >"$api/ruleset-7.json" <<'JSON'
{"name":"Tag Publish Protection","source_type":"Repository","source":"3leaps/storageprims",
 "target":"tag","enforcement":"active",
 "conditions":{"ref_name":{"include":["refs/tags/v*"],"exclude":[]}},
 "rules":[{"type":"creation"},{"type":"update"},{"type":"deletion"},{"type":"non_fast_forward"}],
 "bypass_actors":[{"actor_id":null,"actor_type":"OrganizationAdmin","bypass_mode":"always"}]}
JSON
	printf '%s\n' '{"enabled":true,"enforced_by_owner":false}' >"$api/immutable.json"
}

cat >"$scratch/bin/gh" <<SH
#!/usr/bin/env bash
set -euo pipefail
[[ "\$GH_HOST" == github.com && "\${*: -2:1}" == --hostname && "\${*: -1}" == github.com ]] || exit 9
case "\$2" in
repos/3leaps/storageprims/actions/permissions/workflow) f=workflow.json ;;
'repos/3leaps/storageprims/rulesets?per_page=100') f=rulesets.json ;;
repos/3leaps/storageprims/rulesets/7) f=ruleset-7.json ;;
repos/3leaps/storageprims/immutable-releases) f=immutable.json ;;
*) exit 4 ;;
esac
[[ -f "$api/\$f" ]] || { echo 'HTTP 403' >&2; exit 1; }
cat "$api/\$f"
SH
chmod +x "$scratch/bin/gh"
export PATH="$scratch/bin:$PATH"

run() { bash "$root/scripts/release-github-preflight.sh" >"$scratch/out" 2>&1; }
refused() {
	local reason="$1"
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
refused 'could not read the Actions workflow permissions'

# Tag ruleset.
edit rulesets.json '. + [{"id":9,"name":"Tag Publish Protection"}]'
refused "expected exactly one 'Tag Publish Protection' ruleset"
edit rulesets.json 'map(select(.name != "Tag Publish Protection"))'
refused "expected exactly one 'Tag Publish Protection' ruleset"
edit ruleset-7.json '.enforcement = "evaluate"'
refused 'does not match the required tag protection policy'
edit ruleset-7.json '.conditions.ref_name.include = ["refs/tags/*"]'
refused 'does not match the required tag protection policy'
edit ruleset-7.json '.conditions.ref_name.exclude = ["refs/tags/v0.*"]'
refused 'does not match the required tag protection policy'
edit ruleset-7.json '.rules = [{"type":"creation"},{"type":"update"},{"type":"deletion"}]'
refused 'does not match the required tag protection policy'
edit ruleset-7.json '.bypass_actors += [{"actor_id":5,"actor_type":"Team","bypass_mode":"always"}]'
refused 'does not match the required tag protection policy'
edit ruleset-7.json '.source_type = "Organization"'
refused 'does not match the required tag protection policy'
rm "$api/ruleset-7.json"
refused 'could not read ruleset 7'

# Immutable releases.
edit immutable.json '.enabled = false'
refused 'immutable releases are not enabled'
rm "$api/immutable.json"
refused 'could not read the immutable releases setting'

# Only github.com.
GH_HOST=example.invalid refused 'GH_HOST is set'
GH_REPO=other/repo refused 'GH_REPO is set'
echo '[ok] GitHub preflight: required settings pass; token write default, Actions PR approval, missing, duplicate, inactive, widened, excluded, weakened or bypassed tag ruleset, disabled or unreadable immutable releases, and host overrides refused'
