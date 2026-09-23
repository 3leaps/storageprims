#!/usr/bin/env bash
# Signed-tag ceremony invariants. No signing material is stored in this repo.
set -euo pipefail

tag_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"

tag_die() {
	echo "error: $*" >&2
	return 1
}

tag_identity() {
	[[ "${STORAGEPRIMS_TAGGER_NAME:-} <${STORAGEPRIMS_TAGGER_EMAIL:-}>" == "$(cat "$tag_root/config/release/tagger-identity.txt")" ]] || {
		tag_die 'infosec tagger identity required'
		return 1
	}
	[[ "$STORAGEPRIMS_TAGGER_NAME" != *$'\n'* && "$STORAGEPRIMS_TAGGER_NAME" != *$'\r'* &&
		"$STORAGEPRIMS_TAGGER_NAME" != *'<'* && "$STORAGEPRIMS_TAGGER_NAME" != *'>'* ]] || {
		tag_die 'invalid tagger name'
		return 1
	}
	[[ "$STORAGEPRIMS_TAGGER_EMAIL" != *$'\n'* && "$STORAGEPRIMS_TAGGER_EMAIL" != *$'\r'* &&
		"$STORAGEPRIMS_TAGGER_EMAIL" != *'<'* && "$STORAGEPRIMS_TAGGER_EMAIL" != *'>'* ]] || {
		tag_die 'invalid tagger email'
		return 1
	}
}

tag_version() {
	[[ -n "${STORAGEPRIMS_RELEASE_TAG:-}" ]] || {
		tag_die 'STORAGEPRIMS_RELEASE_TAG required'
		return 1
	}
	"$tag_root/scripts/release-guard-tag-version.sh" >/dev/null
}

