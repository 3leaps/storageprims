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
	local mode="${2:-full}"
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
            all(.rules[]; (keys | sort) == ["type"])
        ' >/dev/null; then
		echo "error: live tag ruleset does not match the required publication policy" >&2
		return 1
	fi
	local actors='[{"actor_id":null,"actor_type":"OrganizationAdmin","bypass_mode":"always"}]'
	case "$mode" in
	full)
		jq -e --argjson actors "$actors" '.bypass_actors == $actors' >/dev/null <<<"$ruleset" || {
			echo 'error: full ruleset must restrict bypass to organization administrators' >&2
			return 1
		}
		;;
	read-only)
		jq -e --argjson actors "$actors" \
			'(.bypass_actors == null) or (.bypass_actors == []) or (.bypass_actors == $actors)' \
			>/dev/null <<<"$ruleset" || {
			echo 'error: unexpected visible bypass actors' >&2
			return 1
		}
		;;
	*)
		echo 'error: invalid ruleset validation mode' >&2
		return 1
		;;
	esac
}

main() {
	local mode=full
	local print_attestation=0
	local expected_attestation=0
	local verify_object=""
	while [ "$#" -gt 0 ]; do
		case "$1" in
		--read-only) mode=read-only ;;
		--verify-tag-attestation)
			shift
			[[ -n "${1:-}" ]] || {
				echo 'error: missing tag object' >&2
				exit 1
			}
			verify_object="$1"
			;;
		--print-attestation) print_attestation=1 ;;
		--expected-attestation) expected_attestation=1 ;;
		*)
			echo "error: unknown argument: $1" >&2
			exit 1
			;;
		esac
		shift
	done
	[[ "$print_attestation" -eq 0 || "$mode" == full ]] || {
		echo 'error: full policy required for attestation' >&2
		exit 1
	}

	require_command jq
	if [ "${expected_attestation}" -eq 1 ]; then
		policy_attestation
		exit 0
	fi
	require_command gh

	local ruleset
	ruleset="$(resolve_ruleset)"
	validate_ruleset "${ruleset}" "$mode"
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
		echo "[ok] tag ruleset matches the $mode publication policy"
	fi
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
	main "$@"
fi
