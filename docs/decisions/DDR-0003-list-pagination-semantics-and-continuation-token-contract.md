# DDR-0003: List Pagination Semantics and Continuation-Token Contract

> **Status**: Approved
> **Date**: 2026-03-16
> **Authors**: entarch, devlead, Architecture Council

## Context

Listing is one of the most deceptively important parts of storageprims.

Different consumers pull the design in different directions:

- `datarakt` needs reliable batch enumeration for `ls`, transfer planning, and inspection
- `gonimbus` needs prefix-first listing that scales to extremely large buckets and can be
  resumed page-by-page without accidental semantic drift
- `fulseed` needs predictable list behavior for verify/cleanup workflows, but does not need
  indexing-specific orchestration inside the library

The canonical list contract needs to be useful for all three without embedding app-level
scoping or crawl orchestration into storageprims itself.

## Design

### Overview

storageprims defines `list` as a **literal-prefix, paginated object enumeration primitive**.

It is intentionally narrower than an application-level crawl engine:

- it lists objects under a literal prefix
- it paginates with opaque continuation tokens
- it returns provider-normalized object summaries
- it does not implement pattern expansion, sharding plans, checkpoint policy, or index builds

### API / Interface

Conceptual shape:

```rust
pub struct ListOptions {
    pub prefix: Option<String>,
    pub continuation_token: Option<String>,
    pub max_keys: Option<u32>,
}

pub struct ListResult {
    pub objects: Vec<ObjectSummary>,
    pub continuation_token: Option<String>,
    pub is_truncated: bool,
}
```

Delimiter/common-prefix discovery is intentionally left to separate capability design work.
This record covers the core paginated object-listing primitive only.

### Data Structures

Required object summary fields:

```rust
pub struct ObjectSummary {
    pub key: String,
    pub size: u64,
    pub last_modified: Option<DateTime<Utc>>,
    pub etag: Option<String>,
    pub content_type: Option<String>,
}
```

### Behavior

#### 1. Prefix is literal, not pattern-based

`prefix` is a literal provider prefix.

It is not:

- a glob
- a regex
- a recursive query language
- an application-level shard plan

Pattern derivation, prefix planning, and partition expansion belong in consumers such as
gonimbus, not in storageprims.

#### 2. Continuation tokens are opaque

`continuation_token` is an opaque provider-issued cursor.

storageprims MUST:

- treat the token as opaque data
- return it unchanged except for serialization/transport safety
- avoid parsing or inventing token structure at the public contract layer

Consumers MUST NOT assume token meaning beyond "resume the same listing request".

#### 3. Tokens are request-scoped, not durable checkpoints

Continuation tokens are valid only for the same logical request shape:

- same provider target
- same authority/container/bucket context
- same literal prefix
- same list mode and related options

Bindings and docs MUST describe tokens as **request-scoped cursors**, not as durable,
long-lived checkpoints suitable for arbitrary future replay.

If applications need durable checkpoint/resume semantics across restarts, that logic
belongs above storageprims.

#### 4. `max_keys` is an upper bound request, not an exact-count guarantee

`max_keys` requests an upper bound for page size.

Semantics:

- zero/omitted means provider or library default
- providers MUST omit a zero page-size value from requests rather than forwarding zero
- values that exceed a provider's page-size representation MUST fail as `InvalidArgument`
  instead of silently falling back to the provider default
- providers may return fewer objects than requested
- if more results remain, `is_truncated` MUST be `true`
- when more results remain, `continuation_token` MUST be present
- when no more results remain, `continuation_token` MUST be absent

Implementation conformance: the S3 provider normalizes `max_keys` before request construction,
omitting zero and rejecting values above `i32::MAX` locally as `InvalidArgument`.

#### 5. storageprims preserves provider-native page order

storageprims MUST NOT resort list pages to impose a synthetic global order.

Rationale:

- resorting adds cost on the hot path for large-bucket consumers
- resorting can obscure provider behavior and pagination boundaries
- consumers that need application-specific ordering can sort above the primitive layer

The contract promise is page-sequential resumability, not cross-provider canonical sort order.

#### 6. Mutation during listing is not hidden

storageprims does not promise snapshot isolation for object listings.

If the underlying bucket/container changes during enumeration, results may reflect provider
behavior, including object appearance/disappearance between pages.

Applications that require stable snapshots, deduplication, or reconciliation must implement
that policy above storageprims.

#### 7. Error behavior stays primitive-level

List errors must map into the canonical storageprims error taxonomy, but the `list` contract
itself remains primitive-level:

- authentication/authz errors are surfaced as uniform errors
- invalid or stale continuation tokens are surfaced as request errors
- throttling and provider-unavailable conditions are surfaced without embedding app-level retry policy

Whether a partial crawl continues after a per-prefix error is a consumer decision, not a
storageprims list semantic.

## Trade-offs

### Pros

- Keeps listing fast and honest for datarakt and future transfer planning.
- Preserves the performance characteristics large-bucket consumers care about.
- Avoids embedding crawl engines, shard planners, or index policies into the library.
- Leaves room for gonimbus-specific higher-level planning on top of a stable primitive.

### Cons

- Consumers wanting durable resume/checkpoint semantics must build them above the library.
- Delimiter/common-prefix semantics are defined in [DDR-0010: Delimiter Listing Contract](DDR-0010-delimiter-listing-contract.md).
- Different providers may still exhibit different real-world mutation behavior during long listings.

## Alternatives Considered

### Alternative 1: Pattern-aware listing in the core contract

Rejected.

This would pull match planning, wildcard semantics, and provider-cost strategy into the
primitive library, which is application behavior rather than storage access behavior.

### Alternative 2: Normalize continuation tokens into a storageprims-owned cursor format

Rejected.

This would create a false impression of durability and portability while forcing the
library to simulate provider pagination semantics it does not truly own.

### Alternative 3: Impose canonical sorted output across providers

Rejected.

That would add hot-path overhead for the largest consumers and blur the line between raw
enumeration and application-specific post-processing.

## Implementation Notes

- Go bindings should model continuation tokens as opaque strings with no helper parsing.
- CLI and API consumers built on storageprims should document that pagination resume requires
  replaying the same request shape.
- Delimiter/common-prefix support should be settled with the optional-capability decision,
  not smuggled into ad hoc list parameters.

## Decision Points

This record is `Approved` on current evidence.

Re-open or supersede it if:

- provider implementations expose a practical need for storageprims-owned durable cursors
- early consumers raise strong objections to request-scoped tokens or provider-native ordering
- launch use cases require storageprims itself to provide stronger pagination guarantees than this record allows

## References

- `docs/decisions/ADR-0001-canonical-core-contract-and-provider-neutral-surface.md`
- `https://github.com/3leaps/gonimbus/blob/main/docs/architecture/indexing.md`
- `https://github.com/3leaps/gonimbus/blob/main/docs/architecture/adr/ADR-0003-index-build-provider-capabilities.md`
- `https://github.com/3leaps/gonimbus/blob/main/pkg/provider/provider.go`
- `https://github.com/fulmenhq/fulseed/blob/main/contracts/contracts.go`
- `https://github.com/fulmenhq/datarakt/blob/main/README.md`
