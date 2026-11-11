#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Cargo configuration that substitutes sources or names a program for Cargo to
# run is refused, in the environment, in configuration files, and in the
# tagged source tree.
set -euo pipefail
unset RUSTFLAGS RUSTDOCFLAGS CARGO_ENCODED_RUSTFLAGS CARGO_ENCODED_RUSTDOCFLAGS
root="$(cd "$(dirname "$0")/.." && pwd -P)"
python3 -B - "$root/scripts/check-registry-config.py" <<'PY'
import importlib.util
import sys
spec = importlib.util.spec_from_file_location('registry_config', sys.argv[1])
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
module.validate({'build': {'jobs': 2}, 'registry': {'token': 'synthetic'}, 'net': {'retry': 3},
                 'registries': {'crates-io': {'token': 'synthetic'}}},
                {'CARGO_REGISTRY_TOKEN': 'synthetic', 'CARGO_REGISTRIES_CRATES_IO_TOKEN': 'synthetic', 'CARGO_TERM_COLOR': 'always'})
for config, env in [
    ({'patch': {'crates-io': {}}}, {}),
    ({'paths': ['synthetic']}, {}),
    ({'source': {'crates-io': {'replace-with': 'other'}}}, {}),
    ({'replace': {}}, {}),
    ({'include': ['other.toml']}, {}),
    ({'registries': {'crates-io': {'index': 'synthetic'}}}, {}),
    ({}, {'CARGO_REGISTRIES_CRATES_IO_INDEX': 'synthetic'}),
    ({'env': {'CARGO_REGISTRY_INDEX': 'synthetic'}}, {}),
    # Commands Cargo would run inside the release source.
    ({'build': {'rustc-wrapper': 'scripts/wrapper'}}, {}),
    ({'build': {'rustc-workspace-wrapper': 'scripts/wrapper'}}, {}),
    ({'build': {'rustc': '/synthetic/rustc'}}, {}),
    ({'build': {'target-dir': 'elsewhere'}}, {}),
    ({'target': {'x86_64-unknown-linux-gnu': {'linker': 'scripts/linker'}}}, {}),
    ({'target': {'x86_64-unknown-linux-gnu': {'runner': 'scripts/runner'}}}, {}),
    ({'host': {'linker': 'scripts/linker'}}, {}),
    ({'alias': {'publish': 'run --bin synthetic'}}, {}),
    ({'registry': {'credential-provider': 'scripts/provider'}}, {}),
    ({'registry': {'global-credential-providers': ['scripts/provider']}}, {}),
    ({'registries': {'crates-io': {'credential-provider': 'scripts/provider'}}}, {}),
    ({'unstable': {}}, {}),
    ({}, {'RUSTC_WRAPPER': 'synthetic'}),
    ({}, {'RUSTC_WORKSPACE_WRAPPER': 'synthetic'}),
    ({}, {'RUSTC': 'synthetic'}),
    ({}, {'CARGO_BUILD_RUSTC_WRAPPER': 'synthetic'}),
    ({}, {'CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER': 'synthetic'}),
    ({}, {'CARGO_ALIAS_PUBLISH': 'synthetic'}),
    ({}, {'CARGO_REGISTRY_CREDENTIAL_PROVIDER': 'synthetic'}),
    ({}, {'CARGO_REGISTRIES_CRATES_IO_CREDENTIAL_PROVIDER': 'synthetic'}),
    ({'env': {'RUSTC_WRAPPER': 'synthetic'}}, {}),
    ({'env': {'PATH': {'value': '/synthetic', 'force': True}}}, {}),
    ({}, {'RUSTFLAGS': '-C linker=/synthetic'}),
    ({}, {'CARGO_ENCODED_RUSTFLAGS': '-Clinker=/synthetic'}),
    ({}, {'RUSTDOCFLAGS': 'x'}),
    ({}, {'LD_PRELOAD': '/synthetic.so'}),
    ({}, {'DYLD_INSERT_LIBRARIES': '/synthetic.dylib'}),
    ({}, {'CARGO_ENV_PATH': '/synthetic'}),
]:
    try:
        module.validate(config, env)
    except ValueError:
        continue
    raise SystemExit(f'error: Cargo override accepted: {config} {env}')
