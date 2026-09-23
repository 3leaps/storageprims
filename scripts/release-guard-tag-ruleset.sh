#!/usr/bin/env bash

set -euo pipefail

readonly expected_repository="3leaps/storageprims"
readonly expected_ruleset_name="Tag Publish Protection"

require_command() {
	local name="$1"
	if ! command -v "${name}" >/dev/null 2>&1; then
		echo "error: ${name} is required on PATH" >&2
		exit 1
	fi
}

expected_policy_json() {
	jq -cnS \
		--arg repository "${expected_repository}" \
		--arg ruleset_name "${expected_ruleset_name}" \
		'{
            repository: $repository,
            ruleset_name: $ruleset_name,
            source_type: "Repository",
            target: "tag",
            enforcement: "active",
            conditions: {ref_name: {exclude: [], include: ["refs/tags/v*"]}},
            rules: ["creation", "deletion", "non_fast_forward", "update"],
            bypass_actors: [{actor_id: null, actor_type: "OrganizationAdmin", bypass_mode: "always"}]
        }'
}

policy_digest() {
	if command -v sha256sum >/dev/null 2>&1; then
		expected_policy_json | sha256sum | awk '{print $1}'
		return
	fi
	if command -v shasum >/dev/null 2>&1; then
		expected_policy_json | shasum -a 256 | awk '{print $1}'
		return
	fi
	echo "error: sha256sum or shasum is required" >&2
	return 1
}

policy_attestation() {
	printf 'Tag-Publish-Policy-SHA256: %s\n' "$(policy_digest)"
}

resolve_ruleset() {
	local pages ids count id
	pages="$(gh api --paginate --slurp "repos/${expected_repository}/rulesets?per_page=100")"
	ids="$(printf '%s\n' "${pages}" | jq -r \
		--arg name "${expected_ruleset_name}" \
		'flatten | map(select(.name == $name)) | .[].id')"
	count="$(printf '%s\n' "${ids}" | awk 'NF { count++ } END { print count + 0 }')"
	if [ "${count}" -ne 1 ]; then
		echo "error: expected exactly one '${expected_ruleset_name}' ruleset; found ${count}" >&2
		return 1
	fi
	id="$(printf '%s\n' "${ids}" | awk 'NF { print; exit }')"
	gh api "repos/${expected_repository}/rulesets/${id}"
}

validate_ruleset() {
	local ruleset="$1"
	if ! printf '%s\n' "${ruleset}" | jq -e \
		--arg name "${expected_ruleset_name}" \
		--arg repository "${expected_repository}" '
            .name == $name and
            .source_type == "Repository" and
            .source == $repository and
            .target == "tag" and
            .enforcement == "active" and
            .conditions == {"ref_name":{"exclude":[],"include":["refs/tags/v*"]}} and
            (.rules | length) == 4 and
            ([.rules[].type] | sort) == ["creation","deletion","non_fast_forward","update"] and
            all(.rules[]; (keys | sort) == ["type"]) and
            .bypass_actors == [{"actor_id":null,"actor_type":"OrganizationAdmin","bypass_mode":"always"}]
        ' >/dev/null; then
		echo "error: live tag ruleset does not match the required publication policy" >&2
		return 1
	fi
}

main() {
	local print_attestation=0
	local expected_attestation=0
	local verify_object=""
	if [ "${1:-}" = "--read-only" ]; then
		print_attestation=0
	elif [ "${1:-}" = "--verify-tag-attestation" ] && [ "$#" -eq 2 ]; then
		verify_object="$2"
	elif [ "${1:-}" = "--print-attestation" ]; then
		print_attestation=1
	elif [ "${1:-}" = "--expected-attestation" ]; then
		expected_attestation=1
	elif [ "$#" -ne 0 ]; then
		echo "error: unknown argument: $1" >&2
		exit 1
	fi

	require_command jq
	if [ "${expected_attestation}" -eq 1 ]; then
		policy_attestation
		exit 0
	fi
	require_command gh

	local ruleset
	ruleset="$(resolve_ruleset)"
	validate_ruleset "${ruleset}"
	if [ -n "$verify_object" ]; then
		[ "$(git cat-file -t "$verify_object" 2>/dev/null)" = tag ] || {
			echo 'error: annotated tag required' >&2
			exit 1
		}
		local message
		message="$(git cat-file tag "$verify_object" | sed -n '/^$/,$p' | sed '1d; /^-----BEGIN PGP SIGNATURE-----$/,$d')"
		[[ "$message" == *$'\n\n'"$(policy_attestation)" ]] || {
			echo 'error: signed policy attestation missing or mismatched' >&2
			exit 1
		}
	fi

	if [ "${print_attestation}" -eq 1 ]; then
		echo "[ok] tag ruleset matches the full publication policy" >&2
		policy_attestation
	else
		echo "[ok] tag ruleset matches the full publication policy"
	fi
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
	main "$@"
fi
