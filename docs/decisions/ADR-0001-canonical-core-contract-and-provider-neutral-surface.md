# ADR-0001: Canonical Core Contract and Provider-Neutral Surface

> **Status**: Proposed
> **Date**: 2026-03-16
> **Authors**: entarch, deliverylead, Architecture Council

## Context

storageprims is intended to become the shared object-storage primitive across multiple
repositories and languages:

- Rust-native consumers such as lanyte storage-voy
- Go consumers such as datarakt, gonimbus, and fulseed
- Future TypeScript consumers via bindings

The project is still in bootstrap, which is the best time to lock the canonical core
contract before provider implementations and bindings harden accidental choices.

Early scaffold review surfaced several contract risks:

1. `storageprims-core` already has duplicated provider identity concepts
   (`ProviderKind` and `ProviderType`), which will drift if allowed to persist.
2. The current `StorageUri` shape overloads `bucket` to mean different things for
   different providers, especially Azure account/container/blob targets.
3. The public docs describe a broad operation surface while the code has not yet
   frozen what is universally guaranteed versus optional.
4. Go bindings must arrive early for downstream consumers, so every Rust-facing
   contract choice now has immediate FFI and cross-language consequences.

An additional architectural question was whether storageprims should adopt the Apache
Arrow `object_store` crate as its primary abstraction. Review by entarch and
deliverylead concluded that doing so this early would weaken auth-chain control,
complicate error taxonomy ownership, and save less implementation effort than it first
appears.

## Decision

storageprims adopts a provider-neutral canonical core contract owned by
`storageprims-core`, and all provider crates, adapters, FFI surfaces, and language
bindings MUST conform to it.

### 1. One canonical provider identity

storageprims MUST expose a single public provider enum across:

- Rust library APIs
- Utility CLI and adapter-visible representations where provider identity is exposed
- FFI error codes and JSON payloads
- Binding-layer types in Go and TypeScript
- Documentation and examples

Provider identity MUST NOT be duplicated under separate public enum names with
overlapping meaning.

The canonical provider set for v0.1 is:

- `S3`
- `Gcs`
- `AzureBlob`
- `Local`

Exact Rust type naming is an implementation detail to be settled in code review, but
the project MUST converge on one enum and one serialized representation.

### 2. Provider-neutral target model

storageprims MUST use a parsed target model that preserves provider semantics without
pretending all backends are bucket/key stores.

The canonical model MUST distinguish at least:

- provider
- authority or account-level identifier
- container or bucket where applicable
- object key or filesystem path
- raw source URI

Azure Blob targets MUST preserve account and container distinctly. Local filesystem
targets MUST preserve path semantics distinctly from object-store semantics.

`StorageUri` may remain the parsing entry point, but its field model and serialized
representation MUST be provider-neutral and binding-friendly.

### 3. Minimum universal v0.1 operation surface

The guaranteed cross-provider contract for v0.1 is:

- `list`
- `head`
- `get`
- `get_range`
- `put`
- `delete`
- `copy`

These operations define the minimum portability promise across supported providers and
language bindings.

Capabilities beyond that minimum, including multipart upload support and any future
provider-specific accelerators, MUST be modeled as optional capability extensions
rather than folded into the universal contract.

### 4. Provider-native SDKs behind the canonical trait surface

storageprims will use provider-native SDKs behind its own trait and error contracts.

storageprims will NOT adopt `object_store` as the primary public or internal contract
layer for bootstrap or v0.1 planning.

Rationale:

- Native SDK credential chains align better with downstream consumer expectations.
- Error mapping should be owned directly by storageprims.
- The difficult delivery work is in FFI, streaming, bindings, and uniform semantics,
  not in avoiding thin provider adapters.

This decision does not forbid using `object_store` as an internal implementation aid in
some future provider crate, but only if:

- it does not change the public contract,
- it does not weaken auth-chain behavior,
- it does not obscure or distort error mapping.

### 5. Mechanism/policy boundary

storageprims provides storage mechanisms, not consumer policy.

storageprims owns:

- provider dispatch and URI parsing
- provider authentication integration through native SDK chains
- object metadata and byte-stream operations
- uniform error mapping
- FFI-safe control-plane and data-plane transport mechanisms

Consumers own:

- orchestration, transfer planning, and checkpointing
- delegation and write governance
- retry policy above the primitive boundary when business semantics require it
- application-specific authz or tenancy restrictions

storageprims MUST resist expanding into transfer orchestration, secrets management, or
consumer-specific workflow logic without a separate decision record.

### 6. Binding-first contract discipline

Because Go bindings are an early delivery requirement, core contract design MUST be
treated as cross-language API design from the start.

Therefore:

- JSON control-plane payloads are public contracts, not incidental serialization
- error variants and codes must be reviewable at the binding layer
- optional capabilities must have an intentional cross-language story before exposure

## Consequences

### Positive

- Locks the core contract before S3 and Go bindings encode accidental drift.
- Preserves control over credential-chain behavior for datarakt, gonimbus, and fulseed.
- Keeps Azure and local targets modeled honestly instead of forced into S3-shaped fields.
- Gives FFI and bindings a stable semantic foundation before implementation accelerates.

### Negative

- Requires early discipline and up-front decisions before provider implementation can move quickly.
- Pushes some design complexity into the canonical target model and capability boundaries.
- Leaves some implementation convenience on the table compared with adopting a third-party abstraction wholesale.

### Neutral

- Follow-on DDRs are still needed for pagination semantics, copy behavior, and config representation.
- Follow-on ADRs are still needed for FFI design, dependency governance, error taxonomy, and binding distribution.
- Existing bootstrap code will need a small refactor to align with this contract.

## Alternatives Considered

### Alternative 1: Use `object_store` as the primary abstraction

Rejected.

This would centralize provider access behind a third-party abstraction, but it would not
solve the hardest project risks: FFI design, streaming semantics, early Go binding
delivery, and owning a uniform error surface. It also introduces extra uncertainty in
credential-chain behavior and error mapping.

### Alternative 2: Let each provider crate define its own surface, then normalize later

Rejected.

This would move faster in the very short term, but it would create the most expensive
kind of rework: retrofitting provider crates, FFI JSON, and Go bindings after downstream
consumers have already started integrating.

### Alternative 3: Standardize on an S3-shaped contract and adapt other providers to it

Rejected.

This would simplify some early naming but would distort Azure Blob and local filesystem
semantics, making the contract less honest and harder to explain across languages.

## References

- `.plans/bootstrap/architecture.md`
- `.plans/bootstrap/integration.md`
- `.plans/bootstrap/adr-roadmap.md`
- `crates/storageprims-core/src/error.rs`
- `crates/storageprims-core/src/uri.rs`
- `/Users/davethompson/dev/3leaps/sysprims/docs/decisions/ADR-0004-ffi-design.md`
- `/Users/davethompson/dev/3leaps/sysprims/docs/decisions/ADR-0008-error-handling.md`
- `/Users/davethompson/dev/3leaps/sysprims/docs/decisions/ADR-0012-language-bindings-distribution.md`
- `/Users/davethompson/dev/3leaps/ipcprims/docs/decisions/DDR-0001-transport-scope.md`
- `/Users/davethompson/dev/3leaps/ipcprims/docs/decisions/SDR-0001-schema-validation-scope.md`
