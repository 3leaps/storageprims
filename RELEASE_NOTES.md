# Release Notes

> **Purge policy:** this landing page keeps the latest three releases in
> reverse chronological order. Older cuts remain in `docs/releases/`.
> The signed GitHub payload is the matching per-cut file, not this page.

---

## v0.2.1 — 2026-11-13

### Highlights

- Release drafts are bound to a recorded release ID, tag, commit, and exact
  asset inventory. `make release-publish` verifies the signed draft and remote
  tag before publication, then verifies the immutable published release.
- `make release-crates-publish` publishes crates in dependency order with
  per-crate confirmation and digest-bound credentials. Verification checks
  registry checksums, archive provenance, license texts, and docs.rs status.
- S3 clients explicitly refuse Multi-Region Access Points. The default
  dependency graph no longer enables SigV4A or includes the crypto-bigint
  dependency covered by RUSTSEC-2026-0387.
- S3 copy rejects absolute source or destination URIs with empty or slash-only
  object keys before sending a request. Valid absolute keys retain their
  literal bytes and remain independent of the provider root prefix.

### Compatibility

- Regional AWS S3 and S3-compatible endpoints retain SigV4 signing. MRAP ARNs
  are unsupported and refused before request dispatch.
- Downstream crates that independently enable the AWS SDK's SigV4A feature
  can restore asymmetric-signing dependencies and should audit their resolved
  dependency graph. This release does not prohibit every asymmetric-signing
  path supplied by a downstream SDK configuration.
- Rust API and C ABI signatures are unchanged. The Rust MSRV remains 1.94.1.

### Release tooling

- The package-check CI job uses a pinned tools container. Signing-subkey
  selector validation works with stock Debian awk.
- The checklist covers explicit draft publication, GitHub-first publication
  order, guarded crate publishing, and recovery after signed asset upload.

### Known issue

- On Windows, a `put` that fails can occasionally leave a reserved
  `.storageprims-tmp*` staging file in the destination directory. The provider
  excludes these entries from listings and refuses them as keys. A fix is
  planned.

### Upgrade notes

- The Rust workspace version is `0.2.1`. Publish `storageprims-core`,
  `storageprims-s3`, `storageprims-local`, and `storageprims-ops`, in that order,
  after the signed tag and verified GitHub release. Registry publication is
  an explicit maintainer step.
- Git consumers can pin `tag = "v0.2.1"` after the signed tag is published and
  verified. Release verification instructions are in
  `docs/security/README.md`.

Full notes: [docs/releases/v0.2.1.md](docs/releases/v0.2.1.md)

---

## v0.2.0 — 2026-11-11

### Highlights

- New crate `storageprims-local`: a provider rooted at a local directory, with
  the same `StorageProvider` contract as S3. Writes are atomic, create-only
  writes are enforced by the operating system, and paths cannot escape the
  root through symlinks or reparse points.
- `storageprims-ops::relay_copy` copies one object between two providers (for
  example local to S3; both built-in providers support it) within an explicit
  size, buffer, and time budget, and reports whether the destination write was
  committed.
- New optional extension traits in `storageprims-core`: `ReadResponseProvider`
  for reads that report what the provider observed, and `AdmittedPutProvider`
  for writes that report how far they got. `S3Provider` and `LocalProvider`
  implement both.
- New guides in `docs/guides/` cover how the crates fit together, the local
  provider, testing with it, and writing code that runs against local or S3
  storage, with runnable examples.

### Breaking changes

- `StorageError`, `Capability`, and `InspectionLimitDimension` are now
  `#[non_exhaustive]`. Exhaustive `match` expressions on them in downstream
  crates need a wildcard arm.
- `InspectionLimitDimension` has a new `Entries` variant (local listing scan
  budget). The C ABI's last-error JSON contract can carry
  `limit_dimension: "entries"`; this is additive, and all numeric error codes
  are unchanged.
- S3 `put` now refuses bodies over the 5 GiB single-`PutObject` ceiling,
  non-US-ASCII header values, and user metadata names that differ only in
  case (the new `admit_put` applies the same rules). Provider-relative S3 object
  keys that are only slashes, such as `"/"`, are refused, as empty keys
  already were. S3 errors and `CopyResult` report caller-relative keys rather
  than keys with the configured root prefix prepended.

### Local provider bounds

- Conditional writes: create-only (`MustNotExist`) on Linux, macOS, Android,
  and Windows; `Match` is not supported.
- Delimiter listing supports only `"/"`. Each listing request examines at most
  100,000 directory entries, counting every entry in each directory it opens,
  and is checked against a 10-second deadline between filesystem steps; a
  request over either bound fails rather than returning a partial page. A
  stalled filesystem call cannot be interrupted, so a request can take longer
  than 10 seconds before it fails.
- No guarded reads, ETags, content type, or user metadata.
- The provider assumes cooperating writers. A hard link planted inside the
  root by another writer is not detected; use an access-controlled root when
  untrusted parties can write to the directory.

### Known issue

- On Windows, a `put` that fails can occasionally leave a reserved
  `.storageprims-tmp*` staging file in the destination directory. The provider
  excludes these entries from listings and refuses them as keys. A fix is
  planned.

### Upgrade notes

- Registry consumers: `storageprims-core`, `storageprims-s3`,
  `storageprims-local`, and `storageprims-ops` are published at `0.2.0`, in
  that order, after the signed tag; publication of each crate is a separate
  maintainer step.
- Git consumers can pin `tag = "v0.2.0"` after the signed tag is published and
  verified.
- Release CI now builds artifacts only; the maintainer creates and signs the
  draft release locally. Release verification instructions are in
  `docs/security/README.md`.

Full notes: [docs/releases/v0.2.0.md](docs/releases/v0.2.0.md)

---

## v0.1.2 — 2026-09-28

### Highlights

- The Rust workspace has guarded crates.io packaging and publication tooling,
  including dependency order and registry verification.
- The release workflow verifies signed tags and includes a committed public
  OpenPGP key pin with OpenPGP and minisign fingerprint anchors.
- Public-source reference checks run in the release tooling, and the S3 client
  enables only its selected features. The public storage contracts are unchanged.

### Upgrade notes

- The workspace version is `0.1.2`. Publication and the signed `v0.1.2` tag
  are separate maintainer release steps.
- Git consumers can pin `v0.1.2` after the signed tag is published and verified.

Full notes: [docs/releases/v0.1.2.md](docs/releases/v0.1.2.md)
