#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# The sparse-index reader: listed (0), absent (1), yanked (3), explicit
# User-Agent header, scheme/redirect/size/deadline/truncation refusal (2),
# and malformed rows (2) never read as absent.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd -P)"
scratch="$(mktemp -d)"
python3 -u - "$scratch" >"$scratch/server.log" 2>&1 <<'PY' &
import http.server
import json
import os
import sys
import time

root = sys.argv[1]
ua_log = os.path.join(root, "ua.log")


class Handler(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=root, **kwargs)

    def do_GET(self):
        with open(ua_log, "a", encoding="utf-8") as handle:
            handle.write(json.dumps(self.headers.get("User-Agent")) + "\n")
        if self.path.endswith("/trunc"):
            self.send_response(200)
            self.send_header("Content-Type", "text/plain")
            self.send_header("Content-Length", "4096")
            self.end_headers()
            self.wfile.write(b'{"name":"trunc","vers":')
            self.wfile.flush()
            self.close_connection = True
            return
        if self.path.endswith("/slow"):
            self.send_response(200)
            self.send_header("Content-Type", "text/plain")
            self.end_headers()
            for _ in range(40):
                try:
                    self.wfile.write(b" ")
                    self.wfile.flush()
                except OSError:
                    return
                time.sleep(0.05)
            return
        if self.path.endswith("/slowhdr"):
            self.wfile.write(b"HTTP/1.1 200 OK\r\n")
            self.wfile.flush()
            for idx in range(40):
                try:
                    self.wfile.write(f"X-Slow-{idx}: 1\r\n".encode("ascii"))
                    self.wfile.flush()
                except OSError:
                    return
                time.sleep(0.05)
            return
        if self.path.endswith("/slowchunk"):
            self.wfile.write(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nTransfer-Encoding: chunked\r\n\r\n"
            )
            self.wfile.flush()
            for ch in b"100000":
                try:
                    self.wfile.write(bytes([ch]))
                    self.wfile.flush()
                except OSError:
                    return
                time.sleep(0.1)
            time.sleep(2.0)
            return
        if self.path.endswith("/redirfile"):
            self.send_response(302)
            self.send_header("Location", "file:///etc/hosts")
            self.end_headers()
            return
        return super().do_GET()

    def log_message(self, fmt, *args):
        pass


server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
print(f"Serving HTTP on 127.0.0.1 port {server.server_address[1]} ...", flush=True)
server.serve_forever()
PY
server=$!
trap 'kill "$server" 2>/dev/null || true; rm -rf "$scratch"' EXIT
for _ in $(seq 1 50); do
	grep -q 'Serving HTTP' "$scratch/server.log" && break
	sleep 0.1
