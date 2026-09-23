# Changelog

All notable changes to this project are documented in this file.

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
This project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

> **Purge policy:** this file retains the latest 10 releases. Older entries
> remain available in `docs/releases/v<semver>.md`. `RELEASE_NOTES.md` keeps
> the latest three cuts.

## [Unreleased]

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

[Unreleased]: https://github.com/3leaps/storageprims/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/3leaps/storageprims/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/3leaps/storageprims/releases/tag/v0.1.0
