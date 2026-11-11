# Release Notes

> **Purge policy:** this landing page keeps the latest three releases in
> reverse chronological order. Older cuts remain in `docs/releases/`.
> The signed GitHub payload is the matching per-cut file, not this page.

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
  case (the new `admit_put` applies the same rules). All S3 operations refuse
  slash-only keys such as `"/"`, as they already refused empty keys. S3 errors
  and `CopyResult` report keys relative to the configured root prefix.

### Local provider bounds

- Conditional writes: create-only (`MustNotExist`) on Linux, macOS, Android,
  and Windows; `Match` is not supported.
- Delimiter listing supports only `"/"`. Each listing request examines at most
  100,000 directory entries and runs for at most 10 seconds, counting every
  entry in each directory it opens; a request over either bound fails rather
  than returning a partial page.
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
---

## v0.1.1 — 2026-09-22

### Highlights

- Rust callers can bind a get, head, or range to a native version or a
  validator match. Metadata comes from that same response. A listing is not a
  snapshot. The Unix capability query does not advertise this.
- Inspection helpers enforce finite byte, request, output, and time limits.
  Crossing a limit fails the call. An inspection limit still returns numeric
  `Other`. For that error only, the Unix last-error JSON adds
  `inspection_version`, `inspection_kind`, `limit_dimension`,
  `configured_limit`, and `consumed`. Other errors omit those keys. This is
  not a new Unix capability.
- Rust callers can ask one provider page for objects and common prefixes. The
  library does not walk the bucket. The Unix capability query does not
  advertise this.

### Upgrade notes

- Crates are not on crates.io. Git consumers pin `v0.1.1` after the tag exists.

Full notes: [docs/releases/v0.1.1.md](docs/releases/v0.1.1.md)
