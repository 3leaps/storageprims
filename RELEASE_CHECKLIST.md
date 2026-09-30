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
- The authorized GitHub user with registered MFA runs both tag Make targets

Required ceremony variables:

- `STORAGEPRIMS_RELEASE_TAG` — the sole canonical tag input, for example
  `v0.1.1` when `VERSION` contains `0.1.1`
- `STORAGEPRIMS_MINISIGN_KEY` — minisign secret-key file outside the repository
- `STORAGEPRIMS_MINISIGN_PUB` — explicit minisign public-key file
- `STORAGEPRIMS_DECERNOR_BIN` — absolute executable Decernor v0.1.8+ binary
  selected by the maintainer; self-reported identity does not authenticate
  the file, so bind it to a trusted tag-built regular file (not a symlink)
- `STORAGEPRIMS_TAG_MESSAGE_DIR` — external per-cut directory ending in the
  canonical tag; `make release-prepare-tag-message` creates the directory and
  complete `message.txt` when absent. This is the approved public tag
  annotation (for example, `storageprims v0.1.2` and one final newline), not
  a fill-in template or release-notes copy. It must be nonempty UTF-8, regular
  and non-symlink, with no CR/NUL, trailing whitespace
  or extra final blank line. Do not include policy digests or signing material.
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
- [ ] Confirm the reviewed public pin and Decernor-generated anchor pair are
      committed. Generate the anchors with `make release-insert-anchors` and
      `STORAGEPRIMS_DECERNOR_BIN` set to an absolute executable Decernor
      v0.1.8 or newer. The target never falls back to `DECERNOR_BIN` or PATH,
      and generation verifies the installed pair against both public exports.

- [ ] Select the canonical tag and approved external per-cut message directory
      in the environment (or use the approved optional external loader described
      below). Prepare and review the public message:

  ```console
  $ STORAGEPRIMS_RELEASE_TAG=v0.1.2 make release-prepare-tag-message
  ```

  This target needs no signing keys, GitHub access, or MFA. It creates a
  finished one-line public message when absent, or validates and previews an
  existing maintainer-approved custom message without overwriting it. If the
  directory is unsafe, the message is malformed or contains an unresolved
  example placeholder, stop and correct the external input before proceeding.
  It never signs or publishes. The maintainer reviews and approves the displayed
  text before the local tag ceremony. Select the tag before loading an optional
  per-cut environment script; neither route requires a new env-file convention.

- [ ] From a clean, freshly fetched `main`, run `make release-preflight`

The preflight requires a clean tree, the full `make pr-final` gate, exact
release-note extraction, a successful fetch, and exact equality between
`HEAD` and fetched `origin/main`. It also reports visible applicable tag
protection as FOUND, ABSENT or UNKNOWN. The report is advisory, not a tag
authorization or a signed claim about mutable GitHub policy; inspect actual
applicability and bypass behavior under the authorized account when relevant.

### Initial public pin and anchors (maintainer only)

The committed public pin authorizes the signing key used by CI; GitHub's
Verified badge is a secondary check. On the separate public-pin/anchor branch,
load the approved external environment in a fresh shell. From the repository
root, run these Make targets from zsh or bash; their scripts select Bash 3.2 or
newer via their shebangs. They do not need the release tag or message directory.
An empty per-cut message directory does not block public-pin validation or
anchor generation;
`message.txt` is required later for tag creation. Do not use an ambient GPG
home for export.

**If an approved public pin already exists**, validate that existing export:

```bash
make release-validate-pin
```

This target reads the existing public pin and approved minisign export; it does
not require the ceremony GPG home and never exports or overwrites the pin.
**Only if no pin exists**, explicitly export the existing approved key's public
portion and validate it in one step. This does not create a key or change the
one registered on GitHub:

```bash
make release-export-pin
```

Export refuses an existing pin and uses no-clobber output. Never regenerate an
approved pin to satisfy the procedure. Decernor must find **no private record**
in either public export (`--fail-on-empty` exit 3); validation asserts exactly
one approved primary and the selected signing subkey (`!`). Inspect its
human-readable GPG output for an unexpired, non-revoked primary and subkey;
the display uses a throwaway home. Only after validation succeeds, run:

```bash
(
  set -euo pipefail # stop before later checks if generation or verification fails
  make release-insert-anchors # revalidates the existing pin before generation
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
regenerate anchors; never hand-edit an anchor. If validation of an existing
approved pin fails, stop and investigate without modifying it. If a newly
exported pin fails a post-export check, stop. Confirm the new untracked pin is
the intended export before removing that failed export; then rerun from the
beginning. Never overwrite an existing pin.

## 2. Create the signed tag and unsigned draft

Only after message review, a passing preflight and an explicit tag cue, from
clean `main` at the preflighted commit, the authorized GitHub user with MFA
runs the local tag target. The Make targets use Bash regardless of the
operator's interactive shell (including zsh). Provide the approved ceremony
variables as exported environment variables, or optionally export
`STORAGEPRIMS_APPROVED_ENV_LOADER` pointing to an existing approved,
shell-sourceable external script. The loader must be a readable, regular,
non-symlink absolute file outside this repository; the target sources it
privately and suppresses its output. Do not print loader paths or keys; the
prepare target previews only the finished public message. Run each tag command
only after its own explicit cue:

```console
$ STORAGEPRIMS_RELEASE_TAG=v0.1.2 make release-tag
# Stop; review the locally verified tag object before a separate remote-push cue.
$ STORAGEPRIMS_RELEASE_TAG=v0.1.2 make release-push-tag
```

The optional loader pointer is not required when the ceremony variables are
already exported. `release-tag` signs and
verifies **locally only**; `release-push-tag` re-verifies the local object and
reports visible tag protection before a normal remote push and GitHub
verification. A missing or inaccessible rule produces ABSENT/UNKNOWN advisory
output, not a claim of protection or an exception to signature checks. Neither
target loads a secret from the repository or creates a tag implicitly during
the push step. Stop on any failing target; do not replace an existing tag.

- [ ] Confirm the tag workflow is green and its exact unsigned draft exists.
      Stop on failure; never move or replace a signed tag without a separate
      explicit maintainer decision.
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

For registry publication, use a crates.io token scoped to the publishable
names. Keep it in an external secret store, never in the repository or CI.
First uploads require `publish-new` and `publish-update`; subsequent updates
require only `publish-update`. Set an expiry of 30–90 days. Do not
grant `yank` without a separate decision. Immediately before the first upload
of each new name, confirm it is unclaimed with
`cargo info --registry crates-io <crate>`. Load the token into the environment
only for the specific publish command. Do not use `cargo login`, which persists
plaintext in Cargo credentials.

Only after an explicit publish cue, run the following **one crate at a time**
in the order printed by `make release-crates-list`. Set `crate` to the next
list entry before each pass:

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

If a key is expired, stop. Do not sign or publish until a reviewed replacement
pin is on `main`; review the current tag-protection advisory separately.