done
[[ -n "$(sed -n "s/.*port \([0-9]*\).*/\1/p" "$scratch/server.log")" ]] || {
	echo "error: test server did not start" >&2
	exit 1
}
port="$(sed -n 's/.*port \([0-9]*\).*/\1/p' "$scratch/server.log" | head -1)"
base="sparse+http://127.0.0.1:$port/"
mkdir -p "$scratch/ab/cd" "$scratch/3/a" "$scratch/2" "$scratch/1" "$scratch/tr/un" "$scratch/sl/ow" "$scratch/re/di"
expect() {
	local want="$1" name="$2" body="$3" got=0
	printf '%s\n' "$body" >"$scratch/ab/cd/abcdef"
	python3 -I "$root/scripts/release-index-entry.py" --allow-http "$base" "$name" 1.0.0 >"$scratch/out" 2>/dev/null || got=$?
	[[ "$got" == "$want" ]] || {
		echo "error: $body -> exit $got, expected $want" >&2
		exit 1
	}
}
digest="$(printf '%064d' 7)"
expect 0 abcdef "{\"name\":\"abcdef\",\"vers\":\"1.0.0\",\"cksum\":\"$digest\"}"
[[ "$(cat "$scratch/out")" == "$digest" ]]
grep -q 'storageprims-release-verify/1\.0' "$scratch/ua.log" || {
	echo "error: User-Agent header missing from sparse-index request" >&2
	exit 1
}
expect 0 ABCDEF "{\"name\":\"abcdef\",\"vers\":\"1.0.0\",\"cksum\":\"$digest\"}"
expect 1 abcdef "{\"name\":\"abcdef\",\"vers\":\"0.9.0\",\"cksum\":\"$digest\"}"
expect 3 abcdef "{\"name\":\"abcdef\",\"vers\":\"1.0.0\",\"cksum\":\"$digest\",\"yanked\":true}"
expect 1 abcdefg "{}"
# The last line for a version wins, as in Cargo: a later yank or edit applies.
live="{\"name\":\"abcdef\",\"vers\":\"1.0.0\",\"cksum\":\"$digest\"}"
other="$(printf '%064d' 9)"
expect 3 abcdef "$live
{\"name\":\"abcdef\",\"vers\":\"1.0.0\",\"cksum\":\"$digest\",\"yanked\":true}"
expect 0 abcdef "$live
{\"name\":\"abcdef\",\"vers\":\"1.0.0\",\"cksum\":\"$other\"}"
[[ "$(cat "$scratch/out")" == "$other" ]]
for body in '[]' 'null' '7' '{"name":5,"vers":"1.0.0"}' '{"name":"abcdef","vers":1}' \
	'{"name":"abcdef","vers":"1.0.0","cksum":7}' '{"name":"abcdef","vers":"1.0.0"}' 'not json'; do
	expect 2 abcdef "$body"
done
# Short-name index paths.
printf '{"name":"abc","vers":"1.0.0","cksum":"%s"}\n' "$digest" >"$scratch/3/a/abc"
printf '{"name":"ab","vers":"1.0.0","cksum":"%s"}\n' "$digest" >"$scratch/2/ab"
printf '{"name":"a","vers":"1.0.0","cksum":"%s"}\n' "$digest" >"$scratch/1/a"
for name in a ab abc; do
	python3 -I "$root/scripts/release-index-entry.py" --allow-http "$base" "$name" 1.0.0 >/dev/null
done

expect_exit2() {
	local rc=0
	python3 -I "$root/scripts/release-index-entry.py" "$@" >/dev/null 2>&1 || rc=$?
	[[ "$rc" == 2 ]] || {
		echo "error: args ($*) exited $rc, expected 2" >&2
		exit 1
	}
}

# Plain HTTP without --allow-http, non-HTTP schemes, and unsafe crate names exit 2.
expect_exit2 "$base" abcdef 1.0.0
expect_exit2 "file:///etc/" abcdef 1.0.0
expect_exit2 --allow-http "file:///etc/" abcdef 1.0.0
expect_exit2 "ftp://127.0.0.1/" abcdef 1.0.0
expect_exit2 --allow-http "ftp://127.0.0.1/" abcdef 1.0.0
expect_exit2 --allow-http "$base" "../etc/passwd" 1.0.0
expect_exit2 --allow-http "$base" redirfile 1.0.0

# HTTPS-to-HTTP redirect downgrade is refused by _SafeRedirectHandler.
python3 -I -B - "$root/scripts/release-index-entry.py" <<'PY'
import importlib.util, sys, urllib.error, urllib.request
spec = importlib.util.spec_from_file_location("rie", sys.argv[1])
rie = importlib.util.module_from_spec(spec)
spec.loader.exec_module(rie)
handler = rie._SafeRedirectHandler(allow_http=True)
req = urllib.request.Request("https://index.crates.io/ab/cd/abcdef")
try:
    handler.redirect_request(req, None, 302, "Found", {}, "http://127.0.0.1/ab/cd/abcdef")
except urllib.error.URLError as err:
    assert "HTTPS-to-HTTP" in str(err), err
else:
    raise SystemExit("expected HTTPS-to-HTTP redirect refusal")
PY

# Truncated HTTP 200 response (Content-Length 4096, 23 bytes delivered) exits 2, never 1.
expect_exit2 --allow-http "$base" trunc 1.0.0

# Deeply nested JSON (RecursionError) exits 2, never 1.
python3 -c 'print("[" * 10000 + "]" * 10000)' >"$scratch/ab/cd/abcdef"
expect_exit2 --allow-http "$base" abcdef 1.0.0

# Oversized total response, oversized single line, and overall deadline all exit 2.
printf '%s\n%s\n' "$live" "$live" >"$scratch/ab/cd/abcdef"
expect_exit2 --allow-http --max-bytes 64 "$base" abcdef 1.0.0
expect_exit2 --allow-http --max-line-bytes 32 "$base" abcdef 1.0.0

# Trickle body, trickle headers, and stalled chunk-size framing must all abort
# near --timeout without blocking in BufferedReader.close().
for spec in "slow:0.2:0.8" "slowhdr:0.2:0.8" "slowchunk:0.6:0.95"; do
	slow_crate="${spec%%:*}"
	rem="${spec#*:}"
	tmo="${rem%%:*}"
	cap="${rem#*:}"
	python3 -I -B - "$root/scripts/release-index-entry.py" "$base" "$slow_crate" "$tmo" "$cap" <<'PY'
import subprocess, sys, time
script, base, crate, tmo, cap = sys.argv[1:6]
t0 = time.monotonic()
proc = subprocess.run(
    [sys.executable, "-I", script, "--allow-http", "--timeout", tmo, base, crate, "1.0.0"],
    capture_output=True,
    text=True,
)
elapsed = time.monotonic() - t0
assert proc.returncode == 2, (crate, proc.returncode, proc.stderr)
assert elapsed < float(cap), f"{crate} took {elapsed:.3f}s despite --timeout {tmo} (cap {cap}s)"
PY
done

# No server: an error, not absent.
expect_exit2 --allow-http "http://127.0.0.1:9/" abcdef 1.0.0
echo '[ok] sparse-index reader: listed, absent, yanked, User-Agent, scheme/size/deadline/truncation and malformed rows'
