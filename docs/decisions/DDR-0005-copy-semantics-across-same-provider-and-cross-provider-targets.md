# DDR-0005: Copy Semantics Across Same-Provider and Cross-Provider Targets

> **Status**: Approved
> **Date**: 2026-03-16
> **Authors**: entarch, deliverylead, devlead, Architecture Council

## Context

`copy` sits at an important boundary for storageprims.

For datarakt, object movement is a first-wave use case:

- some transfers are same-provider and may support efficient native copy
- many real transfers are cross-cloud and cross-account, where native server-side copy is unavailable
- downstream applications want one stable primitive, not provider-specific branching everywhere

At the same time, `copy` can easily become overloaded with application behavior:

- resume and checkpoint policy
- verify workflows
- compression or decompression during transfer
- format transcoding
- progress reporting and job orchestration

storageprims needs a copy semantic that is useful for datarakt but still clearly a storage primitive.

## Design

### Overview

storageprims defines `copy` as a **byte-preserving object movement primitive**.

The semantic goal is simple:

- move object content from source target to destination target
- preserve bytes exactly unless the operation fails
- choose the best available implementation strategy behind the contract

The contract does **not** include transformation, workflow policy, or transfer orchestration.

### 1. `copy` is byte-preserving

The `copy` primitive MUST preserve object bytes.

It does not:

- transcode formats
- recompress content
- decompress and rewrite content
- reinterpret text encodings

If the destination bytes differ from source bytes by design, that is not `copy`; it is an application-level transform pipeline.

### 2. Same-provider native copy is preferred when semantically valid

When source and destination are in the same provider domain and the provider offers a native copy path,
storageprims SHOULD prefer that path if it preserves the `copy` contract.

Important subtlety:

- same provider does not imply native copy is available
- cross-account, cross-project, cross-tenant, or cross-security-boundary copies often cannot use provider-native optimization even within one cloud
- in those cases, storageprims should treat the operation like any other non-native copy case and fall back to relay when possible

Examples:

- same-provider bucket/container object copy
- provider-native server-side copy across accounts when supported and authorized

This is an implementation optimization, not a distinct public API shape.

### 3. Cross-provider copy uses relay semantics

When native copy is not available, `copy` MUST be allowed to fall back to a generic relay implementation:

- open source as a read stream
- open destination as a write stream
- transfer bytes from source to destination
- surface terminal success/failure through the normal operation lifecycle

This relay path is expected to be the common denominator for cross-cloud and many cross-account cases.

### 4. Strategy choice is internal to storageprims

The caller should request `copy` by semantic intent, not by provider-specific transport details.

storageprims may internally choose:

- native provider copy
- relay copy
- future optimized relay variants

Bindings and consumers should not have to manually fork their main code path just to ask for ordinary object copy.

If a future need emerges for explicit strategy control, that should be a separate design record rather than silently changing the meaning of `copy`.

### 5. `copy` result should expose strategy and basic outcome metadata

Although the semantic contract is stable, the result should expose enough metadata for diagnostics and higher-level policy.

Useful result fields may include:

- strategy used (`native` or `relay`)
- source provider
- destination provider
- bytes copied when known
- destination etag/checksum when available

This helps datarakt and future applications report what happened without pushing workflow logic into storageprims.

### 6. Metadata behavior must be explicit

The initial `copy` contract should keep metadata semantics conservative.

Recommended baseline:

- preserve object bytes
- preserve basic object metadata only when the underlying strategy/provider can do so cleanly
- do not promise universal preservation of every provider-specific header/metadata field in v0

If metadata preservation rules need to become richer, they should be specified explicitly in a follow-on design record rather than assumed.

### 7. Retry, resume, verify, and progress remain above the library

storageprims `copy` may return errors with enough detail to support retry/verify workflows, but it does not own those workflows.

The following remain application responsibilities:

- checkpoint persistence
- resume policy
- post-copy verification policy
- progress bars and progress events
- job management and scheduling

This is especially important for datarakt, where transfer orchestration belongs in the application layer.

### 8. Transforming transfer is not `copy`

For clarity:

- source -> destination with identical bytes: `copy`
- source -> destination with compression, decompression, transcoding, or schema-aware rewrite: not `copy`

Those workflows may still use storageprims streams, but they are not part of the `copy` primitive.

## Trade-offs

### Pros

- Gives datarakt one stable object-movement primitive across providers.
- Lets storageprims optimize same-provider cases without exposing provider-specific APIs.
- Keeps cross-cloud relay as a first-class, honest path rather than a fallback hidden outside the library.
- Preserves a clear line between byte movement and content transformation.

### Cons

- Metadata preservation behavior may feel conservative in v0.
- Some consumers may want more explicit control over native vs relay strategy.
- Resume and verify logic still need to be implemented above the library.

## Alternatives Considered

### Alternative 1: No `copy` primitive, force applications to compose `get` + `put`

Rejected.

That would duplicate the same relay and lifecycle logic in every consumer and would waste the chance to centralize a core storage behavior.

### Alternative 2: Define `copy` only for same-provider native operations

Rejected.

That would make the public primitive much less useful for the actual datarakt launch cases, where cross-cloud movement is common.

### Alternative 3: Let `copy` include transcoding and compression options

Rejected.

That would collapse storage access and data transformation into one overloaded operation and blur the application/library boundary too early.

## Implementation Notes

- FFI and binding surfaces should be able to report whether `copy` used `native` or `relay` strategy.
- Relay copy should be implemented on the same streaming foundation as `get` and `put`.
- If native copy cannot meet the expected semantics, storageprims should fall back to relay rather than exposing provider-specific surprises as the contract.

## Decision Points

This record is `Approved` on current evidence.

Re-open or supersede it if:

- implementation shows that native copy and relay cannot share one clean public `copy` semantic
- callers need explicit strategy control in the public API to make copy workable
- launch consumers require transform-in-copy semantics from the storageprims core contract

## References

- `docs/decisions/ADR-0001-canonical-core-contract-and-provider-neutral-surface.md`
- `docs/decisions/ADR-0003-ffi-design-for-metadata-and-streaming-data-plane.md`
- `docs/decisions/ADR-0004-error-taxonomy-and-cross-provider-mapping.md`
- `docs/decisions/DDR-0004-optional-capability-model.md`
- `docs/decisions/DDR-0007-binary-first-remote-text-inspection-and-helper-boundary.md`
- `https://github.com/fulmenhq/datarakt/blob/main/README.md`
