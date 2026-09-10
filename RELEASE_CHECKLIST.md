# Release Checklist

This checklist covers release preparation, the annotated tag and unsigned
draft, and the later maintainer MFA signing ceremony. CI never receives
signing keys and never publishes a GitHub release.

## Prerequisites

- `make bootstrap` and `make tools` are green
- `gh`, minisign, and optional GPG tooling are available
- `gh` is authenticated for `3leaps/storageprims`
- Approved signing material is loaded from an external secret store without
  printing values or paths

Required ceremony variables:

- `STORAGEPRIMS_RELEASE_TAG` — the sole canonical tag input, for example
  `v0.1.0`
- `STORAGEPRIMS_MINISIGN_KEY` — minisign secret-key file outside the repository
- `STORAGEPRIMS_MINISIGN_PUB` — explicit minisign public-key file

Optional PGP signing requires both `STORAGEPRIMS_PGP_KEY_ID` and
`STORAGEPRIMS_GPG_HOMEDIR`. Partial configuration fails closed.

## 1. Write and prepare

- [ ] Confirm `VERSION` is the intended stable version (`0.1.0` for the first
      cut); do not bump it for the first cut
- [ ] Run `make version-sync`
- [ ] Update `CHANGELOG.md` and its comparison links
- [ ] Update `RELEASE_NOTES.md`, newest first, retaining at most three cuts
- [ ] Copy only the current section to `docs/releases/vX.Y.Z.md`
- [ ] Confirm README MSRV badge and prose exactly match
      `[workspace.package].rust-version`
- [ ] Run `make pr-final` and `make release-check`
- [ ] Commit the release preparation and merge it through the normal review
      process
- [ ] Confirm required CI on `main` is green
- [ ] From a clean, freshly fetched `main`, run `make release-preflight`

The preflight requires a clean tree, the full `make pr-final` gate, exact
release-note extraction, a successful fetch, and exact equality between
`HEAD` and fetched `origin/main`.

## 2. Create the annotated tag and unsigned draft

From clean `main`:

```bash
export STORAGEPRIMS_RELEASE_TAG=v0.1.0
make release-guard-tag-version
git tag -a "$STORAGEPRIMS_RELEASE_TAG" \
  -m "$STORAGEPRIMS_RELEASE_TAG: first storageprims release"
git push origin "$STORAGEPRIMS_RELEASE_TAG"
```

- [ ] Confirm the tag workflow is green
- [ ] Confirm the GitHub release is still a draft
- [ ] Confirm it contains exactly the three Unix FFI archives, CycloneDX SBOM,
      and both licenses
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
make release
```

`make release` performs the only serialized walk:

1. Safely empty the repository-owned `dist/release`
2. Download and structurally validate the exact unsigned draft assets
3. Add the exact per-cut release notes
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

## crates.io

crates.io publication is not part of this release. There is no registry token
in CI and no release target invokes `cargo publish`. A later, separately cued
release task may publish `storageprims-core`, then `storageprims-ops`, then
`storageprims-s3`; the FFI crate remains unpublished.
