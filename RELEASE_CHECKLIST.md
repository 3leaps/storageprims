# Release Checklist

This checklist covers release preparation, the signed tag and unsigned
draft, and the later maintainer MFA signing ceremony. CI never receives
signing keys and never publishes a GitHub release.
Earlier unsigned annotated tags remain historical; signing begins with the
first cut using this gate.

## Prerequisites

- `make bootstrap` and `make tools` are green
- `gh`, minisign, and optional GPG tooling are available
- `gh` is authenticated for `3leaps/storageprims`
- Approved signing material is loaded from an external secret store without
  printing values or paths

Required ceremony variables:

- `STORAGEPRIMS_RELEASE_TAG` — the sole canonical tag input, for example
  `v0.1.1` when `VERSION` contains `0.1.1`
- `STORAGEPRIMS_MINISIGN_KEY` — minisign secret-key file outside the repository
- `STORAGEPRIMS_MINISIGN_PUB` — explicit minisign public-key file
- `STORAGEPRIMS_DECERNOR_BIN` — absolute executable Decernor v0.1.8+ binary
  selected by the maintainer; self-reported identity does not authenticate
  the file, so bind it to a trusted tag-built regular file (not a symlink)
- `STORAGEPRIMS_TAG_MESSAGE_DIR` — external per-cut directory ending in the
  canonical tag; its only message input is `message.txt`
- `STORAGEPRIMS_TAGGER_NAME` / `STORAGEPRIMS_TAGGER_EMAIL` — fixed infosec
  tagger identity (`3 Leaps Infosec Team <infosec@3leaps.net>`)
- `STORAGEPRIMS_GPG_SIGNING_FINGERPRINT` — full 40-hex authorized primary
  fingerprint; `STORAGEPRIMS_PGP_KEY_ID` selects an exact 40-hex signing
  subkey with a trailing `!`

GPG tag signing requires `STORAGEPRIMS_PGP_KEY_ID` and
`STORAGEPRIMS_GPG_HOMEDIR`. Both manifest signatures are made during the
subsequent maintainer ceremony. Partial configuration fails closed.

## 1. Write and prepare

- [ ] Confirm `VERSION` is the intended stable version for this cut
- [ ] For a patch bump, run `make version-patch` (which runs
      `make version-sync`); if setting `VERSION` directly, run
      `make version-sync` afterward
- [ ] Check release-tooling fixtures for hard-coded prior-version values;
      update them or derive expected assets independently from `VERSION`.
      Run `make release-tooling-test` and confirm mismatched tags fail
- [ ] Update `CHANGELOG.md` and its comparison links
- [ ] Update `RELEASE_NOTES.md`, newest first, retaining at most three cuts
- [ ] Copy only the current section to `docs/releases/vX.Y.Z.md`
- [ ] Confirm README MSRV badge and prose exactly match
      `[workspace.package].rust-version`
- [ ] Run `make pr-final` and `make release-check`
- [ ] Commit the release preparation and merge it through the normal review
      process
- [ ] Confirm required CI on `main` is green
- [ ] Merge the release-tag and anchor tooling through a reviewed PR **before**
      adding its public pin and anchors. A disposable worktree of the local
      tooling branch may be used to rehearse the steps below; perform the real
      public-pin/anchor change on a separate branch from updated `main` and
      review it in a separate PR before tagging
- [ ] Confirm the reviewed public pin and decernor-generated anchor pair are
      committed; Dave generates them with `make release-insert-anchors` using
      `STORAGEPRIMS_DECERNOR_BIN` set to an absolute executable Decernor
      v0.1.8 or newer. The ceremony never falls back to `DECERNOR_BIN` or PATH;
      generation verifies the installed pair against both public exports
- [ ] From a clean, freshly fetched `main`, run `make release-preflight`

The preflight requires a clean tree, the full `make pr-final` gate, exact
release-note extraction, a successful fetch, and exact equality between
`HEAD` and fetched `origin/main`.

### Initial public pin and anchors (maintainer only)

Export the **existing approved key's public portion** into the repository for
review; this does not generate a new key or change the key already registered
on GitHub. GitHub's Verified badge is a secondary check, while the committed
pin authorizes the signing key used by CI. Run the commands from the repository
root on the separate public-pin/anchor branch in a fresh shell. Set the intended
release tag by hand
**before** loading the approved external environment so its per-cut message
directory is derived from the intended tag.
Confirm the external message directory ends in that tag. Do not use an ambient
GPG home:

```bash
export STORAGEPRIMS_RELEASE_TAG=v0.1.2 # example: first signed cut; set each cut explicitly
# Load the approved external environment for this repository before continuing.
(
  set -euo pipefail # a failed guard stops this entire block, even in an interactive shell
  : "${STORAGEPRIMS_RELEASE_TAG:?set the approved tag for this cut}"
  : "${STORAGEPRIMS_GPG_HOMEDIR:?load the approved GPG home}"
  : "${STORAGEPRIMS_PGP_KEY_ID:?load the exact signing-subkey selector}"
  : "${STORAGEPRIMS_GPG_SIGNING_FINGERPRINT:?load the approved primary fingerprint}"
  : "${STORAGEPRIMS_DECERNOR_BIN:?load the trusted Decernor executable}"
  : "${STORAGEPRIMS_TAG_MESSAGE_DIR:?load the per-cut message directory}"
  : "${STORAGEPRIMS_MINISIGN_PUB:?load the approved minisign public export}"
  [[ "$STORAGEPRIMS_GPG_HOMEDIR" == /* && -d "$STORAGEPRIMS_GPG_HOMEDIR" ]]
  [[ "$STORAGEPRIMS_DECERNOR_BIN" == /* && -f "$STORAGEPRIMS_DECERNOR_BIN" &&
     ! -L "$STORAGEPRIMS_DECERNOR_BIN" && -x "$STORAGEPRIMS_DECERNOR_BIN" ]]
  python3 - "$STORAGEPRIMS_GPG_HOMEDIR" "$HOME/.gnupg" <<'PY'
from pathlib import Path
import sys
if Path(sys.argv[1]).resolve() == Path(sys.argv[2]).resolve():
    raise SystemExit('STOP: default GPG home is not approved for this ceremony')
PY
  [[ "$STORAGEPRIMS_PGP_KEY_ID" == *'!' ]]
  [[ "${STORAGEPRIMS_TAG_MESSAGE_DIR%/}" == */"$STORAGEPRIMS_RELEASE_TAG" ]]
  bash -c 'source scripts/release-decernor.sh; resolve_release_decernor ceremony'
  pin=docs/security/release-signing-keys.asc
  [[ ! -e "$pin" && ! -L "$pin" ]] || { echo 'STOP: public pin already exists' >&2; exit 1; }
  set -C # never overwrite a public pin
  gpg --homedir "$STORAGEPRIMS_GPG_HOMEDIR" --batch --armor \
    --export "$STORAGEPRIMS_PGP_KEY_ID" > "$pin"
  require_public_only() {
    if "$STORAGEPRIMS_DECERNOR_BIN" fingerprint "$1" --kind "$2" \
      --class private --fail-on-empty --path-mode none >/dev/null; then
      echo 'STOP: private material in public export' >&2; exit 1
    else
      decernor_rc=$?
      [[ "$decernor_rc" -eq 3 ]] || { echo 'STOP: private-material check failed' >&2; exit 1; }
    fi
  }
  require_public_only "$pin" gpg
  [[ -s "$STORAGEPRIMS_MINISIGN_PUB" && ! -L "$STORAGEPRIMS_MINISIGN_PUB" ]]
  require_public_only "$STORAGEPRIMS_MINISIGN_PUB" minisign
  grep -q '^untrusted comment:' "$STORAGEPRIMS_MINISIGN_PUB"
  verify_tmp="$(mktemp -d)"
  trap 'rm -rf "$verify_tmp"' EXIT
  "$STORAGEPRIMS_DECERNOR_BIN" fingerprint "$pin" --kind gpg \
    --class public --fail-on-empty --path-mode none >"$verify_tmp/gpg.ndjson"
  python3 - "$verify_tmp/gpg.ndjson" "$STORAGEPRIMS_GPG_SIGNING_FINGERPRINT" \
    "$STORAGEPRIMS_PGP_KEY_ID" <<'PY'
import json
import pathlib
import re
import sys
records = [json.loads(line) for line in pathlib.Path(sys.argv[1]).read_text().splitlines()]
primary, selector = sys.argv[2:]
if not re.fullmatch('[0-9A-F]{40}', primary) or not re.fullmatch('[0-9A-F]{40}!', selector):
    raise SystemExit('STOP: invalid configured fingerprints')
if len(records) != 2 or any(r.get('kind') != 'gpg' or r.get('class') != 'public' for r in records):
    raise SystemExit('STOP: expected exactly one public primary and signing subkey')
by_role = {r.get('key_role'): r.get('fingerprint') for r in records}
if by_role != {'primary': primary, 'subkey': selector[:-1]}:
    raise SystemExit('STOP: exported public key differs from approved selector')
PY
  gpg --homedir "$verify_tmp" --batch --show-keys \
    --fingerprint --with-subkey-fingerprint "$pin" # inspect readable expiry/revocation
)
```

