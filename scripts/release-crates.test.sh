#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
"$SCRIPT_DIR/release-crates.py" check
for crate in storageprims-ffi storageprims-cli; do
	if cargo metadata --no-deps --format-version 1 --locked |
		jq -e --arg name "$crate" 'any(.packages[]; .name == $name)' >/dev/null; then
		result="$(cargo publish --dry-run --locked --allow-dirty -p "$crate" 2>&1)" && {
			echo "error: unpublished crate $crate passed publish dry run" >&2
			exit 1
		}
		grep -q 'cannot be published' <<<"$result" || {
			echo "error: $crate failed for a reason other than publish = false" >&2
			exit 1
		}
	fi
done
python3 -B - "$SCRIPT_DIR" <<'PY'
import importlib.util
import pathlib
import sys

path = pathlib.Path(sys.argv[1]) / "release-crates.py"
spec = importlib.util.spec_from_file_location("release_crates", path)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
names = ["storageprims-core", "storageprims-s3", "storageprims-ops"]
packages = [
    {"name": "storageprims-core", "publish": None, "dependencies": []},
    {"name": "storageprims-s3", "publish": None, "dependencies": [{"name": "storageprims-core"}]},
    {"name": "storageprims-ops", "publish": None, "dependencies": [{"name": "storageprims-s3", "kind": "dev"}]},
    {"name": "storageprims-ffi", "publish": [], "dependencies": []},
]
manifests = {name: {"package": {"publish": True}} for name in names}
module.validate(packages, names, manifests)
for bad_names, bad_packages, bad_manifests in (
    (names[::-1], packages, manifests),
    (names[:-1], packages, manifests),
    (names + ["storageprims-ffi"], packages, manifests),
    (names, packages[:-1] + [{**packages[-1], "publish": None}], manifests),
    (names, packages + [{"name": "storageprims-cli", "publish": None, "dependencies": []}], manifests),
    (names, packages, {**manifests, "storageprims-core": {"package": {"publish": False}}}),
):
    try:
        module.validate(bad_packages, bad_names, bad_manifests)
    except ValueError:
        continue
    raise AssertionError("negative control unexpectedly passed")
print("[ok] publishable crate list and negative controls")
PY
