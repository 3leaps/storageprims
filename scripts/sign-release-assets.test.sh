#!/usr/bin/env bash
# Exercise prompt context and signing failures using synthetic tools, never keys.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd -P)"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
mkdir -p "$scratch/repo/scripts" "$scratch/repo/dist/release" "$scratch/bin"
cp "$root/scripts/sign-release-assets.sh" "$scratch/repo/scripts/"
printf 'synthetic file\n' >"$scratch/key.fixture"
printf 'synthetic public file\n' >"$scratch/public.fixture"
cat >"$scratch/repo/scripts/release-common.sh" <<'SH'
require_release_guard() { [[ "$1" == 1 ]]; }
release_tag() { printf 'v1.2.3\n'; }
release_repo_root() { printf '%s\n' "$MOCK_ROOT"; }
SH
for script in verify-checksums.sh validate-release-assets.sh; do
	printf '#!/usr/bin/env bash\nexit 0\n' >"$scratch/repo/scripts/$script"
	chmod +x "$scratch/repo/scripts/$script"
done
cat >"$scratch/bin/minisign" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
[[ "$#" == 9 && "$1" == -S && "$2" == -s && "$3" == "$STORAGEPRIMS_MINISIGN_KEY" &&
   "$4" == -m && "$6" == -t && "$7" == 'storageprims v1.2.3' && "$8" == -x &&
   "$9" == "$5.minisig" ]] || exit 2
manifest="${5##*/}"
printf 'mock-sign %s\n' "$manifest"
[[ "${MOCK_FAIL:-}" != "$manifest" ]] || exit 7
printf 'synthetic signature\n' >"$9"
SH
chmod +x "$scratch/bin/minisign"
export PATH="$scratch/bin:$PATH" MOCK_ROOT="$scratch/repo"
export STORAGEPRIMS_MINISIGN_KEY="$scratch/key.fixture" STORAGEPRIMS_MINISIGN_PUB="$scratch/public.fixture"
unset STORAGEPRIMS_PGP_KEY_ID STORAGEPRIMS_GPG_HOMEDIR
cd "$scratch/repo"
bash scripts/sign-release-assets.sh >"$scratch/output"
python3 - "$scratch/output" "$STORAGEPRIMS_MINISIGN_KEY" "$STORAGEPRIMS_MINISIGN_PUB" <<'PY'
import pathlib, sys
text = pathlib.Path(sys.argv[1]).read_text()
for manifest in ('SHA256SUMS', 'SHA512SUMS'):
    context = f'[info] Signing {manifest} with minisign; enter the minisign key passphrase if prompted (one prompt per manifest).'
    assert text.count(context) == 1
    assert text.index(context) < text.index(f'mock-sign {manifest}')
assert all(value not in text for value in sys.argv[2:])
assert '[ok] checksum manifests signed' in text
PY
for manifest in SHA256SUMS SHA512SUMS; do
	set +e
	MOCK_FAIL="$manifest" bash scripts/sign-release-assets.sh >"$scratch/failure"
	status=$?
	set -e
	[[ "$status" == 7 ]]
	if grep -q '\[ok\] checksum manifests signed' "$scratch/failure"; then
		echo 'error: failed signing reported success' >&2
		exit 1
	fi
	if [[ "$manifest" == SHA256SUMS ]]; then
		if grep -q 'mock-sign SHA512SUMS' "$scratch/failure"; then
			echo 'error: signing continued after failure' >&2
			exit 1
		fi
	fi
done
echo '[ok] minisign prompt context, command arguments, and failure stops passed'