tag_message_file() {
	[[ -n "${STORAGEPRIMS_TAG_MESSAGE_DIR:-}" && -d "$STORAGEPRIMS_TAG_MESSAGE_DIR" &&
		! -L "$STORAGEPRIMS_TAG_MESSAGE_DIR" ]] || {
		tag_die 'tag message directory required'
		return 1
	}
	[[ "${STORAGEPRIMS_TAG_MESSAGE_DIR%/}" == */"$STORAGEPRIMS_RELEASE_TAG" ]] || {
		tag_die 'message directory must end in tag'
		return 1
	}
	local canonical
	canonical="$(cd "$STORAGEPRIMS_TAG_MESSAGE_DIR" && pwd -P)"
	case "$canonical" in "$tag_root" | "$tag_root"/*)
		tag_die 'message directory must be outside repository'
		return 1
		;;
	esac
	local file="${STORAGEPRIMS_TAG_MESSAGE_DIR%/}/message.txt"
	[[ -f "$file" && ! -L "$file" && -s "$file" ]] || {
		tag_die 'message.txt required'
		return 1
	}
	python3 - "$file" <<'PY' || {
import pathlib
import sys
data = pathlib.Path(sys.argv[1]).read_bytes()
try:
    data.decode('utf-8')
except UnicodeError:
    raise SystemExit(1)
if (b'\x00' in data or b'\r' in data or not data.endswith(b'\n')
        or data.endswith(b'\n\n') or any(line.rstrip(b' \t') != line for line in data.splitlines())):
    raise SystemExit(1)
PY
		tag_die 'message.txt must be UTF-8 with exactly one final newline'
		return 1
	}
	printf '%s\n' "$file"
}

tag_origin() {
	local url host
	url="$(git config --get remote.origin.url)"
	case "$url" in
	https://github.com/3leaps/storageprims | https://github.com/3leaps/storageprims.git | \
		git@github.com:3leaps/storageprims | git@github.com:3leaps/storageprims.git) ;;
	git@*:3leaps/storageprims | git@*:3leaps/storageprims.git)
		host="${url#git@}"
		host="${host%%:*}"
		[[ "$(ssh -G "$host" 2>/dev/null | awk '$1=="hostname" {print $2;exit}')" == github.com ]] || {
			tag_die 'origin SSH host must resolve to github.com'
			return 1
		}
		;;
	*)
		tag_die 'origin must be 3leaps/storageprims on GitHub'
		return 1
		;;
	esac
}

tag_checkout() {
	tag_origin
	git fetch --quiet origin '+refs/heads/main:refs/remotes/origin/main'
	[[ -z "$(git status --porcelain --untracked-files=normal)" ]] || tag_die 'clean checkout required'
	[[ "$(git rev-parse HEAD)" == "$(git rev-parse refs/remotes/origin/main)" ]] || tag_die 'HEAD must match fetched origin/main'
}

tag_selector_shape() {
	[[ "${STORAGEPRIMS_PGP_KEY_ID:-}" =~ ^[0-9A-F]{40}!$ ]] || {
		tag_die 'exact uppercase signing subkey fingerprint with ! required'
		return 1
	}
	[[ "${STORAGEPRIMS_GPG_SIGNING_FINGERPRINT:-}" =~ ^[0-9A-F]{40}$ ]] || {
		tag_die 'uppercase primary fingerprint required'
		return 1
	}
}

tag_key_selector() {
	tag_selector_shape || return 1
	"$tag_root/scripts/validate-release-anchors.sh" >/dev/null
	[[ "$(awk '$1=="gpg" {print $2}' "$tag_root/keys/expected-fingerprints.txt")" == "$STORAGEPRIMS_GPG_SIGNING_FINGERPRINT" ]] || {
		tag_die 'operator fingerprint differs from committed anchor'
		return 1
	}
	[[ -n "${STORAGEPRIMS_GPG_HOMEDIR:-}" && -d "$STORAGEPRIMS_GPG_HOMEDIR" ]] || {
		tag_die 'external GPG home required'
		return 1
	}
	local home
	home="$(cd "$STORAGEPRIMS_GPG_HOMEDIR" && pwd -P)"
	case "$home" in "$tag_root"/* | "$tag_root")
		tag_die 'GPG home must be outside repository'
		return 1
		;;
	esac
	export GNUPGHOME="$home"
	local listing primary subkey
	listing="$(gpg --batch --with-colons --fingerprint --with-subkey-fingerprint --list-keys "${STORAGEPRIMS_PGP_KEY_ID%!}" 2>/dev/null)" || tag_die 'signing subkey unavailable'
	primary="$(printf '%s\n' "$listing" | awk -F: '$1=="pub" {p=1;next} p && $1=="fpr" {print $10;exit}')"
	subkey="$(printf '%s\n' "$listing" | awk -F: -v f="${STORAGEPRIMS_PGP_KEY_ID%!}" '$1=="sub" {s=1;cap=$12;next} s && $1=="fpr" {if ($10==f && cap ~ /[sS]/) print $10; s=0}')"
	[[ "$primary" == "$STORAGEPRIMS_GPG_SIGNING_FINGERPRINT" && "$subkey" == "${STORAGEPRIMS_PGP_KEY_ID%!}" ]] || {
		tag_die 'signing subkey not on pinned primary'
		return 1
	}
}

tag_expected_message() {
	local file
	file="$(tag_message_file)" || return 1
	cat "$file"
	printf '\n'
	"$tag_root/scripts/release-guard-tag-ruleset.sh" --print-attestation
}

tag_verify_object() {
	local object="$1" expected_file="$2" tagger actual_file
	[[ "$(git cat-file -t "$object" 2>/dev/null)" == tag ]] || {
		tag_die 'annotated tag object required'
		return 1
	}
	[[ "$(git cat-file tag "$object" | sed -n 's/^tag //p' | head -1)" == "$STORAGEPRIMS_RELEASE_TAG" ]] || {
		tag_die 'tag name mismatch'
		return 1
	}
	[[ "$(git rev-parse "${object}^{}")" == "$(git rev-parse HEAD)" ]] || {
		tag_die 'tag target mismatch'
		return 1
	}
	[[ "$(git rev-parse HEAD)" == "$(git rev-parse refs/remotes/origin/main)" ]] || {
		tag_die 'tag not at main'
		return 1
	}
	tagger="$(git cat-file tag "$object" | sed -n 's/^tagger \(.*\) [0-9][0-9]* [+-][0-9][0-9][0-9][0-9]$/\1/p' | head -1)"
	[[ "$tagger" == "$STORAGEPRIMS_TAGGER_NAME <$STORAGEPRIMS_TAGGER_EMAIL>" ]] || {
		tag_die 'tagger identity mismatch'
		return 1
	}
	actual_file="$(mktemp)"
	if ! "$tag_root/scripts/release-tag-body.sh" "$object" >"$actual_file"; then
		rm -f "$actual_file"
		return 1
	fi
	if ! cmp -s "$expected_file" "$actual_file"; then
		rm -f "$actual_file"
		tag_die 'signed tag message mismatch'
		return 1
	fi
	rm -f "$actual_file"
}
