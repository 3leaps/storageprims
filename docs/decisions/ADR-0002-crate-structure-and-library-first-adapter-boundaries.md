# ADR-0002: Crate Structure and Library-First Adapter Boundaries

> **Status**: Approved
> **Date**: 2026-03-16
> **Authors**: entarch, deliverylead, devlead, Architecture Council

## Context

storageprims is intended to be a foundational multi-language library, not a one-off Rust crate and
not an application disguised as a library.

Several pressures make crate structure a first-order architectural concern:

- the project must serve Rust-native consumers, Go bindings, and later TypeScript bindings
- datarakt needs a reliable storage foundation with clear CLI/control-plane parity above shared code
- storageprims itself should include a maintained utility CLI for diagnostics, smoke tests, and real
  operational use, but that CLI must not become the hidden source of truth
- teams routinely drift by putting too much logic in CLI adapters or control-plane handlers instead
  of the shared library, creating duplicate semantics and inconsistent behavior

Early workspace scaffold already hints at the desired shape, but the boundaries are not yet locked.

## Decision

storageprims adopts a workspace structure centered on a shared Rust library contract, with all
adapters treated as thin consumers of that shared implementation.

## Workspace Structure

### 1. Core contract crate

`crates/storageprims-core`

Owns:

- provider-neutral types and traits
- provider enum and parsed target model
- canonical error taxonomy
- shared operation contracts and result shapes

This crate is the semantic center of the repository.

### 2. Provider implementation crates

Planned provider crates:

- `crates/storageprims-s3`
- `crates/storageprims-gcs`
- `crates/storageprims-azb`
- `crates/storageprims-local`

Each provider crate owns provider-native SDK integration and mapping into the canonical core surface.

`storageprims-s3` is AWS S3 first. S3-compatible providers may be supported through explicit
endpoint/auth configuration, but that support must not degrade AWS-native defaults, auth chains,
or semantics in the core implementation.

Provider crates MUST depend on `storageprims-core`.
`storageprims-core` MUST NOT depend on provider crates.

### 3. FFI crate

`ffi/storageprims-ffi`

Owns:

- C-ABI exports
- runtime management for FFI consumers
- JSON-over-FFI control-plane surface
- stream endpoint bridging for data-plane operations

The FFI crate is an adapter over shared library behavior. It does not define independent storage semantics.

### 4. Maintained utility CLI

`crates/storageprims-cli`

storageprims will include a maintained utility CLI.

Its role is:

- provider diagnostics
- credential and endpoint smoke testing
- direct exercising of metadata, range, stream, and copy behavior
- practical operational use when a focused storage utility is sufficient

It is **not** the primary end-user workflow product for transfer/transcode/query use cases. Those belong in
applications such as datarakt.

### 5. Bindings

Planned binding surfaces:

- `bindings/go/storageprims`
- `bindings/typescript/storageprims` or equivalent native package layout

Bindings consume the FFI contract and should not redefine the core operation model.

## Library-First Boundary Rules

### 1. Shared library code is the source of truth

Provider access semantics, auth integration, URI parsing, error mapping, streaming behavior, and copy behavior
must live in shared Rust library code.

No adapter is allowed to become the hidden home of canonical behavior.

### 2. Adapters are thin

For storageprims, adapters include:

- `storageprims-cli`
- `storageprims-ffi`
- Go bindings
- TypeScript bindings
- future control-plane wrappers built by downstream applications

Adapter responsibilities are limited to:

- argument parsing
- request/response translation
- output formatting
- runtime/process wiring
- transport concerns such as CLI UX or control-plane serialization

Adapters MUST NOT own:

- provider-specific auth-chain logic
- independent error taxonomies
- alternate copy or range semantics
- separate URI parsing rules
- provider behavior implemented only in one adapter

### 3. DRY is an architectural rule, not just a style preference

If a feature is genuinely part of storage access semantics, the default design move is:

1. add or refine it in shared library code
2. expose it through adapters

Not the reverse.

This rule exists specifically to prevent drift between:

- utility CLI behavior
- FFI/binding behavior
- future control-plane behavior in consumers such as datarakt

### 4. Utility CLI does not create special semantics

`storageprims-cli` may provide helpful commands and output modes, but those commands must map to the same
shared library operations and semantics exposed elsewhere.

CLI-specific convenience is allowed for:

- flags and UX
- diagnostics presentation
- output formatting

CLI-only storage semantics are not allowed.

### 5. Control-plane parity comes from shared code, not duplicate implementations

storageprims does not itself need to become a control-plane application, but its design must support parity
for downstream control planes.

That means future control-plane consumers should be able to reuse the same library semantics that the utility
CLI and bindings already exercise.

Parity should come from shared implementation reuse, not from reimplementing storage behavior behind a REST API.

## Dependency Direction

The intended dependency flow is:

```text
storageprims-core
    ^
    |
provider crates
    ^
    |
storageprims-ffi      storageprims-cli
    ^
    |
Go / TypeScript bindings
```

Rules:

- `storageprims-core` has no provider-specific dependency
- provider crates do not depend on adapters
- adapters depend on shared crates, never the reverse
- bindings consume the FFI contract rather than bypassing it with duplicate provider logic

## Testing Consequences

The structure implies a testing strategy:

- semantic behavior is tested at library level first
- provider parity is tested across provider crates
- adapter tests verify translation and transport correctness
- utility CLI tests verify stdout/stderr discipline and command wiring, not alternate business logic

This is part of the anti-drift strategy.

## Consequences

### Positive

- Makes library-first intent enforceable instead of aspirational.
- Keeps utility CLI valuable without letting it become a semantic fork.
- Supports Go-first bindings and future TypeScript bindings cleanly.
- Reduces the risk of CLI/control-plane drift for datarakt and later consumers.

### Negative

- Forces some upfront discipline when it may feel faster to prototype in the CLI.
- Makes adapter code thinner and therefore less free to solve local convenience problems ad hoc.
- Increases pressure on core crate design quality because more surfaces depend on it.

## Decision Points

This record is `Approved` on current evidence.

Re-open or supersede it if:

- the workspace structure changes materially enough to alter crate or adapter boundaries
- adapter work starts introducing CLI-only or control-plane-only storage semantics
- day-to-day implementation shows the anti-drift rules are impractical or too restrictive

## Alternatives Considered

### Alternative 1: Treat CLI as the main implementation surface and expose library features opportunistically

Rejected.

That is exactly the drift pattern this ADR is intended to prevent.

### Alternative 2: Skip a maintained utility CLI and rely only on downstream applications

Rejected.

A maintained utility CLI is valuable as a test bed, diagnostic tool, and practical operator tool, as long as it stays thin.

### Alternative 3: Let bindings call provider crates directly

Rejected.

That would fragment the binding surface and duplicate adapter logic across languages.

## References

- `Cargo.toml`
- `docs/decisions/ADR-0001-canonical-core-contract-and-provider-neutral-surface.md`
- `docs/decisions/ADR-0003-ffi-design-for-metadata-and-streaming-data-plane.md`
- `https://github.com/3leaps/crucible/blob/main/docs/coding/baseline.md`
- `https://github.com/3leaps/crucible/blob/main/docs/coding/rust.md`
- `https://github.com/3leaps/crucible/blob/main/docs/coding/go.md`
- `https://github.com/3leaps/crucible/blob/main/docs/knowledge/testing/README.md`
- `https://github.com/3leaps/crucible/blob/main/docs/knowledge/toolchains/rust/ffi-bindings-setup.md`
- `https://github.com/3leaps/crucible/blob/main/docs/sop/stream-output.md`