The `!` selects one signing subkey; the export includes its primary public key.
Decernor must find **no private record**. With `--fail-on-empty`, exit 3 is the
expected no-match result; exit 0 means private material was found and every
other result is a stop. The block asserts exactly one primary matching the
approved fingerprint and one subkey matching the `!` selector. Before
proceeding, inspect the human-readable GPG output for unexpired, non-revoked
primary and signing subkey; it uses a throwaway home, not the ceremony keyring.
Only after those checks pass, run:

```bash
(
  set -euo pipefail # stop before later checks if generation or verification fails
  make release-insert-anchors
  ./scripts/validate-release-anchors.sh
  make release-tooling-test
  make pr-final
  git status --short
)
```

The minisign export starts with `untrusted comment:`. After insertion, Decernor reports
`fingerprint verify: records=2 findings=0` and the script reports
`[ok] generated public fingerprint anchors for review`. The two generated
files contain exactly `gpg <40 uppercase hex>` and `minisign <64 lowercase hex>`
in TXT, with corresponding primary and public-blob NDJSON records. The GPG
anchor equals the approved primary. Only the public pin and those two anchors
should appear in the working-tree diff; reviewers re-derive the fingerprints
from the public exports before approving the PR. Stop on any unexpected file,
private marker, missing or extra key, expiry/revocation, mismatch, invalid
Decernor version, or nonzero generation/verification result. Fix the cause and
regenerate; never hand-edit an anchor. If a post-export check fails, Dave must
confirm that the new untracked public pin is his intended export, remove that
failed export, then rerun from the beginning. Never overwrite an existing pin.

## 2. Create the signed tag and unsigned draft

Only after an explicit tag cue, from clean `main` at the preflighted commit:

```bash
: "${STORAGEPRIMS_RELEASE_TAG:?load the approved cut}"
: "${STORAGEPRIMS_TAG_MESSAGE_DIR:?load the external per-cut message dir}"
test -s "${STORAGEPRIMS_TAG_MESSAGE_DIR}/message.txt"
make release-tag          # creates and verifies locally, no push
make release-push-tag     # explicit maintainer action; verifies remote object
```

- [ ] Confirm the tag workflow is green
- [ ] Confirm the tag has GitHub **Verified** status and the
      `verify-signature` gate passed before a draft is created
- [ ] Confirm the GitHub release is still a draft
- [ ] Confirm it contains the FFI archives from
      `config/release/ffi-platforms.txt`, CycloneDX SBOM, and both licenses
- [ ] Do not sign until the draft inventory is complete

## 3. Maintainer MFA sign and upload

Use a clean detached checkout. Fetch both the exact tag and `main`; the strict
guard peels the annotated tag and requires the tag, `HEAD`, and fetched
`origin/main` to be the same commit.

```bash
: "${STORAGEPRIMS_RELEASE_TAG:?load the approved release tag}"
: "${STORAGEPRIMS_MINISIGN_KEY:?load the approved secret key}"
: "${STORAGEPRIMS_MINISIGN_PUB:?load the approved public key}"
test -z "$(git status --porcelain)"
git fetch origin \
  "+refs/heads/main:refs/remotes/origin/main" \
  "+refs/tags/${STORAGEPRIMS_RELEASE_TAG}:refs/tags/${STORAGEPRIMS_RELEASE_TAG}"
git checkout --detach "$STORAGEPRIMS_RELEASE_TAG"
STORAGEPRIMS_REQUIRE_TAG=1 make release-guard-tag-version
make release-verify-tag
make release-verify-remote-tag
make release
```

