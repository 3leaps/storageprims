#!/usr/bin/env bash
# Read-only GitHub repository preflight for the release ceremony:
#   1. The repository default GITHUB_TOKEN permission is read-only, and GitHub
#      Actions may not create or approve pull requests.
#   2. Exactly one active repository `Tag Publish Protection` ruleset protects
#      exactly `refs/tags/v*` against creation, update, deletion, and
#      non-fast-forward, with only an OrganizationAdmin bypass.
#   3. Immutable releases are enabled on the repository.
#
# The settings endpoints need repository administration read access, so this
# runs on the maintainer machine, not in CI. Any unreadable or malformed
# answer fails. The gh configuration in use is trusted: requests are pinned to
# github.com, and a configured Unix-socket transport is refused.
set -euo pipefail

die() {
	echo "error: $*" >&2
	exit 1
}

repository=3leaps/storageprims

for command_name in gh jq; do
	command -v "$command_name" >/dev/null 2>&1 || die "$command_name not found in PATH"
done

# Talk to github.com only: refuse inherited host or repository overrides and a
# Unix-socket transport before any request, and pin the host for every call.
for name in GH_HOST GH_REPO GH_ENTERPRISE_TOKEN GITHUB_ENTERPRISE_TOKEN; do
	[[ -z "${!name+x}" ]] || die "$name is set; the preflight talks to github.com only"
done
socket="$(gh config get http_unix_socket -h github.com 2>/dev/null || true)"
[[ -z "$socket" ]] || die 'gh is configured with http_unix_socket; the preflight requires a direct github.com transport'

scratch="$(mktemp -d "${TMPDIR:-/tmp}/storageprims-gh-preflight.XXXXXX")"
trap 'rm -rf "$scratch"' EXIT

# api_object PATH: one JSON object from the GitHub API, or stop with gh's error.
api_object() {
	local path="$1" body
	body="$(GH_HOST=github.com gh api "$path" --hostname github.com 2>"$scratch/err")" || {
		sed 's/^/  gh: /' "$scratch/err" >&2
		die "could not read $path"
	}
	jq -se 'length == 1 and (.[0] | type) == "object"' >/dev/null 2>&1 <<<"$body" ||
		die "unexpected response shape from $path"
	jq -c '.' <<<"$body"
}

# api_list PATH: every page of a list endpoint, merged into one JSON array.
api_list() {
	local path="$1" body
	body="$(GH_HOST=github.com gh api --paginate --slurp "$path" --hostname github.com 2>"$scratch/err")" || {
		sed 's/^/  gh: /' "$scratch/err" >&2
		die "could not read $path"
	}
	jq -se 'length == 1 and (.[0] | type) == "array" and all(.[0][]; type == "array" and all(.[]; type == "object"))' \
		>/dev/null 2>&1 <<<"$body" || die "unexpected response shape from $path"
	jq -c 'add // []' <<<"$body"
}

check_workflow_token() {
	local settings
	settings="$(api_object "repos/$repository/actions/permissions/workflow")"
	jq -e '.default_workflow_permissions == "read" and .can_approve_pull_request_reviews == false' \
		>/dev/null <<<"$settings" ||
		die "the default GITHUB_TOKEN must be read-only and Actions must not approve pull requests"
	echo "[ok] GitHub Actions: default GITHUB_TOKEN read-only; Actions cannot approve pull requests"
}

check_tag_ruleset() {
	local rulesets id ruleset
	rulesets="$(api_list "repos/$repository/rulesets?per_page=100")"
	id="$(jq -r '[ .[] | select(.name == "Tag Publish Protection") ] | if length == 1 then .[0].id else empty end' \
		<<<"$rulesets")"
	[[ "$id" =~ ^[1-9][0-9]*$ ]] || die "expected exactly one 'Tag Publish Protection' ruleset on $repository"
	ruleset="$(api_object "repos/$repository/rulesets/$id")"
	jq -e --arg repo "$repository" '
		.name == "Tag Publish Protection" and
		.source_type == "Repository" and
		.source == $repo and
		.target == "tag" and
		.enforcement == "active" and
		.conditions.ref_name.include == ["refs/tags/v*"] and
		.conditions.ref_name.exclude == [] and
		(.rules | type) == "array" and
		([.rules[].type] | sort) == ["creation", "deletion", "non_fast_forward", "update"] and
		.bypass_actors == [{"actor_id": null, "actor_type": "OrganizationAdmin", "bypass_mode": "always"}]
	' >/dev/null <<<"$ruleset" ||
		die "ruleset 'Tag Publish Protection' does not match the required tag protection policy"
	echo "[ok] GitHub ruleset 'Tag Publish Protection': active on refs/tags/v*"
}

check_immutable_releases() {
	local settings
	settings="$(api_object "repos/$repository/immutable-releases")"
	jq -e '.enabled == true' >/dev/null <<<"$settings" ||
		die "immutable releases are not enabled on $repository"
	echo "[ok] GitHub immutable releases: enabled"
}

check_workflow_token
check_tag_ruleset
check_immutable_releases
