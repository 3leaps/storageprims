#!/usr/bin/env bash
# Real archive inspection, with fixture-only network and Cargo responses.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd -P)"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
repo="$scratch/repo"
mkdir -p "$repo/scripts" "$repo/config/release" "$scratch/bin" "$scratch/registry"
cp "$root/scripts/"{release-crates-verify.sh,release-crates-registry.sh,release-crates-audit.py,crate-licenses.py} "$repo/scripts/"
# The auditor imports the real provenance helper, but plan checks are stubbed.
cp "$root/scripts/release-crates.py" "$repo/scripts/provenance.py"
sed 's@ROOT / "scripts/release-crates.py"@ROOT / "scripts/provenance.py"@' "$root/scripts/release-crates-audit.py" >"$repo/scripts/release-crates-audit.py"
printf '#!/usr/bin/env python3\nraise SystemExit(0)\n' >"$repo/scripts/release-crates.py"
printf '#!/usr/bin/env bash\nexit 0\n' >"$repo/scripts/release-guard-tag-version.sh"
chmod +x "$repo/scripts/"*.sh "$repo/scripts/release-crates.py"
printf '1.2.3\n' >"$repo/VERSION"
printf 'storageprims-core\n' >"$repo/config/release/publishable-crates.txt"
printf 'Synthetic MIT\n' >"$repo/LICENSE-MIT"
printf 'Synthetic Apache\n' >"$repo/LICENSE-APACHE"
git -C "$repo" init -q
git -C "$repo" config user.name fixture
git -C "$repo" config user.email fixture@example.invalid
git -C "$repo" commit -qm fixture --allow-empty
git -C "$repo" tag -am fixture v1.2.3
FIXTURE_COMMIT="$(git -C "$repo" rev-parse HEAD)"
export FIXTURE_COMMIT FIXTURE_REPO="$repo" FIXTURE_REGISTRY="$scratch/registry"
cat >"$scratch/bin/cargo" <<'STUB'
#!/usr/bin/env bash
if [[ "$1" == info ]]; then exit 0; fi
[[ "$1" == metadata ]] || exit 1
printf '{"workspace_root":"%s"}\n' "$FIXTURE_REPO"
STUB
cat >"$scratch/bin/curl" <<'STUB'
#!/usr/bin/env bash
[[ "$1" == "--disable" ]] || {
	echo "error: curl must start with --disable" >&2
	exit 2
}
expected_ua='storageprims-release-verify/1.0 (+https://github.com/3leaps/storageprims)'
has_ua=0
has_https=0
prev=""
for arg in "$@"; do
	if [[ "$prev" == "-A" || "$prev" == "--user-agent" ]]; then
		[[ "$arg" == "$expected_ua" ]] && has_ua=1
	fi
	if [[ "$prev" == "--proto" ]]; then
		[[ "$arg" == "=https" ]] && has_https=1
	fi
	prev="$arg"