for config in ({'env': {'RUSTC_WRAPPER': 'synthetic'}}, {'env': {'PATH': {'value': '/synthetic', 'force': True}}}):
    try:
        module.validate(config, {})
    except ValueError as error:
        assert '[env]' in str(error), str(error)
        continue
    raise SystemExit('error: Cargo [env] accepted')
print('[ok] Cargo source and command override controls')
PY

scratch="$(cd "$(mktemp -d)" && pwd -P)"
trap 'rm -rf "$scratch"' EXIT
check() { "$root/scripts/check-registry-config.py" "$@" >"$scratch/output" 2>&1; }
expect_refused() {
	if check "$@"; then
		echo "error: expected refusal: $*" >&2
		cat "$scratch/output" >&2
		exit 1
	fi
}
mkdir "$scratch/home"
printf 'paths=["synthetic"]\n' >"$scratch/home/config.toml"
CARGO_HOME="$scratch/home" expect_refused
printf '[build]\nrustc-wrapper = "/synthetic/wrapper"\n' >"$scratch/home/config.toml"
CARGO_HOME="$scratch/home" expect_refused
CARGO_HOME=relative expect_refused

# A tagged source tree: any Cargo configuration file or a build script in crates/* refuses.
source_tree="$scratch/source"
mkdir -p "$source_tree/crates/storageprims-core" "$scratch/clean-home"
cp "$root/Cargo.toml" "$source_tree/"
printf '[package]\nname = "storageprims-core"\n' >"$source_tree/crates/storageprims-core/Cargo.toml"
CARGO_HOME="$scratch/clean-home" check --source-root "$source_tree"
mkdir -p "$source_tree/.cargo"
printf '[net]\nretry = 1\n' >"$source_tree/.cargo/config.toml"
CARGO_HOME="$scratch/clean-home" expect_refused --source-root "$source_tree"
grep -q 'may not carry Cargo configuration' "$scratch/output"
rm "$source_tree/.cargo/config.toml"
mkdir -p "$source_tree/crates/storageprims-core/.cargo"
printf '[build]\nrustc-wrapper = "scripts/wrapper"\n' >"$source_tree/crates/storageprims-core/.cargo/config"
CARGO_HOME="$scratch/clean-home" expect_refused --source-root "$source_tree"
rm -r "$source_tree/crates/storageprims-core/.cargo"
printf 'fn main() {}\n' >"$source_tree/crates/storageprims-core/build.rs"
CARGO_HOME="$scratch/clean-home" expect_refused --source-root "$source_tree"
grep -q 'may not have a build script' "$scratch/output"
rm "$source_tree/crates/storageprims-core/build.rs"
printf '[package]\nname = "storageprims-core"\nbuild = "tools/gen.rs"\n' >"$source_tree/crates/storageprims-core/Cargo.toml"
CARGO_HOME="$scratch/clean-home" expect_refused --source-root "$source_tree"
# A Cargo configuration in an ancestor of the release source (a planted
# shared temporary directory) is refused, even with only "safe" tables.
nested="$scratch/shared/source"
mkdir -p "$scratch/shared/.cargo" "$nested/crates/storageprims-core"
cp "$source_tree/Cargo.toml" "$nested/"
printf '[package]\nname = "storageprims-core"\n' >"$nested/crates/storageprims-core/Cargo.toml"
CARGO_HOME="$scratch/clean-home" check --source-root "$nested"
printf '[http]\nproxy = "http://127.0.0.1:9"\n' >"$scratch/shared/.cargo/config.toml"
CARGO_HOME="$scratch/clean-home" expect_refused --source-root "$nested"
grep -q 'ancestor of the release source' "$scratch/output"
# A Cargo home inside the release source is refused.
CARGO_HOME="$source_tree/home" expect_refused --source-root "$source_tree"
# The environment is checked in every mode.
RUSTC_WRAPPER=/synthetic CARGO_HOME="$scratch/clean-home" expect_refused
echo '[ok] Cargo configuration file and release-source controls'
