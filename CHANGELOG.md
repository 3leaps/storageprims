# Changelog

All notable changes to this project are documented in this file.

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
This project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

> **Purge policy:** this file retains the latest 10 releases. Older entries
> remain available in `docs/releases/v<semver>.md`. `RELEASE_NOTES.md` keeps
> the latest three cuts.

## [Unreleased]

- Add `storageprims-local`, a rooted local filesystem provider with
  handle-relative no-follow containment, atomic overwrite and create-only
  conditional writes, bounded lexical flat and `"/"` delimiter listing, and
  receipt/admitted-write hooks.
- Add optional Rust `ReadResponseProvider` and `AdmittedPutProvider` extension
  traits, `OperationControl` cancellation and deadline hooks, and
  `storageprims-ops::relay::relay_copy` for bounded single-object streaming
  transfer with truthful `CommitKnowledge`.
- Extend `S3Provider` with `ReadResponseProvider` (reporting
  `Evidence::Unavailable` when `x-amz-missing-meta` is nonzero),
  `AdmittedPutProvider` (`max_attempts = 1` and final-fragment withholding),
  and `DelimiterListingProvider::supports_delimiter`.
- Enforce the 5 GiB single-`PutObject` ceiling (`S3_MAX_SINGLE_PUT_BYTES`),
  US-ASCII header field values, and case-insensitively unique user metadata
  names across S3 `put` and `admit_put`; omit empty S3 metadata maps, reject
  empty and slash-only S3 keys (`"/"`), and project caller-relative keys
  (omitting `root_prefix`) in S3 errors and `CopyResult`.
- Release CI runs with read-only permissions and publishes only workflow
  artifacts. The maintainer ceremony stages the artifacts of the verified tag
  run, creates the draft release, and uploads signatures to that draft after
  checking its assets byte for byte. Release public keys are copied from the
  tagged commit, and `docs/security/` documents the verification pins.

## [0.1.2] - 2026-09-28

- Add guarded crates.io publication tooling and package checks for the Rust
  workspace, including ordered dependency publishing and registry verification.
- Add signed-tag verification guards, a public OpenPGP signing key pin, and
  fingerprint anchors for the public OpenPGP and minisign keys.
- Add public-source reference checks and clarify the maintainer release
  checklist.
- Configure explicit S3 client features without changing the public storage
  contracts.

## [0.1.1] - 2026-09-22

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

## [0.1.0] - 2026-09-09

- Add provider-neutral storage contracts, line-oriented operations, and an
  AWS S3 and S3-compatible backend
- Add list, head, get, range get, put, delete, and S3-native copy operations
- Contain object operations and listing beneath configured root prefixes
- Add provider-enforced conditional put behavior and callable capability
  reporting, including bounded listing support
- Make S3 probing and configuration behavior explicit, including refusal of
  unsupported credential-file configuration
- Add a Unix/POSIX C ABI with JSON control messages and descriptor/pipe
  streaming
- Establish reproducible version, dependency, MSRV 1.94.1, package, and native
  CI gates
- Add unsigned native FFI release artifacts and a local MFA signing and
  verification workflow

[Unreleased]: https://github.com/3leaps/storageprims/compare/v0.1.2...HEAD
[0.1.2]: https://github.com/3leaps/storageprims/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/3leaps/storageprims/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/3leaps/storageprims/releases/tag/v0.1.0