done
[[ "$has_https" == 1 ]] || {
	echo "error: curl must restrict --proto to =https" >&2
	exit 2
}
[[ "$has_ua" == 1 ]] || {
	echo "curl: (56) The requested URL returned error: 403" >&2
	exit 56
}
case "${*: -1}" in
*api/v1/crates/storageprims-core/1.2.3) cat "$FIXTURE_REGISTRY/api.json" ;;
*storageprims-core-1.2.3.crate) cat "$FIXTURE_REGISTRY/storageprims-core-1.2.3.crate" ;;
*status.json) printf '{"doc_status":%s,"version":"%s"}\n' "${MOCK_DOCS:-true}" "${MOCK_DOCS_VERSION:-1.2.3}" ;;
*) exit 1 ;;
esac
STUB
chmod +x "$scratch/bin/"*
export PATH="$scratch/bin:$PATH" STORAGEPRIMS_RELEASE_TAG=v1.2.3
archive() {
	python3 - "$1" <<'PY'
import hashlib, io, json, os, pathlib, sys, tarfile
mode=sys.argv[1]; root=pathlib.Path(os.environ["FIXTURE_REPO"]); dest=pathlib.Path(os.environ["FIXTURE_REGISTRY"])
prefix="storageprims-core-1.2.3"
info={"git":{"sha1":os.environ["FIXTURE_COMMIT"]},"path_in_vcs":"crates/storageprims-core"}
if mode=="wrong-commit": info["git"]["sha1"]="0"*40
if mode=="dirty": info["git"]["dirty"]=True
if mode=="wrong-path": info["path_in_vcs"]="crates/other"
with tarfile.open(dest/(prefix+".crate"),"w:gz") as tf:
 for name in ["LICENSE-MIT","LICENSE-APACHE",".cargo_vcs_info.json"]:
  if mode=="missing-license" and name=="LICENSE-MIT": continue
  value=json.dumps(info).encode() if name.startswith(".") else (root/name).read_bytes()
  if mode=="wrong-license" and name=="LICENSE-MIT": value=b"wrong"
  member=tarfile.TarInfo(prefix+"/"+name); member.size=len(value)
  tf.addfile(member,io.BytesIO(value))
digest=hashlib.sha256((dest/(prefix+".crate")).read_bytes()).hexdigest()
if mode=="wrong-checksum": digest="0"*64
(dest/"api.json").write_text(json.dumps({"version":{"crate":"storageprims-core","num":"1.2.3","yanked":False,"checksum":digest}}))
PY
}
run() { (cd "$repo" && bash scripts/release-crates-verify.sh) >"$scratch/out" 2>&1; }
archive good
run || {
	cat "$scratch/out" >&2
	exit 1
}
for mode in wrong-commit dirty wrong-path missing-license wrong-license wrong-checksum; do
	archive "$mode"
	if run; then
		echo "error: accepted $mode" >&2
		exit 1
	fi
done
archive good
if MOCK_DOCS=false run; then
	echo 'error: accepted failed docs build' >&2
	exit 1
fi
if MOCK_DOCS_VERSION=9.9.9 run; then
	echo 'error: accepted wrong docs version' >&2
	exit 1
fi
# Regression control: omitting the explicit User-Agent on any curl request must
# fail closed with HTTP 403 (not merely a shell syntax error).
cp "$root/scripts/release-crates-registry.sh" "$scratch/orig-registry.sh"
awk '
	/-A "\$RELEASE_VERIFY_USER_AGENT"/ { next }
	{ print }
' "$scratch/orig-registry.sh" >"$repo/scripts/release-crates-registry.sh"
if run; then
	echo "error: accepted registry_api_check without explicit User-Agent" >&2
	exit 1
fi
grep -q 'returned error: 403' "$scratch/out" || {
	cat "$scratch/out" >&2
	exit 1
}
cp "$scratch/orig-registry.sh" "$repo/scripts/release-crates-registry.sh"

cp "$root/scripts/release-crates-verify.sh" "$scratch/orig-verify.sh"
for occurrence in 1 2 3; do
	awk -v target="$occurrence" '
		/-A "\$RELEASE_VERIFY_USER_AGENT"/ { count++; if (count == target) next }
		{ print }
	' "$scratch/orig-verify.sh" >"$repo/scripts/release-crates-verify.sh"
	chmod +x "$repo/scripts/release-crates-verify.sh"
	if run; then
		echo "error: accepted curl call #$occurrence without explicit User-Agent" >&2
		exit 1
	fi
	grep -q 'returned error: 403' "$scratch/out" || {
		cat "$scratch/out" >&2
		exit 1
	}
done
cp "$scratch/orig-verify.sh" "$repo/scripts/release-crates-verify.sh"
chmod +x "$repo/scripts/release-crates-verify.sh"
echo '[ok] registry archives: checksum, provenance, licenses, User-Agent and docs.rs failures rejected'
