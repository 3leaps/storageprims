#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Cargo credential-provider and zero-compile crate publication
# controls against a local sparse test registry.
set -euo pipefail
# shellcheck source=scripts/release-registry-fixture.sh
source "$(dirname "$0")/release-registry-fixture.sh"
export PYTHONDONTWRITEBYTECODE=1
unset CI CARGO_REGISTRY_TOKEN CARGO_REGISTRIES_CRATES_IO_TOKEN \
	RUSTFLAGS RUSTDOCFLAGS CARGO_ENCODED_RUSTFLAGS CARGO_ENCODED_RUSTDOCFLAGS

msrv="$(sed -n 's/^rust-version = "\(.*\)"/\1/p' "$fixture_root/Cargo.toml" | head -n 1)"
if ! command -v cargo >/dev/null; then
	echo "error: this test needs cargo" >&2
	exit 1
fi
if [[ -n "$msrv" ]] && ! compgen -G "${RUSTUP_HOME:-$HOME/.rustup}/toolchains/${msrv}*" >/dev/null; then
	msrv=""
fi

prov="$fixture_root/scripts/release-cargo-provider.py"
expect_fail_with 'run by Cargo as a credential provider' python3 -I "$prov"

version=1.2.3
tag="v$version"
digest="$(printf 'a%.0s' $(seq 64))"
printf 'dummy-token\n' >"$fixture_dir/token"
printf '{"index-url":"https://github.com/rust-lang/crates.io-index","name":"storageprims-core","vers":"%s","cksum":"%s"}' \
	"$version" "$digest" >"$fixture_dir/approval.json"

ask() {
	printf '%s\n' "$1" | STORAGEPRIMS_CARGO_APPROVAL="$fixture_dir/approval.json" \
		STORAGEPRIMS_CARGO_TOKEN_FILE="$fixture_dir/token" \
		STORAGEPRIMS_CARGO_PROVIDER_LOG="$fixture_dir/unit-provider.log" \
		python3 -I "$prov" --cargo-plugin | tail -1
}

