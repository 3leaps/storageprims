#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Minimal sparse-registry stand-in for crates.io, for ceremony tests only.
Serves config.json, sparse index files, /api/v1/crates/<crate>/<version>, and
/dl/<crate>/<crate>-<version>.crate; accepts PUT /api/v1/crates/new, stores the
uploaded .crate and appends an index line. Logs every request."""

import hashlib
import http.server
import json
import os
import struct
import sys

ROOT = sys.argv[1]
PORT = int(sys.argv[2])
TOKEN = sys.argv[3]
os.makedirs(f"{ROOT}/index", exist_ok=True)
os.makedirs(f"{ROOT}/crates", exist_ok=True)


def ipath(name):
    n = name.lower()
    if len(n) <= 2:
        return f"{len(n)}/{n}"
    if len(n) == 3:
        return f"3/{n[0]}/{n}"
    return f"{n[:2]}/{n[2:4]}/{n}"


def find_index_entry(crate, version):
    p = f"{ROOT}/index/{ipath(crate)}"
    if not os.path.exists(p):
        return None
    found = None
    with open(p, encoding="utf-8") as handle:
        for line in handle:
            if not line.strip():
                continue
            entry = json.loads(line)
            if (
                entry.get("name", "").lower() == crate.lower()
                and entry.get("vers") == version
            ):
                found = entry
    return found


class H(http.server.BaseHTTPRequestHandler):
    def log(self, extra=""):
        with open(f"{ROOT}/requests.log", "a", encoding="utf-8") as f:
            f.write(
                json.dumps(
                    {
                        "m": self.command,
                        "p": self.path,
                        "auth": self.headers.get("Authorization"),
                        **({"x": extra} if extra else {}),
                    }
                )
                + "\n"
            )

    def do_GET(self):
        self.log()
        if self.path == "/index/config.json":
            body = json.dumps(
                {
                    "dl": f"http://127.0.0.1:{BOUND}/dl/{{crate}}/{{crate}}-{{version}}.crate",
                    "api": f"http://127.0.0.1:{BOUND}",
                }
            ).encode()
            return self.send(200, body)
        if self.path.startswith("/index/"):
            f = f"{ROOT}/index/{self.path[7:]}"
            return (
                self.send(200, open(f, "rb").read())
                if os.path.exists(f)
                else self.send(404, b"")
            )
        if self.path.startswith("/api/v1/crates/"):
            parts = self.path.split("/")
            if len(parts) == 6:
                crate, version = parts[4], parts[5]
                entry = find_index_entry(crate, version)
                if entry is None:
                    return self.send(404, b"{}")
                body = json.dumps(
                    {
                        "version": {
                            "crate": entry["name"],
                            "num": entry["vers"],
                            "checksum": entry["cksum"],
                            "yanked": bool(entry.get("yanked", False)),
                        }
                    }
                ).encode()
                return self.send(200, body)
            return self.send(404, b"{}")
        if self.path.startswith("/dl/"):
            # /dl/<crate>/<crate>-<version>.crate, the crates.io static layout
            parts = self.path.split("/")
            if len(parts) != 4:
                return self.send(404, b"")
            f = f"{ROOT}/crates/{parts[3]}"
            if os.path.exists(f"{ROOT}/corrupt-downloads") and os.path.exists(f):
                return self.send(200, open(f, "rb").read() + b"corrupted")
            return (
                self.send(200, open(f, "rb").read())
                if os.path.exists(f)
                else self.send(404, b"")
            )
        self.send(404, b"")

    def do_PUT(self):
        n = int(self.headers.get("Content-Length", 0))
        data = self.rfile.read(n)
        self.log()
        if self.headers.get("Authorization") != TOKEN:
            return self.send(
                403, json.dumps({"errors": [{"detail": "bad token"}]}).encode()
            )
        jl = struct.unpack("<I", data[:4])[0]
        meta = json.loads(data[4 : 4 + jl])
        cl = struct.unpack("<I", data[4 + jl : 8 + jl])[0]
        crate = data[8 + jl : 8 + jl + cl]
        cksum = hashlib.sha256(crate).hexdigest()
        if os.path.exists(f"{ROOT}/wrong-index-cksum"):
            cksum = "0" * 64
        open(f"{ROOT}/crates/{meta['name']}-{meta['vers']}.crate", "wb").write(crate)
        deps = [
            {
                "name": d["name"],
                "req": d["version_req"],
                "features": d.get("features", []),
                "optional": d.get("optional", False),
                "default_features": d.get("default_features", True),
                "target": d.get("target"),
                "kind": d.get("kind", "normal"),
                **({"registry": d["registry"]} if d.get("registry") else {}),
            }
            for d in meta.get("deps", [])
        ]
        line = {
            "name": meta["name"],
            "vers": meta["vers"],
            "deps": deps,
            "cksum": cksum,
            "features": meta.get("features", {}),
            "yanked": False,
        }
        p = f"{ROOT}/index/{ipath(meta['name'])}"
        os.makedirs(os.path.dirname(p), exist_ok=True)
        open(p, "a", encoding="utf-8").write(json.dumps(line) + "\n")
        self.log({"published": meta["name"], "vers": meta["vers"], "cksum": cksum})
        self.send(
            200,
            json.dumps(
                {
                    "warnings": {
                        "invalid_categories": [],
                        "invalid_badges": [],
                        "other": [],
                    }
                }
            ).encode(),
        )

    def send(self, code, body):
        self.send_response(code)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *a):
        pass


# ROOT/seed holds "<crate> <index line>" rows to preload (e.g. a yanked version).
if os.path.exists(f"{ROOT}/seed"):
    for row in open(f"{ROOT}/seed", encoding="utf-8"):
        crate, _, entry = row.strip().partition(" ")
        p = f"{ROOT}/index/{ipath(crate)}"
        os.makedirs(os.path.dirname(p), exist_ok=True)
        open(p, "a", encoding="utf-8").write(entry + "\n")
server = http.server.ThreadingHTTPServer(("127.0.0.1", PORT), H)
BOUND = server.server_address[1]
open(f"{ROOT}/ready", "w", encoding="utf-8").write(str(BOUND))
server.serve_forever()
