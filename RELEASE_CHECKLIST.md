# Release Checklist

This checklist covers release preparation, the signed tag, and the
maintainer MFA ceremony that creates the draft from verified workflow
artifacts, signs it, and uploads the signed set. CI runs with read-only
permissions, never receives signing keys, and never creates or publishes a
GitHub release.
Earlier unsigned annotated tags remain historical; signing begins with the
first cut using this gate.

## Prerequisites

- `make bootstrap` and `make tools` are green
- `gh`, minisign, and optional GPG tooling are available
- `make release-tooling-test` and the ceremony also need bash, `jq`, `rg`,
  `curl`, `python3` (3.11+), `cargo`, gpg, minisign, a Decernor v0.1.8+ binary
  (`STORAGEPRIMS_DECERNOR_BIN` or `PATH`), and mikefarah `yq` v4 (Debian's
  `apt` package named `yq` is a different tool)
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
  annotation (for example, `storageprims v0.2.0` and one final newline), not
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
  $ STORAGEPRIMS_RELEASE_TAG=v0.2.0 make release-prepare-tag-message
  ```

  This target needs no signing keys, GitHub access, or MFA. It creates a
  finished one-line public message when absent, or validates and previews an
  existing maintainer-approved custom message without overwriting it. If the
  directory is unsafe, the message is malformed or contains an unresolved
  example placeholder, stop and correct the external input before proceeding.
  It never signs or publishes. The maintainer reviews and approves the displayed
  text before the local tag ceremony. Select the tag before loading an optional
  per-cut environment script; neither route requires a new env-file convention.

- [ ] From a clean, freshly fetched `main`, run `make release-preflight`.
      After the clean-tree check it runs the GitHub repository preflight
      (also available as `make release-github-preflight`; needs repository
      administration read access and a gh configuration you trust). It stops
      unless the default `GITHUB_TOKEN` is read-only, Actions cannot approve
      pull requests, exactly one active `Tag Publish Protection` ruleset
      protects `refs/tags/v*` with only the intended organization-admin
      bypass, and immutable releases are enabled. It checks these mutable
      settings at that moment only. The token default does not limit a
      workflow that declares its own permissions; the workflow policy check
      covers that. Unlike the advisory tag-rule report below, this preflight
      is a hard stop. With immutable releases, a published release is final:
      assets cannot be added or replaced, so the draft must be complete and
      verified before publishing.

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

## 2. Create the signed tag

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
$ STORAGEPRIMS_RELEASE_TAG=v0.2.0 make release-tag
# Stop; review the locally verified tag object before a separate remote-push cue.
$ STORAGEPRIMS_RELEASE_TAG=v0.2.0 make release-push-tag
```

The optional loader pointer is not required when the ceremony variables are
already exported. `release-tag` signs and
verifies **locally only**; `release-push-tag` re-verifies the local object and
reports visible tag protection before a normal remote push and GitHub
verification. A missing or inaccessible rule produces ABSENT/UNKNOWN advisory
output, not a claim of protection or an exception to signature checks. Neither
target loads a secret from the repository or creates a tag implicitly during
the push step. Stop on any failing target; do not replace an existing tag.

- [ ] Confirm the tag workflow is green. Stop on failure; never move or
      replace a signed tag without a separate explicit maintainer decision.
- [ ] Confirm the tag has GitHub **Verified** status and the
      `verify-signature` job passed
- [ ] Confirm the run uploaded the `release-identity`, `sbom`, and one
      `ffi-<platform>` artifact per line of `config/release/ffi-platforms.txt`
- [ ] Confirm no GitHub release exists yet for the tag; the ceremony creates
      the draft

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
2. Stage the exact unsigned artifacts of the one successful push run of the
   tag workflow at the verified tag commit (`make release-fetch`); set
   `STORAGEPRIMS_RELEASE_RUN_ID` when more than one run qualifies. The
   artifact release identity must name the verified tag object and commit,
   and the licenses must equal the tagged commit's
3. Create a new draft from the staged set and upload it create-only
   (`make release-draft`); it refuses when any release or draft for the tag
   exists or its absence cannot be proven from the full release list
4. Add the exact per-cut release notes and staged public fingerprint anchors
5. Generate exact SHA-256 and SHA-512 manifests
6. Sign both manifests
7. Export and prove public verification keys copied from the tagged commit
8. Verify once, prove the draft still holds the verified unsigned bytes,
   upload the explicit provenance set, and recheck the exact remote inventory

Each minisign manifest signature may prompt for the minisign secret-key
passphrase. The terminal identifies the manifest and prompt purpose; two
prompts are expected when signing the two manifests. Never paste a passphrase
or secret-key path into command receipts or release notes.

Recovery after a stop. Never rerun `make release` once a draft exists:
`release-draft` refuses any existing release or draft for the tag.

- Stopped after `release-draft` completed and before `release-upload` (for
  example a signing prompt failed): run `make release-resume`. It re-stages the
  verified workflow artifacts and `release-verify-draft` requires the one draft
  for the tag to hold exactly those unsigned bytes before anything is signed.
  Resume re-fetches from the same workflow run: set the same
  `STORAGEPRIMS_RELEASE_RUN_ID` if the first fetch needed one, and resume while
  the run's artifacts are retained (30 days). A rerun of the workflow produces
  different archive bytes; `release-verify-draft` then refuses, and the draft
  must be deleted and the ceremony restarted.
