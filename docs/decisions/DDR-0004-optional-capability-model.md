# DDR-0004: Optional Capability Model

> **Status**: Proposed
> **Date**: 2026-03-16
> **Authors**: entarch, devlead, ffiarch, Architecture Council

## Context

storageprims needs a minimum universal surface that is useful immediately for datarakt, while
still leaving room for gonimbus-scale listing behavior and future provider-specific strengths.

The risk is symmetrical:

- if the core trait becomes too large, every provider and binding pays for behavior that many
  consumers do not need immediately
- if the core trait is too small, downstream apps rebuild capability detection ad hoc and the
  library stops being useful

Existing consumer experience already shows this tension:

- `gonimbus` uses optional interfaces such as prefix/delimiter listing and multipart upload
- `datarakt` needs strong streaming primitives now, but not every advanced capability on day one
- `fulseed` benefits from a stable object-store core and should not be forced to think in terms
  of indexing-specific capabilities

## Design

### Overview

storageprims adopts a **core-plus-capabilities** model.

- A small universal contract is guaranteed across providers and bindings.
- Additional behaviors are modeled as optional capabilities.
- Capability presence is explicit and queryable.

### Core contract

The universal v0 contract remains:

- `list`
- `head`
- `get`
- `get_range`
- `put`
- `delete`
- `copy`

These operations are mandatory for any provider that claims general storageprims conformance.

### Optional capabilities

Capabilities beyond that minimum are modeled separately.

Initial candidates:

- delimiter/common-prefix listing
- multipart upload lifecycle
- provider-side write-probe helpers if ever standardized
- future bulk-delete or batch operations if introduced

Capabilities are optional because they are either:

- not required by every first-wave consumer
- not uniformly supported or implemented across providers
- advanced enough that they should not block the minimum useful contract

### Rust API model

In Rust, optional capabilities SHOULD be represented as separate traits rather than folded into
one monolithic `StorageProvider` trait.

Conceptually:

```rust
pub trait StorageProvider { /* core operations */ }

pub trait DelimiterLister: StorageProvider { /* delimiter/common-prefix listing */ }
pub trait MultipartUploader: StorageProvider { /* multipart lifecycle */ }
```

This keeps the core trait stable while allowing provider crates to implement advanced behavior
without expanding the minimum guarantee.

### Binding and FFI model

Bindings cannot rely on Rust trait assertions directly, so optional capabilities MUST also have
a binding-friendly representation.

storageprims SHOULD expose an explicit capability query surface, conceptually similar to:

- `capabilities()` returning a list/set of capability identifiers, or
- `has_capability(name)` returning a boolean

The binding contract should let Go and TypeScript answer questions like:

- does this provider support delimiter/common-prefix listing?
- does this provider support multipart upload?

without invoking an operation blindly and inferring from generic failure.

### Unsupported capability behavior

If a caller invokes an optional capability that is not available, the operation MUST fail with
`UnsupportedCapability` rather than a vague generic error.

This is important for:

- clean branching in bindings
- conformance tests
- rollout planning for retrofits such as gonimbus

### Scope boundary

Capabilities should still describe **storage primitives**, not app workflows.

Good capability examples:

- delimiter listing
- multipart upload lifecycle
- maybe future batch delete if it is truly a provider/storage primitive

Bad capability examples:

- transfer job execution
- resume checkpoint persistence
- path-scoped index-plan compilation
- content-aware routing and reflow

Those belong above storageprims.

## Trade-offs

### Pros

- Keeps the minimum interface useful and implementable.
- Matches the real needs of datarakt-first rollout and gonimbus retrofit.
- Gives bindings a clean way to expose feature detection.
- Prevents application workflows from being smuggled into the primitive contract.

### Cons

- Adds one more layer of API design and conformance testing.
- Requires explicit capability discovery in bindings and docs.
- Some consumers may still prefer convenience helpers above the raw capability model.

## Alternatives Considered

### Alternative 1: One large universal provider trait

Rejected.

This would overfit to the most demanding consumer and make the minimum contract harder to ship early.

### Alternative 2: No formal capability model, let each binding invent one

Rejected.

That would recreate cross-language drift and make parity planning harder.

### Alternative 3: Feature flags only

Rejected.

Compile-time feature flags are useful for building the library, but they do not replace runtime
capability discovery for bindings and multi-provider consumers.

## Implementation Notes

- The first optional capability to design concretely should probably be delimiter/common-prefix listing because gonimbus depends on it heavily.
- Multipart should follow closely because it is useful for large transfers and safe write-probe behavior.
- The capability identifier names used in JSON/FFI should be stable and intentionally small.

## References

- `docs/decisions/ADR-0001-canonical-core-contract-and-provider-neutral-surface.md`
- `docs/decisions/DDR-0003-list-pagination-semantics-and-continuation-token-contract.md`
- `/Users/davethompson/dev/3leaps/gonimbus/pkg/provider/capabilities.go`
- `/Users/davethompson/dev/3leaps/gonimbus/pkg/provider/delimiter.go`
- `/Users/davethompson/dev/3leaps/gonimbus/pkg/provider/provider.go`
- `/Users/davethompson/dev/fulmenhq/datarakt/README.md`
