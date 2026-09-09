# ADR-0004: Error Taxonomy and Cross-Provider Mapping

> **Status**: Proposed
> **Date**: 2026-03-16
> **Authors**: entarch, devlead, deliverylead, Architecture Council

## Context

storageprims exists in large part to stop downstream consumers from re-solving provider
differences independently.

That matters most at the error boundary:

- `datarakt` needs predictable handling for read, range, copy, verify, and transfer failures
- `gonimbus` needs stable classification so large crawls and index builds can distinguish
  retryable conditions from target-specific gaps
- `fulseed` needs write/delete/verify flows that do not encode provider SDK details in the app
- Go bindings need a stable mapping that feels idiomatic without losing programmatic meaning

If storageprims leaks raw provider SDK errors, each consumer will drift again. If it
over-normalizes and throws away all context, the library becomes hard to debug and hard to
operate.

## Decision

storageprims adopts a **canonical, provider-neutral error taxonomy** with two layers:

1. **Stable semantic classes** that downstream consumers can match on
2. **Preserved provider context** for diagnostics and observability

### 1. Error taxonomy is semantic, not SDK-shaped

The public storageprims error model MUST classify failures by semantic meaning, not by
provider SDK type names.

The core taxonomy for v0.1 SHOULD include at least:

- `InvalidUri`
- `InvalidArgument`
- `NotFound`
- `ContainerNotFound` or equivalent bucket/container-level absence
- `AccessDenied`
- `InvalidCredentials`
- `Throttled`
- `ProviderUnavailable`
- `UnsupportedCapability`
- `Conflict` where operation semantics require it
- `Io`
- `Other`

Exact Rust type names may evolve in code review, but the semantic set and intent should remain stable.

### 2. Provider identity and operation context are preserved

Canonical errors MUST preserve enough structured context to support debugging and policy decisions.

Where applicable, errors should retain fields such as:

- provider identity
- operation name or operation class
- affected key/path/target
- bucket/container/account context where relevant
- retry-after or retryability hints when known

The goal is: consumers match on semantic class first, then inspect structured context when needed.

### 3. storageprims owns the mapping from provider SDK errors

Provider crates MUST map native SDK errors directly into storageprims errors.

storageprims will not delegate public error semantics to:

- provider SDK type hierarchies
- third-party abstraction crates
- binding-specific wrappers

This ensures the mapping is consistent across Rust, FFI, Go, and future TypeScript consumers.

### 4. FFI and binding layers use coarse error codes plus rich detail

The FFI surface MUST expose:

- a flat error-code layer for immediate programmatic branching
- rich detail through last-error state and/or structured JSON payloads

Binding layers should surface idiomatic language errors while preserving access to:

- canonical storageprims class/code
- message/detail
- structured context where useful

The stable contract is the canonical storageprims classification, not the exact provider message text.

### 5. Retry and partial-run policy remain above the library

storageprims classifies retry-relevant conditions, but does not decide higher-level workflow policy.

Examples:

- `Throttled` means the provider asked us to slow down; application retry/backoff orchestration still belongs above storageprims
- `ProviderUnavailable` means the backend/service path is down or unavailable; whether to retry a transfer job, skip a prefix, or fail a crawl is an application decision
- `AccessDenied` tells gonimbus/datarakt/fulseed what happened, but the decision to continue a larger workflow belongs in those applications

### 6. Missing-object and stale-state semantics should be explicit

storageprims MUST distinguish between at least:

- object/key not found
- bucket/container/account-level target not found
- stale or invalid pagination/range/request state when detectable

This matters because downstream consumers use those cases differently:

- datarakt may treat missing source objects and stale resume state differently
- gonimbus may mark a prefix partial rather than fatal
- fulseed may treat missing keys as idempotent delete success in specific operations

### 7. Unsupported behavior is first-class

storageprims MUST treat unsupported features as an explicit semantic class rather than folding
them into `Other`.

This is important for optional capabilities and provider parity planning. Consumers should be able
to tell the difference between:

- the provider or build does not support this capability
- the capability exists but the current request failed

## Consequences

### Positive

- Downstream apps get a stable programmatic error surface across providers.
- Go bindings can expose idiomatic errors without inventing their own taxonomy.
- Provider-specific diagnostics remain available without leaking SDK types as the contract.
- Optional capability handling becomes clearer once unsupported behavior is explicit.

### Negative

- Provider implementations must do careful mapping work instead of forwarding raw errors.
- Some provider-specific distinctions will still be collapsed into shared categories.
- The taxonomy needs disciplined maintenance as new operations and providers are added.

### Neutral

- Message wording may evolve without changing the canonical semantic contract.
- Some operation-specific error nuances will be refined by later DDRs.
- Bindings may add convenience subclasses/wrappers as long as the canonical class remains visible.

## Current Implementation

The core taxonomy and S3 mappings implement the first provider slice. The
Unix/POSIX FFI exposes coarse error codes and a last-error message, but does not
yet expose the proposed structured provider and operation context. Validation
across a second provider and a language binding is also outstanding, so this
record remains `Proposed`.

## Decision Points

This record should remain `Proposed` until:

- the first provider implementations exercise the agreed classes against real SDK errors
- not-found, credential, throttling, and conflict mappings are validated across at least S3 and one non-S3 provider
- the FFI and Go bindings consume the same canonical classes without inventing alternate taxonomies

## Alternatives Considered

### Alternative 1: Expose raw provider SDK errors through wrappers

Rejected.

That would recreate the exact fragmentation storageprims is intended to eliminate.

### Alternative 2: Flatten everything into strings plus one generic failure code

Rejected.

This would be too weak for automation, transfers, retries, and large-bucket workflows.

### Alternative 3: Over-normalize and discard provider context entirely

Rejected.

That would make production debugging and conformance work unnecessarily difficult.

## References

- `docs/decisions/ADR-0001-canonical-core-contract-and-provider-neutral-surface.md`
- `docs/decisions/ADR-0003-ffi-design-for-metadata-and-streaming-data-plane.md`
- `https://github.com/3leaps/sysprims/blob/main/docs/decisions/ADR-0008-error-handling.md`
- `https://github.com/3leaps/gonimbus/blob/main/docs/architecture/indexing.md`
- `https://github.com/3leaps/gonimbus/blob/main/docs/architecture/adr/ADR-0003-index-build-provider-capabilities.md`
- `https://github.com/fulmenhq/datarakt/blob/main/README.md`
- `https://github.com/fulmenhq/fulseed/blob/main/contracts/contracts.go`
