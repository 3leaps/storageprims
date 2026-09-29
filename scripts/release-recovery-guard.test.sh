#!/usr/bin/env bash
# Model actions/checkout replacing an annotated tag ref with its peeled commit.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd -P)"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
git init -q --bare "$scratch/remote.git"
git init -q -b main "$scratch/source"
git -C "$scratch/source" config user.name 'Fixture Author'
git -C "$scratch/source" config user.email 'fixture@example.invalid'
printf 'fixture\n' >"$scratch/source/file"
git -C "$scratch/source" add file
git -C "$scratch/source" commit -qm 'fixture commit'
git -C "$scratch/source" tag -am 'Fixture release' v1.2.3
fixture_commit="$(git -C "$scratch/source" rev-parse HEAD)"
object="$(git -C "$scratch/source" rev-parse refs/tags/v1.2.3)"
git -C "$scratch/source" remote add origin "$scratch/remote.git"
git -C "$scratch/source" push -q origin main refs/tags/v1.2.3
git clone -q "$scratch/remote.git" "$scratch/checkout"
git -C "$scratch/checkout" fetch -q --no-tags origin "+$fixture_commit:refs/tags/v1.2.3"
[[ "$(git -C "$scratch/checkout" cat-file -t refs/tags/v1.2.3)" == commit ]]

(
	cd "$scratch/checkout"
	source "$root/scripts/release-recovery-guard.sh"
	recover_annotated_ref v1.2.3 "$object" "$fixture_commit"
	[[ "$(git rev-parse refs/tags/v1.2.3)" == "$object" ]]
	[[ "$(git cat-file -t refs/tags/v1.2.3)" == tag ]]
	if recover_annotated_ref v1.2.3 "$object" 0000000000000000000000000000000000000000 >/dev/null 2>&1; then
		echo 'error: wrong approved commit was accepted' >&2
		exit 1
	fi
)

# A remote replacement must be rejected before changing a known-good local ref.
git -C "$scratch/remote.git" update-ref refs/tags/v1.2.3 "$fixture_commit"
(
	cd "$scratch/checkout"
	source "$root/scripts/release-recovery-guard.sh"
	if recover_annotated_ref v1.2.3 "$object" "$fixture_commit" >/dev/null 2>&1; then
		echo 'error: changed remote tag was accepted' >&2
		exit 1
	fi
	[[ "$(git rev-parse refs/tags/v1.2.3)" == "$object" ]]
)
mkdir -p "$scratch/bin"
cat >"$scratch/bin/gh" <<'SH'
#!/usr/bin/env bash
[[ "$*" == 'api -i repos/3leaps/storageprims/releases/tags/v1.2.3' ]] || exit 1
printf 'HTTP/2.0 %s\n' "${RELEASE_STATUS:-404}"
[[ "${RELEASE_STATUS:-404}" == 200 ]]
SH
chmod +x "$scratch/bin/gh"
(
	export PATH="$scratch/bin:$PATH"
	source "$root/scripts/release-recovery-guard.sh"
	require_absent_release v1.2.3
	for status in 200 403; do
		if RELEASE_STATUS="$status" require_absent_release v1.2.3 >/dev/null 2>&1; then
			echo 'error: existing or unreadable release was accepted' >&2
			exit 1
		fi
	done
)
echo '[ok] runner-local peeled tag repair and remote-object refusal passed'