- Stopped inside `release-draft` (partial asset upload) or inside
  `release-upload` (some signed assets uploaded): the draft inventory no longer
  matches and `release-resume` refuses. Inspect the draft, delete it as a
  separate explicit maintainer decision, then run `make release` again.

`release-upload` runs `release-verify-draft` before it uploads signatures, so a
draft asset replaced after creation stops the ceremony. Every upload and the
notes update address that verified release id, never the tag, and the same
release must then hold exactly the signed set byte for byte. Do not publish,
edit, or delete the draft by any other means while the ceremony runs.

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

### Enter the exact tagged checkout

This registry procedure can be run independently of the signing ceremony.
Load the approved external environment, select the tag, and start with a clean
checkout. Note the starting branch (`git branch --show-current`) so you can
return after **all** release work is complete. Do not discard changes to make
the checkout clean.

The tag and exact-version examples in this section are for the **v0.2.0 cut**.
For a future cut, match the selected tag to `v$(cat VERSION)` and use that
version in the registry queries; do not copy a previous cut's tag or version.

```bash
export STORAGEPRIMS_RELEASE_TAG=v0.2.0
test -z "$(git status --porcelain)" # stop if this fails
git fetch origin \
  "+refs/heads/main:refs/remotes/origin/main" \
  "+refs/tags/${STORAGEPRIMS_RELEASE_TAG}:refs/tags/${STORAGEPRIMS_RELEASE_TAG}"
git checkout --detach "$STORAGEPRIMS_RELEASE_TAG"
STORAGEPRIMS_REQUIRE_TAG=1 make release-guard-tag-version
make release-verify-tag
make release-verify-remote-tag
make release-crates-list
make release-crates-dry-run
```

Run commands in order and stop at the first failure. The strict guard requires
detached `HEAD`, a clean tree, and identical tag/`HEAD`/fetched `origin/main`
commits; the remote verifier checks the signed annotated object and GitHub
verification. Keep `main` fixed throughout the ceremony.

The Make dry-run rehearsal uses local path patches for **all earlier entries**
in the publication list, whether or not they are already on the registry. It
does not upload anything or prove unpatched registry dependency resolution.
For a separate per-crate dry run, wait until its predecessors are indexed.
Cargo may print `Uploading` followed by `aborting upload due to dry
run`; that warning is expected, not an actual upload. Optionally confirm that
`cargo publish --dry-run -p storageprims-ffi` fails as unpublished (and do
the same for a future `storageprims-cli`).

For registry publication, use a crates.io token whose name scope includes every
crate from `make release-crates-list`. Store the token in an external regular
file and set `STORAGEPRIMS_CARGO_TOKEN_FILE` to its absolute path. Both
`CARGO_REGISTRY_TOKEN` and `CARGO_REGISTRIES_CRATES_IO_TOKEN` must be unset,
even if empty. Do not use `cargo login` or place credentials in the repository
or CI. Use a short-lived `publish-new` token for first uploads and revoke it
afterwards; subsequent updates need only `publish-update`.

Publish the immutable GitHub release first, and keep merges to `main` frozen
until all crates have been verified. Work from a clean detached checkout of
the signed tag with its pinned Rust toolchain installed.

### Publish one crate at a time

After the maintainer's explicit publication cue:

```bash
make release-crates-list
make release-crates-publish
make release-crates-verify
```

The publisher exports the exact tagged blobs into a fresh tree and packages
each crate without compiling. Required build and dry-run checks must already
have passed before this publication step. It presents the crate, version and
archive SHA-256, then requires the maintainer to type the exact crate name at
a terminal. The Cargo credential provider releases the token only for that
registry, name, version and archive digest. Cargo source and command overrides
are refused; approval does not authorize a different archive.

Each successful upload waits for the sparse index and checks the downloaded
archive before proceeding to the next crate. A rerun skips a published version
only when its checksum equals the package from the tagged source; a mismatch,
yanked version or unreadable registry stops the run. Never re-upload a version
to recover from a verification failure.

The final `release-crates-verify` checks each downloaded archive against the
crates.io API checksum, both license texts and the tagged commit recorded in
`.cargo_vcs_info.json`, then checks docs.rs for that exact version. A pending
docs.rs build fails verification; rerun the read-only verification target after
the build completes. `CRATE=<name>` selects one listed crate for verification.

To inspect the
**0.2.0 example cut** manually, use the following. For future cuts, use the
version in `VERSION` that matches the selected release tag:

```bash
cargo info --registry crates-io storageprims-core@0.2.0
cargo info --registry crates-io storageprims-s3@0.2.0
cargo info --registry crates-io storageprims-local@0.2.0
cargo info --registry crates-io storageprims-ops@0.2.0
```

With no `@version`, an explicit registry query shows the latest version, not
necessarily this cut. Never use bare `cargo info` as registry proof: it may
resolve a local workspace crate. Review the registry pages and **separately**
check docs.rs builds for this version; index/API success does not establish a
documentation build. If the tag Release
workflow's package check failed while registry dependencies were unavailable,
rerun it after the index exposes this version, before signing the draft.

### Return to the starting branch

After **all** strict signing and registry work is complete, confirm the tree
is clean and reattach to the branch you noted earlier. If you started on main:

```bash
git status --short
test -z "$(git status --porcelain)" # stop if this fails
git switch main
```

Do not reset or discard unexpected work. If using a separate detached worktree,
remove it only after confirming it contains no work to preserve. Returning to a
branch before finishing the ceremony causes the strict detached-checkout guard
to fail.

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
