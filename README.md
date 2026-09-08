# storageprims

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE-MIT)
[![Rust: 1.94+](https://img.shields.io/badge/Rust-1.94+-orange.svg)](https://www.rust-lang.org)

**Uniform, cross-language cloud storage primitives — one Rust implementation, consumed everywhere.**

## The Problem

Cloud storage access is reimplemented independently across multiple repositories and organizations. Each implementation re-solves the same problems: auth chain resolution, error taxonomy, retry behavior, streaming I/O, credential management, provider-specific quirks. When a bug is fixed in one place, it stays broken in others.

## The Solution

storageprims provides a uniform interface for cloud object storage operations, implemented once in Rust and consumed via native crate dependencies or cross-language bindings (Go, TypeScript).

Part of the **3leaps prims family**:

| Library          | Domain                                         |
| ---------------- | ---------------------------------------------- |
| **sysprims**     | Process control (timeout, signals, inspection) |
| **ipcprims**     | IPC channels (async peers, schema validation)  |
| **docprims**     | Document handling                              |
| **storageprims** | Cloud object storage                           |

## Providers

| Provider                 | URI Scheme                     | Auth                                      |
| ------------------------ | ------------------------------ | ----------------------------------------- |
| AWS S3 (+ S3-compatible) | `s3://bucket/key`              | AWS SDK default chain, profiles, explicit |
| Google Cloud Storage     | `gs://bucket/object`           | Application Default Credentials, SA keys  |
| Azure Blob Storage       | `azb://account/container/blob` | DefaultAzureCredential, storage keys      |
| Local filesystem         | `file:///path`                 | OS permissions                            |

## Operations

- **List** objects with prefix filtering and pagination
- **Head** object metadata (size, ETag, content type, last modified)
- **Get** object content — full download or byte-range requests
- **Put** object content — streaming upload with metadata
- **Delete** objects
- **Copy** objects within or across providers
- **Multipart** uploads for large objects

## Quick Start (Rust)

```rust
use storageprims_core::{StorageProvider, StorageUri};
use storageprims_s3::S3Provider;

let uri = StorageUri::parse("s3://my-bucket/data/")?;
let provider = S3Provider::from_uri(&uri, Default::default())?;

// List objects
let result = provider.list(ListOptions {
    prefix: Some("data/"),
    ..Default::default()
}).await?;

// Get object
let response = provider.get("data/export.csv").await?;
```

## Quick Start (Go)

```go
import "github.com/3leaps/storageprims/bindings/go/storageprims"

provider, err := storageprims.Open("s3://my-bucket/data/")
defer provider.Close()

// Control plane — metadata
meta, err := provider.Head("data/export.csv")

// Data plane — streaming via OS pipe
reader, err := provider.GetStream("data/export.csv")
defer reader.Close()
// reader is an io.ReadCloser backed by an OS pipe — native Go I/O
```

## Architecture

storageprims uses a layered workspace architecture:

```
storageprims-core          Core traits, error types, URI parsing
storageprims-s3            AWS S3 provider (aws-sdk-rust)
storageprims-gcs           GCS provider (google-cloud-rust)
storageprims-azb           Azure Blob provider (azure_storage_blobs)
storageprims-local         Local filesystem provider
storageprims-cli           Diagnostic CLI
storageprims-ffi           C-ABI exports + runtime management
```

### FFI Design

- **Control plane** (metadata): JSON over FFI boundary (list, head, delete)
- **Data plane** (streaming): OS pipes/file descriptors (get, put) — no FFI overhead for bulk data

## Platform Support

| Platform          | Target                    | Status  |
| ----------------- | ------------------------- | ------- |
| Linux x64 glibc   | x86_64-unknown-linux-gnu  | Planned |
| Linux arm64 glibc | aarch64-unknown-linux-gnu | Planned |
| macOS arm64       | aarch64-apple-darwin      | Planned |
| Windows x64       | x86_64-pc-windows-msvc    | Planned |

## Development

```bash
make bootstrap    # Install tools (sfetch -> goneat, cargo-deny, cargo-audit, cargo-nextest)
make check        # Run all quality checks (fmt, lint, test, deny)
make build        # Build all crates
make test         # Run tests
make test-integration-s3  # Run provider integration via cargo nextest
```

Provider follow-on work should use the reusable hardening checklist in [docs/provider-hardening-checklist.md](docs/provider-hardening-checklist.md).

## Quality Gates

- `cargo fmt --check` — zero-diff formatting
- `cargo clippy -- -Dwarnings` — zero warnings
- `cargo test` — all tests pass
- `cargo deny check` — license and advisory compliance

## Supply Chain

- **License**: MIT OR Apache-2.0 (dual, permissive)
- **License compliance**: `cargo deny` enforces no GPL/LGPL/AGPL dependencies
- **Security audit**: `cargo audit` for known vulnerabilities
- **MSRV**: 1.94.1, aligned with the pinned AWS SDK and development toolchain

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT License ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

## Contributing

See [AGENTS.md](AGENTS.md) for development protocols and commit attribution requirements.