reg='{"index-url":"https://github.com/rust-lang/crates.io-index","name":"crates-io"}'
[[ "$(ask "{\"v\":1,\"registry\":$reg,\"kind\":\"get\",\"operation\":\"read\"}")" == *'"token": "storageprims-read-placeholder-not-a-credential"'* ]]
[[ "$(ask "{\"v\":1,\"registry\":$reg,\"kind\":\"get\",\"operation\":\"publish\",\"name\":\"storageprims-core\",\"vers\":\"$version\",\"cksum\":\"$digest\"}")" == *'"token": "dummy-token"'* ]]
for request in \
	"{\"v\":1,\"registry\":$reg,\"kind\":\"get\",\"operation\":\"publish\",\"name\":\"storageprims-core\",\"vers\":\"$version\",\"cksum\":\"$(printf '%064d' 0)\"}" \
	"{\"v\":1,\"registry\":$reg,\"kind\":\"get\",\"operation\":\"publish\",\"name\":\"storageprims-signal\",\"vers\":\"$version\",\"cksum\":\"$digest\"}" \
	"{\"v\":1,\"registry\":$reg,\"kind\":\"get\",\"operation\":\"publish\",\"name\":\"storageprims-core\",\"vers\":\"9.9.9\",\"cksum\":\"$digest\"}" \
	"{\"v\":1,\"registry\":{\"index-url\":\"https://example.invalid/index\"},\"kind\":\"get\",\"operation\":\"publish\",\"name\":\"storageprims-core\",\"vers\":\"$version\",\"cksum\":\"$digest\"}" \
	"{\"v\":1,\"registry\":$reg,\"kind\":\"get\",\"operation\":\"yank\",\"name\":\"storageprims-core\",\"vers\":\"$version\"}" \
	"{\"v\":1,\"registry\":$reg,\"kind\":\"login\"}"; do
	[[ "$(ask "$request")" == *'"Err"'* ]] || {
		echo "error: provider released a token for $request" >&2
		exit 1
	}
done
if grep -q 'dummy-token' "$fixture_dir/unit-provider.log"; then
	echo 'error: provider logged the registry token' >&2
	exit 1
fi

printf '{"index-url":"x","name":"storageprims-core","vers":"1.2.3","cksum":"bad"}' >"$fixture_dir/bad-approval.json"
expect_fail_with 'malformed approval' env STORAGEPRIMS_CARGO_APPROVAL="$fixture_dir/bad-approval.json" \
	STORAGEPRIMS_CARGO_TOKEN_FILE="$fixture_dir/token" python3 -I "$prov" --cargo-plugin </dev/null
: >"$fixture_dir/empty-token"
expect_fail_with 'empty token' bash -c "printf '%s\n' '{\"v\":1,\"registry\":$reg,\"kind\":\"get\",\"operation\":\"publish\",\"name\":\"storageprims-core\",\"vers\":\"$version\",\"cksum\":\"$digest\"}' | STORAGEPRIMS_CARGO_APPROVAL='$fixture_dir/approval.json' STORAGEPRIMS_CARGO_TOKEN_FILE='$fixture_dir/empty-token' python3 -I '$prov' --cargo-plugin"

# A test endpoint must not alias a production registry or accept real credentials.
expect_fail_with 'test registry endpoints must be loopback' bash "$fixture_root/scripts/release-crates-publish.sh" --publish \
	--registry-index sparse+https://index.crates.io/ --registry-api http://127.0.0.1:12345 --registry-dl http://127.0.0.1:12345
expect_fail_with 'test registry requires index' bash "$fixture_root/scripts/release-crates-publish.sh" --publish --registry-dl http://127.0.0.1:12345

# End-to-end integration with real Cargo against the local sparse registry.
repo="$fixture_dir/repo"
fixture_repo "$repo" "$version" publish 0
cp "$fixture_root"/scripts/{release-crates-publish.sh,release-crates.py,release-index-entry.py,release-export-tree.py,release-cargo-provider.py,check-registry-config.py,write-exclusive.py} "$repo/scripts/"
printf 'storageprims-core\n' >"$repo/config/release/publishable-crates.txt"
mkdir -p "$repo/crates/storageprims-core/src"
cat >"$repo/Cargo.toml" <<'TOML'
[workspace]
members = ["crates/storageprims-core"]
resolver = "2"

[workspace.package]
version = "1.2.3"
edition = "2021"
rust-version = "1.94.1"
license = "MIT OR Apache-2.0"
publish = false
TOML
cat >"$repo/crates/storageprims-core/Cargo.toml" <<'TOML'
[package]
name = "storageprims-core"
version.workspace = true
edition.workspace = true
license.workspace = true
rust-version.workspace = true
publish = true
description = "Synthetic storageprims-core fixture"
readme = "README.md"
TOML
printf '# storageprims-core\n' >"$repo/crates/storageprims-core/README.md"
printf 'pub fn core_ready() -> bool { true }\n' >"$repo/crates/storageprims-core/src/lib.rs"
printf 'target/\n' >"$repo/.gitignore"
(cd "$repo" && cargo generate-lockfile --offline -q)

# Poisoned cargo in the repo root, the repo's bin/ directory, and ../outside:
# relative PATH entries and absolute PATH entries inside $root must be stripped.
mkdir -p "$repo/bin" "$fixture_dir/outside" "$fixture_dir/cargo-wrap"
printf '#!/usr/bin/env bash\ntouch "%s/poisoned"\nexit 0\n' "$fixture_dir" >"$repo/cargo"
printf '#!/usr/bin/env bash\ntouch "%s/poisoned"\nexit 0\n' "$fixture_dir" >"$repo/bin/cargo"
printf '#!/usr/bin/env bash\ntouch "%s/poisoned"\nexit 0\n' "$fixture_dir" >"$fixture_dir/outside/cargo"
chmod +x "$repo/cargo" "$repo/bin/cargo" "$fixture_dir/outside/cargo"
real_cargo="$(command -v cargo)"
cat >"$fixture_dir/cargo-wrap/cargo" <<EOF
#!/usr/bin/env bash
if [[ "\${1:-}" == publish ]]; then
	if [[ -n "\${STORAGEPRIMS_CARGO_TOKEN_FILE:-}\${STORAGEPRIMS_CARGO_APPROVAL:-}\${STORAGEPRIMS_CARGO_PROVIDER_LOG:-}" ]]; then
		echo "error: cargo publish inherited provider secret/approval env vars" >&2
		exit 1
	fi
	if [[ -f $(printf '%q' "$fixture_dir/dirty-during-publish") ]]; then
		touch "\$PWD/dirty-during-publish"
		exit 0
	fi
fi
exec $(printf '%q' "$real_cargo") "\$@"
EOF
chmod +x "$fixture_dir/cargo-wrap/cargo"
printf 'target/\n/cargo\n' >"$repo/.gitignore"

fixture_commit "$repo" release
git -C "$repo" tag -a -m "Release $version" "$tag"
git -C "$repo" checkout -q --detach "$tag"
commit="$(git -C "$repo" rev-parse HEAD)"

# Compute the exact .crate digest produced by real cargo package --no-verify --locked.
pkg_check="$fixture_dir/pkg-check"
python3 "$repo/scripts/release-export-tree.py" "$repo" "$commit" "$pkg_check/tree"
(cd "$pkg_check/tree" && env -i PATH="$PATH" HOME="$pkg_check/home" CARGO_HOME="$pkg_check/home" \
	CARGO_TARGET_DIR="$pkg_check/target" RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" \
	${msrv:+RUSTUP_TOOLCHAIN="$msrv"} cargo package --locked --no-verify -p storageprims-core >/dev/null 2>&1)
expected_crate="$pkg_check/target/package/storageprims-core-$version.crate"
digest="$(shasum -a 256 "$expected_crate" | cut -c1-64)"

registry="$fixture_dir/registry"
python3 "$fixture_root/scripts/test-registry.py" "$registry" 0 dummy-token >/dev/null 2>&1 &
registry_pid=$!
trap 'kill "$registry_pid" 2>/dev/null || true; fixture_cleanup' EXIT
for _ in $(seq 1 50); do
	[[ -f "$registry/ready" ]] && break
	sleep 0.1
done
port="$(cat "$registry/ready")"
index="sparse+http://127.0.0.1:$port/index/"
api="http://127.0.0.1:$port"
dl="http://127.0.0.1:$port/dl"

export STORAGEPRIMS_RELEASE_TAG="$tag"
export STORAGEPRIMS_CARGO_TOKEN_FILE="$fixture_dir/token"
export STORAGEPRIMS_INDEX_WAIT_TRIES=20 STORAGEPRIMS_INDEX_WAIT_SECONDS=0.2

publish_tty() {
	local answer="$1"
	shift
	python3 - "$answer" "$repo" "$fixture_dir/cargo-wrap" "$index" "$api" "$dl" "$@" <<'PY'
import fcntl, os, pty, select, subprocess, sys, termios

answer, repo, wrap, index, api, dl, *extra = sys.argv[1:]
master, slave = pty.openpty()
env = os.environ.copy()
env["PATH"] = f"::bin:../outside:{repo}:{repo}/bin:{wrap}:" + env["PATH"]
for item in extra:
    if item.startswith("-u:"):
        env.pop(item[3:], None)
    else:
        k, _, v = item.partition("=")
        env[k] = v

def preexec():
    os.setsid()
    fcntl.ioctl(slave, termios.TIOCSCTTY, 0)

proc = subprocess.Popen(
    [f"{repo}/scripts/release-crates-publish.sh", "--publish", "--registry-index", index, "--registry-api", api, "--registry-dl", dl],
    cwd=repo,
    env=env,
    stdin=slave,
    stdout=slave,
    stderr=slave,
    preexec_fn=preexec,
    close_fds=True,
)
os.close(slave)
sent = False
chunks = []
while True:
    r, _, _ = select.select([master], [], [], 0.1)
    if r:
        try:
            data = os.read(master, 4096)
        except OSError:
            break
        if not data:
            break
        chunks.append(data)
        joined = b"".join(chunks)
        if not sent and b"Type the crate name to continue:" in joined:
            if answer != "__EOF__":
                os.write(master, (answer + "\n").encode("utf-8"))
            sent = True
    elif proc.poll() is not None:
        break
os.close(master)
rc = proc.wait()
sys.stdout.buffer.write(b"".join(chunks))
sys.exit(rc)
PY
}

publish_no_tty() {
	python3 - "$repo" "$fixture_dir/cargo-wrap" "$index" "$api" "$dl" <<'PY'
import os, subprocess, sys
repo, wrap, index, api, dl = sys.argv[1:]
env = os.environ.copy()
env["PATH"] = f"::bin:../outside:{repo}:{repo}/bin:{wrap}:" + env["PATH"]
proc = subprocess.run(
    [f"{repo}/scripts/release-crates-publish.sh", "--publish", "--registry-index", index, "--registry-api", api, "--registry-dl", dl],
    cwd=repo,
    env=env,
    input=b"storageprims-core\n",
    stdout=subprocess.PIPE,
    stderr=subprocess.STDOUT,
    start_new_session=True,
)
sys.stdout.buffer.write(proc.stdout)
sys.exit(proc.returncode)
PY
}
puts() { grep -c '"m": "PUT"' "$registry/requests.log" 2>/dev/null || true; }

# 1. With real Cargo: a mismatched approval checksum gets no token and uploads nothing.
gate_home="$fixture_dir/gate-home"
mkdir -p "$gate_home"
printf '[registries.testreg]\nindex = "%s"\n[registry]\nglobal-credential-providers = ["%s"]\n' \
	"$index" "$prov" >"$gate_home/config.toml"
printf '{"index-url":"%s","name":"storageprims-core","vers":"%s","cksum":"%s"}' \
	"$index" "$version" "$(printf '%064d' 0)" >"$fixture_dir/wrong-approval.json"
if (cd "$pkg_check/tree" && env -i PATH="$PATH" HOME="$gate_home" CARGO_HOME="$gate_home" \
	CARGO_TARGET_DIR="$pkg_check/gate-target" RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" \
	${msrv:+RUSTUP_TOOLCHAIN="$msrv"} STORAGEPRIMS_CARGO_APPROVAL="$fixture_dir/wrong-approval.json" \
	STORAGEPRIMS_CARGO_TOKEN_FILE="$fixture_dir/token" \
	cargo publish --no-verify --locked --registry testreg -p storageprims-core) >"$fixture_dir/gate.out" 2>&1; then
	echo 'error: cargo published with a mismatched approval' >&2
	exit 1
fi
grep -q 'is not approved' "$fixture_dir/gate.out" || {
	cat "$fixture_dir/gate.out" >&2
	exit 1
}
[[ "$(puts)" == 0 ]] || {
	echo 'error: an upload reached the registry without approval' >&2
	exit 1
}

# 2. Direct environment tokens, missing token file, non-tty or wrong tty answer,
# ancestor .cargo/config.toml, and
# packaging-dirtied trees are all refused through the script.
expect_fail_with 'unset CARGO_REGISTRY_TOKEN' publish_tty storageprims-core CARGO_REGISTRY_TOKEN=x
expect_fail_with 'Cargo source or command environment overrides are forbidden' publish_tty storageprims-core RUSTFLAGS=-Dwarnings
expect_fail_with 'STORAGEPRIMS_CARGO_TOKEN_FILE must name a file' publish_tty storageprims-core -u:STORAGEPRIMS_CARGO_TOKEN_FILE
printf 'synthetic-other-token\n' >"$fixture_dir/other-token"
expect_fail_with 'test registry requires the synthetic dummy-token' publish_tty storageprims-core STORAGEPRIMS_CARGO_TOKEN_FILE="$fixture_dir/other-token"
expect_fail_with 'not confirmed' publish_no_tty
expect_fail_with 'not confirmed' publish_tty wrong-crate
mkdir -p "$fixture_dir/.cargo"
printf '[registry]\nglobal-credential-providers = ["cargo:token"]\n' >"$fixture_dir/.cargo/config.toml"
expect_fail_with 'Cargo registry configuration may not name an index or command' publish_tty storageprims-core
rm -rf "$fixture_dir/.cargo"
mkdir -p "$fixture_dir/shared-tmp/.cargo"
printf '[net]\nretry = 1\n' >"$fixture_dir/shared-tmp/.cargo/config.toml"
expect_fail_with 'Cargo configuration in an ancestor of the release source is forbidden' publish_tty storageprims-core TMPDIR="$fixture_dir/shared-tmp"
rm -rf "$fixture_dir/shared-tmp"
touch "$fixture_dir/dirty-during-publish"
expect_fail_with 'packaging changed the release source' publish_tty storageprims-core
rm -f "$fixture_dir/dirty-during-publish"

# Inherited Python startup hooks must never execute in the production helper
# chain, either before refused approval or through an approved publication.
mkdir -p "$fixture_dir/python-startup"
cat >"$fixture_dir/python-startup/sitecustomize.py" <<'PYHOOK'
import os
from pathlib import Path
Path(os.environ["STORAGEPRIMS_TEST_HOOK_MARKER"]).write_text(
    Path(os.environ["STORAGEPRIMS_CARGO_TOKEN_FILE"]).read_text()
)
PYHOOK
expect_fail_with 'not confirmed' publish_tty wrong-crate \
	PYTHONPATH="$fixture_dir/python-startup" STORAGEPRIMS_TEST_HOOK_MARKER="$fixture_dir/python-hook-ran"
[[ ! -e "$fixture_dir/python-hook-ran" ]]
[[ "$(puts)" == 0 ]]

# 3. Real publication through the release provider.
export STORAGEPRIMS_CARGO_PROVIDER_LOG="$fixture_dir/provider.log"
publish_tty storageprims-core PYTHONPATH="$fixture_dir/python-startup" \
	STORAGEPRIMS_TEST_HOOK_MARKER="$fixture_dir/python-hook-ran" >"$fixture_dir/out" 2>&1 || {
	cat "$fixture_dir/out" >&2
	exit 1
}
[[ ! -e "$fixture_dir/python-hook-ran" ]]
grep -q "storageprims-core $version published and verified ($digest)" "$fixture_dir/out"
[[ "$(puts)" -ge 1 ]]
python3 - "$fixture_dir/provider.log" "$digest" <<'PY'
import json, sys
records = [json.loads(line) for line in open(sys.argv[1], encoding="utf-8")]
ops = [(r["operation"], r["decision"]) for r in records]
assert ops == [("read", "placeholder"), ("publish", "released")], ops
assert records[1]["cksum"] == sys.argv[2], records[1]
PY
python3 - "$registry/requests.log" <<'PY'
import json, sys
for line in open(sys.argv[1], encoding="utf-8"):
    r = json.loads(line)
    if r["m"] == "GET":
        assert r["auth"] in (None, ""), r
    if r["m"] == "PUT":
        assert r["auth"] == "dummy-token", r
PY
cmp "$registry/crates/storageprims-core-$version.crate" "$expected_crate"
[[ ! -e "$fixture_dir/poisoned" ]] || {
	echo 'error: relative or checkout-internal PATH entry executed a binary inside the checkout' >&2
	exit 1
}

# 4. Re-running when already published with identical contents skips without uploading again.
puts_before="$(puts)"
out="$(publish_tty storageprims-core)"
[[ "$out" == *"already on crates.io with these contents; skipping"* ]]
[[ "$(puts)" == "$puts_before" ]]

# 5. Mismatched index checksum after upload is refused.
kill "$registry_pid" 2>/dev/null || true
wait "$registry_pid" 2>/dev/null || true
registry_idx="$fixture_dir/registry-idx"
mkdir -p "$registry_idx"
touch "$registry_idx/wrong-index-cksum"
python3 "$fixture_root/scripts/test-registry.py" "$registry_idx" 0 dummy-token >/dev/null 2>&1 &
registry_pid=$!
for _ in $(seq 1 50); do
	[[ -f "$registry_idx/ready" ]] && break
	sleep 0.1
done
port="$(cat "$registry_idx/ready")"
index="sparse+http://127.0.0.1:$port/index/"
api="http://127.0.0.1:$port"
dl="http://127.0.0.1:$port/dl"
expect_fail_with 'the index checksum for storageprims-core differs from the verified archive' publish_tty storageprims-core

# 6. Corrupted download from the registry is refused after upload.
kill "$registry_pid" 2>/dev/null || true
wait "$registry_pid" 2>/dev/null || true
registry2="$fixture_dir/registry2"
mkdir -p "$registry2"
touch "$registry2/corrupt-downloads"
python3 "$fixture_root/scripts/test-registry.py" "$registry2" 0 dummy-token >/dev/null 2>&1 &
registry_pid=$!
for _ in $(seq 1 50); do
	[[ -f "$registry2/ready" ]] && break
	sleep 0.1
done
port="$(cat "$registry2/ready")"
index="sparse+http://127.0.0.1:$port/index/"
api="http://127.0.0.1:$port"
dl="http://127.0.0.1:$port/dl"
expect_fail_with 'published storageprims-core archive differs from the verified one' publish_tty storageprims-core

# 7. Yanked version on the registry is refused before any provider call or upload.
kill "$registry_pid" 2>/dev/null || true
wait "$registry_pid" 2>/dev/null || true
registry3="$fixture_dir/registry3"
mkdir -p "$registry3"
printf 'storageprims-core {"name":"storageprims-core","vers":"%s","deps":[],"cksum":"%s","features":{},"yanked":true}\n' \
	"$version" "$digest" >"$registry3/seed"
python3 "$fixture_root/scripts/test-registry.py" "$registry3" 0 dummy-token >/dev/null 2>&1 &
registry_pid=$!
for _ in $(seq 1 50); do
	[[ -f "$registry3/ready" ]] && break
	sleep 0.1
done
port="$(cat "$registry3/ready")"
index="sparse+http://127.0.0.1:$port/index/"
api="http://127.0.0.1:$port"
dl="http://127.0.0.1:$port/dl"
rm -f "$fixture_dir/yanked-provider.log"
export STORAGEPRIMS_CARGO_PROVIDER_LOG="$fixture_dir/yanked-provider.log"
expect_fail_with "storageprims-core@$version is yanked on crates.io" publish_tty storageprims-core
[[ ! -e "$fixture_dir/yanked-provider.log" ]] || {
	echo 'error: a yanked version reached the credential provider' >&2
	exit 1
}

echo '[ok] Cargo credential-provider and zero-compile publication controls passed'
