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
- [ ] Confirm the reviewed public pin and decernor-generated anchor pair are
      committed; Dave generates them with `make release-insert-anchors` using
      `DECERNOR_BIN` (absolute executable) or `decernor` on `PATH`, version
      0.1.7 or newer. No sibling repository path is inferred
- [ ] From a clean, freshly fetched `main`, run `make release-preflight`

The preflight requires a clean tree, the full `make pr-final` gate, exact
release-note extraction, a successful fetch, and exact equality between
`HEAD` and fetched `origin/main`.

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
