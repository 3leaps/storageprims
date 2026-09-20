# Release Notes

> **Purge policy:** this landing page keeps the latest three releases in
> reverse chronological order. Older cuts remain in `docs/releases/`.
> The signed GitHub payload is the matching per-cut file, not this page.

---

## v0.1.0 — 2026-09-09

The first storageprims release establishes provider-neutral Rust storage
contracts, an implemented S3 backend, and a Unix/POSIX C ABI.

### Highlights

- Core contracts cover provider construction, storage URIs, typed errors,
  listing, metadata, reads, ranged reads, writes, deletes, and copy.
- Reusable line-oriented operations inspect remote text without converting the
  storage layer into a text-only API. They do not impose hard total-byte,
  request, or elapsed-time budgets; callers must enforce their own byte limits.
  With `force_stream`, `mid` and `tail` operations may buffer the whole object,
  and compressed objects are not decompressed. This release does not provide
  revision-safe gets.
- The S3 implementation supports AWS S3 and compatible endpoints, including
  provider-enforced conditional writes and native same-provider copy.
- Configured root prefixes contain object operations and listing results.
- Callable capabilities report implemented behavior, including supported
  conditional operations and listing bounds.
- Provider probes use an S3 bucket-head request, and unsupported
  credential-file configuration fails during construction.
- The Unix/POSIX C ABI carries control data as JSON and streams object data
  over bounded descriptors and pipes.
- CI enforces version consistency, dependency policy, package checks, an exact
  Rust 1.94.1 toolchain, five native Rust targets, and Unix FFI coverage.
- GitHub releases contain unsigned native FFI archives and an SBOM; maintainers
  add checksum manifests and signatures through a local MFA workflow.

### Upgrade notes

- The workspace version is `0.1.0`.
- Rust crates are not published on crates.io in this release.
- Git consumers should pin the exact `v0.1.0` tag.
- The C ABI is available on Linux x86_64, Linux arm64, and macOS arm64.
- Windows C ABI and additional storage providers remain planned.

Full notes: [docs/releases/v0.1.0.md](docs/releases/v0.1.0.md)
