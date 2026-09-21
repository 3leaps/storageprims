# DDR-0004: Optional Capability Model

> **Status**: Accepted-as-amended
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
- A provider advertises a capability if and only if that behavior is callable through its
  current public surface.

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

storageprims exposes an explicit capability query surface as a list of stable identifiers.
The query MUST be deterministic and MUST NOT perform provider I/O, credential resolution, or
connectivity probes.

Rust providers expose `capabilities()` and `has_capability()`. Bindings use a standalone FFI
query that returns the same identifiers as a JSON array.

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

- Capability identifiers may exist before their extension traits, but providers MUST NOT advertise
  those identifiers until callers can invoke the corresponding behavior.
- Multipart upload extension traits remain future work. Delimiter listing is
  implemented by the additive Rust extension described in DDR-0010.
- Conditional put is the first advertised optional data-plane capability.
- The capability identifier names used in JSON/FFI should be stable and intentionally small.

## Acceptance Amendment

The capability model is accepted with the following amendment:

- advertisement means the capability is callable now, not merely planned or supported by the
  underlying storage service
- extension traits remain the preferred future Rust shape for advanced optional operations
- the FFI discovery surface returns only closed capability identifiers and performs no provider I/O

### Guarded-read surface amendment (v0.1.1)

`GuardedRead` is a Rust-only optional capability in the first delivery. It uses
the object-safe `GuardedReadProvider` extension and the default
`StorageProvider::guarded_reads()` discovery hook, so existing provider
implementations remain source-compatible and callers holding `&dyn
StorageProvider` can receive a canonical `UnsupportedCapability` refusal.

The Rust capability list may contain `guarded_read` only when that hook returns
`Some`. The existing Unix FFI capability query and FFI-serialized probe result
must project the list to ABI-callable identifiers and omit `guarded_read` until
an explicit FFI operation exists. This intentional difference is not an FFI
feature claim. Adding the Rust enum variant can require downstream exhaustive
`Capability` matches to add a case.

The extension provides an unguarded observation method and guarded HEAD, GET,
and range GET. Guarded operations require a bound native-version or
validator-match selector; observation followed by an unconditional legacy GET
does not enforce a source selection. A selection binds configured target,
logical key, selector kind, and opaque token. Credentials are not target
identity. Receipts report source evidence from the same response as the
metadata or body and distinguish total object size from requested and returned
byte windows.

### Delimiter-listing surface amendment

`DelimiterListing` is callable through an object-safe Rust extension and a
default `StorageProvider::delimiter_lists()` discovery hook. Existing provider
implementations remain source-compatible. The S3 provider advertises the
capability on its Rust surface and performs one native delimiter-list request;
it does not emulate grouping, refill a page, or descend into common prefixes.

The Unix FFI capability query and probe projection omit `delimiter_listing`
until an explicit FFI operation exists. The universal `ListOptions` and
`ListResult` contracts remain unchanged. See DDR-0010 for admission, page
integrity, projection, and continuation semantics.

## References

- `docs/decisions/ADR-0001-canonical-core-contract-and-provider-neutral-surface.md`
- `docs/decisions/DDR-0003-list-pagination-semantics-and-continuation-token-contract.md`
- `https://github.com/3leaps/gonimbus/blob/main/pkg/provider/capabilities.go`
- `https://github.com/3leaps/gonimbus/blob/main/pkg/provider/delimiter.go`
- `https://github.com/3leaps/gonimbus/blob/main/pkg/provider/provider.go`
- `https://github.com/fulmenhq/datarakt/blob/main/README.md`
