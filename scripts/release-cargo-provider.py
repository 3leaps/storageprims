#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Cargo credential provider that releases the registry token only for one
approved upload.

Cargo (1.88) asks a credential provider twice during `cargo publish`: first
`operation: read`, before packaging, then `operation: publish` with the crate
`name`, `vers` and `cksum` (the SHA-256 of the exact archive it is about to
send). This provider answers:

- `read`: a placeholder that is not a credential (Cargo aborts the whole
  publish if `read` fails; the placeholder is never sent because the
  registry does not require authentication for reads, which the ceremony
  checks before using this provider);
- `publish` for the approved index URL, name, version and checksum: the
  token, with `cache: never` and `operation_independent: false`;
- anything else (another crate, version or checksum, yank, owner, login,
  logout): an error, so Cargo transmits nothing.

The approval file (STORAGEPRIMS_CARGO_APPROVAL, written by the ceremony) holds
`index-url`, `name`, `vers` and `cksum`. The token comes from the file named
by STORAGEPRIMS_CARGO_TOKEN_FILE (read once, never logged). Every request and
decision, without the token, is appended to STORAGEPRIMS_CARGO_PROVIDER_LOG.
"""

import json
import os
import re
import sys

PLACEHOLDER = "storageprims-read-placeholder-not-a-credential"


def respond(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


def refuse(message):
    respond({"Err": {"kind": "other", "message": message}})


def main():
    with open(os.environ["STORAGEPRIMS_CARGO_APPROVAL"], encoding="utf-8") as handle:
        approval = json.load(handle)
    if set(approval) != {"index-url", "name", "vers", "cksum"} or not re.fullmatch(
        r"[0-9a-f]{64}", approval["cksum"]
    ):
        raise SystemExit("error: malformed approval")
    log = os.environ.get("STORAGEPRIMS_CARGO_PROVIDER_LOG")
    respond({"v": [1]})
    for line in sys.stdin:
        request = json.loads(line)
        operation = request.get("operation", request.get("kind"))
        decision = "refused"
        if request.get("kind") != "get":
            refuse(f"{request.get('kind')} is not supported by the release provider")
        elif operation == "read":
            decision = "placeholder"
            respond(
                {
                    "Ok": {
                        "kind": "get",
                        "token": PLACEHOLDER,
                        "cache": "never",
                        "operation_independent": False,
                    }
                }
            )
        elif (
            operation == "publish"
            and request.get("registry", {}).get("index-url") == approval["index-url"]
            and request.get("name") == approval["name"]
            and request.get("vers") == approval["vers"]
            and request.get("cksum") == approval["cksum"]
        ):
            decision = "released"
            with open(
                os.environ["STORAGEPRIMS_CARGO_TOKEN_FILE"], encoding="utf-8"
            ) as handle:
                token = handle.read().strip()
            if not token:
                raise SystemExit("error: empty token")
            respond(
                {
                    "Ok": {
                        "kind": "get",
                        "token": token,
                        "cache": "never",
                        "operation_independent": False,
                    }
                }
            )
        else:
            refuse(
                f"{operation} of {request.get('name')} {request.get('vers')} with checksum {request.get('cksum')} is not approved"
            )
        if log:
            record = {
                k: request.get(k)
                for k in ("kind", "operation", "name", "vers", "cksum")
            }
            record["index-url"] = request.get("registry", {}).get("index-url")
            record["decision"] = decision
            with open(log, "a", encoding="utf-8") as handle:
                handle.write(json.dumps(record) + "\n")


if __name__ == "__main__":
    if "--cargo-plugin" not in sys.argv[1:]:
        raise SystemExit("error: run by Cargo as a credential provider")
    main()