`make release` performs the only serialized walk:

Keep `STORAGEPRIMS_DECERNOR_BIN` bound to the reviewed absolute v0.1.8+
executable throughout the ceremony; `release-verify-keys` re-derives both
exported publics against the anchors staged into the signed set.

1. Safely empty the repository-owned `dist/release`
2. Download and structurally validate the exact unsigned draft assets
3. Add the exact per-cut release notes and staged public fingerprint anchors
4. Generate exact SHA-256 and SHA-512 manifests
5. Sign both manifests
6. Export and prove public verification keys
7. Verify once, upload the explicit provenance set, and recheck the exact
   remote inventory

- [ ] Confirm the release remains a draft after upload
- [ ] Independently download into an empty directory and verify both checksum
      manifests and minisign signatures
- [ ] Review the final release notes and asset list
- [ ] Publish the GitHub release only as a separate, explicit maintainer action

## crates.io (manual maintainer action after the tag)

The first registry version is the current tagged cut; do not backfill older
tags. Later cuts publish only their new versions. A registry version cannot
be overwritten: a correction takes a new patch version or a separately cued
yank. CI has no registry token and never uploads crates.

The sole ordered list is `config/release/publishable-crates.txt`; print and
validate it with `make release-crates-list`. The FFI crate and any future CLI
are unpublished (`publish = false`). The list orders dependencies, including
dev dependencies.

After the tag exists on origin, use a clean detached checkout of the exact
tag and recheck `STORAGEPRIMS_REQUIRE_TAG=1 make release-guard-tag-version`.
Re-run `make release-verify-remote-tag` before the dry run and each registry
publication. The signed tag is the provenance root for registry publication.
Run `make release-crates-dry-run` for all publishable crates. This uses local
path patches only for earlier workspace crates that have not yet reached the
registry; it does not upload them. Optionally confirm that
`cargo publish --dry-run -p storageprims-ffi` fails as unpublished (and do
the same for a future `storageprims-cli`).

Dave uses a crates.io token scoped to the publishable names and kept in an
external secret store, never the repository or CI. For first uploads it needs
`publish-new` and `publish-update`; subsequent updates need only
`publish-update`. Use an expiry of 30–90 days; do not grant `yank` without a
separate decision. Confirm any new name is unclaimed immediately before its
first upload using `cargo info --registry crates-io <crate>`.
Load the token only into the environment for the specific publish command;
do not use `cargo login` (which persists plaintext in Cargo credentials).

Only after an explicit publish cue, Dave runs the following **one crate at a
time** in the order printed by `make release-crates-list`, setting `crate`
to the next list entry before each pass:

```bash
make release-crates-list
cargo publish --locked -p "${crate:?set the next ordered crate}"
make release-crates-verify CRATE="$crate"
# Repeat the previous two commands for each remaining entry; after the last:
make release-crates-verify
```

Immediately before **each** upload, reconfirm the cue; any intervening hold
stops the sequence. Each verification waits for `cargo info --registry crates-io
<crate>@<version>` (including after the last crate), and checks the exact
version on the crates.io API. The final command rechecks the entire list. Never use bare
`cargo info` as proof: it may resolve a local workspace crate. Review the
registry pages and docs.rs builds after the final check. If the tag Release
workflow's package check failed while registry dependencies were unavailable,
rerun it after the index exposes this version, before signing the draft.

## Rotate release signing keys

Before the first cut with a new key, generate a new public export and anchors
under maintainer identity and land both through a reviewed PR. CI must see the
new pinned public key _in the tagged commit_; an account-level GitHub Verified
indicator alone cannot authorize a release. Re-derive both fingerprint records
with `decernor fingerprint`, review the primary/subkey relationship, and confirm
the tagger account has the matching public key and verified email. Check the
primary and signing-subkey expiration with:

```bash
gpg --homedir "$STORAGEPRIMS_GPG_HOMEDIR" --list-keys --with-subkey-fingerprint --with-colons "$STORAGEPRIMS_GPG_SIGNING_FINGERPRINT"
```

An expired key is a rotation nobody scheduled. Do not sign or publish until a
reviewed replacement pin is on `main` and the tag ruleset is active.
