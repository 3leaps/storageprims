# storageprims

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE-MIT)
[![Rust: 1.94.1](https://img.shields.io/badge/Rust-1.94.1-orange.svg)](https://www.rust-lang.org)

**Provider-neutral Rust storage primitives with an implemented S3 backend and a
Unix/POSIX C ABI.**

## The Problem

Cloud storage access is reimplemented independently across multiple repositories and organizations. Each implementation re-solves the same problems: auth chain resolution, error taxonomy, retry behavior, streaming I/O, credential management, provider-specific quirks. When a bug is fixed in one place, it stays broken in others.

## The Solution

storageprims provides a uniform Rust interface for cloud object storage
operations. The current workspace implements the core contracts, reusable
line-oriented operations, an S3 backend, and a Unix/POSIX C ABI. Additional
providers and language bindings are planned.

Part of the **3leaps prims family**:

| Library          | Domain                                         |
| ---------------- | ---------------------------------------------- |
| **sysprims**     | Process control (timeout, signals, inspection) |
| **ipcprims**     | IPC channels (async peers, schema validation)  |
| **docprims**     | Document handling                              |
| **storageprims** | Cloud object storage                           |

## Implementation Status

| Surface                                                     | Status                        |
| ----------------------------------------------------------- | ----------------------------- |
| Rust core contracts and line-oriented operations            | Implemented                   |
| AWS S3 and S3-compatible provider                           | Implemented                   |
| List, head, get, range get, put, delete, and S3-native copy | Implemented                   |
| Provider-enforced conditional put                           | Implemented                   |
| Root-prefix containment for listing and object operations   | Implemented                   |
| C ABI control JSON and descriptor/pipe streaming            | Implemented on Unix/POSIX     |
| Portable Rust crates on Windows                             | Implemented and native-tested |
| Windows C ABI                                               | Planned                       |
| Go and TypeScript bindings                                  | Planned                       |
| GCS, Azure Blob, local filesystem, and CLI crates           | Planned                       |
| Multipart upload and delimiter/common-prefix listing        | Planned                       |

## Quick Start (Rust)

```rust
use storageprims_core::{
    CredentialSource, ListOptions, ProviderConfig, ProviderKind, StorageProvider, StorageUri,
    TargetConfig,
};
use storageprims_s3::S3Provider;

let uri = StorageUri::parse("s3://my-bucket/data/")?;
let config = ProviderConfig {
    provider: ProviderKind::S3,
    target: TargetConfig::default(),
    credentials: CredentialSource::DefaultChain,
};
let provider = S3Provider::from_uri(&uri, config).await?;

let result = provider.list(ListOptions::default()).await?;
println!("{} objects", result.objects.len());
```

The complete, compile-checked version is
[`crates/storageprims-s3/examples/list.rs`](crates/storageprims-s3/examples/list.rs).
It uses the AWS SDK default credential chain; no credentials are passed in
arguments or source code.

### S3 authentication

The default credential chain is the recommended mode. A profile is also
supported as a non-secret selector. Explicit in-memory credential and
environment-based modes are supported for process-isolated callers, but are
caller-risk channels and are not the default happy path. The S3 provider does
not support credential-file configuration and rejects that mode during
construction.

Credential values, selectors, and environment details are omitted from normal
diagnostics. See
[DDR-0006](docs/decisions/DDR-0006-provider-configuration-surface-and-credential-representation.md)
and
[SDR-0001](docs/decisions/SDR-0001-credential-boundary-and-redaction-policy.md)
for the proposed configuration and credential-safety model.

## Architecture

storageprims uses a layered workspace architecture:

```
storageprims-core          Core traits, error types, URI parsing
        │
        ├── storageprims-ops    Provider-neutral line operations
        ├── storageprims-s3     AWS S3 and S3-compatible provider
        └── storageprims-ffi    Unix/POSIX C ABI and runtime
```

### FFI Design

- **Control plane**: JSON strings for metadata operations and coarse error
  codes with last-error messages
- **Data plane**: bounded OS pipes/file descriptors for get, range get, and put

Structured provider and operation context in FFI errors remains planned.

## Platform Support

| Platform          | Target                    | Portable Rust crates | C ABI         |
| ----------------- | ------------------------- | -------------------- | ------------- |
| Linux x64 glibc   | x86_64-unknown-linux-gnu  | Native-tested        | Native-tested |
| Linux arm64 glibc | aarch64-unknown-linux-gnu | Native-tested        | Native-tested |
| macOS arm64       | aarch64-apple-darwin      | Native-tested        | Native-tested |
| Windows x64       | x86_64-pc-windows-msvc    | Native-tested        | Planned       |
| Windows arm64     | aarch64-pc-windows-msvc   | Native-tested        | Planned       |

## Development

```bash
make bootstrap    # Install tools, including cbindgen for the C ABI
make check        # Run all quality checks (fmt, lint, test, deny)
make build        # Build all crates
make test         # Run tests
make test-integration-s3  # Run provider integration via cargo nextest
```

Provider follow-on work should use the reusable hardening checklist in [docs/provider-hardening-checklist.md](docs/provider-hardening-checklist.md).

## Releases

The first release is distributed as signed GitHub assets and can be pinned
directly from git. The Rust crates are not published on crates.io in this cut.

```toml
storageprims-core = { git = "https://github.com/3leaps/storageprims", tag = "v0.1.0" }
storageprims-ops = { git = "https://github.com/3leaps/storageprims", tag = "v0.1.0" }
storageprims-s3 = { git = "https://github.com/3leaps/storageprims", tag = "v0.1.0" }
```

Each GitHub release includes SHA-256 and SHA-512 manifests, minisign
signatures, and the public verification key. After downloading one release's
assets into an otherwise empty directory:

```bash
minisign -Vm SHA256SUMS -p storageprims-minisign.pub
minisign -Vm SHA512SUMS -p storageprims-minisign.pub
shasum -a 256 -c SHA256SUMS
shasum -a 512 -c SHA512SUMS
```

Release history and operator-facing verification details are in
[RELEASE_NOTES.md](RELEASE_NOTES.md) and
[RELEASE_CHECKLIST.md](RELEASE_CHECKLIST.md).

## Quality Gates

- `cargo fmt --check` — zero-diff formatting
- `cargo clippy -- -Dwarnings` — zero warnings
- `cargo test` — all tests pass
- `cargo deny check` — license and advisory compliance

## Supply Chain

- **License**: MIT OR Apache-2.0 (dual, permissive)
- **License compliance**: `cargo deny` enforces no GPL/LGPL/AGPL dependencies
- **Security audit**: `cargo audit` for known vulnerabilities
- **MSRV**: 1.94.1. The MSRV follows the minimum required by the pinned AWS SDK
  and tooling line and may be higher than some sibling crates.

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.

## Contributing

See [AGENTS.md](AGENTS.md) for development protocols and commit attribution requirements.
