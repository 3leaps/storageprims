# DDR-0011: Bounded One-Object Relay

> **Status**: Accepted
> **Date**: 2026-10-01
> **Authors**: entarch, devlead, devrev, secrev

## Decision

The Rust operations API composes two supplied provider handles:

```rust
pub fn relay_copy<'a>(
    source: &'a dyn StorageProvider,
    destination: &'a dyn StorageProvider,
    request: RelayRequest,
    cancellation: Cancellation,
) -> FutureResult<'a, TransferReceipt, TransferFailure>;
```

The exact constructor/getter declarations are in
[`relay.rs`](../../crates/storageprims-ops/src/relay.rs). Read/admission contracts
are defined in [DDR-0013](DDR-0013-core-extension-contracts.md). Handles retain
independent configuration and credentials; relay constructs neither provider.
This operation transfers one whole object, not a tree, move, resume or native-copy
replacement. Existing `copy()` remains callable independently.

### Explicit request and resource budget

`RelayRequest::new` requires a `ReadRequest`, destination key, `PutPrecondition`,
`MetadataPolicy` and `TransferBudget`. It selects `SinglePart`; `with_write_mode`
is explicit. There is no default guard, destination predicate, metadata policy or
budget. `MetadataPolicy` is `None`, `ContentTypeOnly` or
`ContentTypeAndUserMetadata`. Unsupported requested preservation is refused before
opening, even if the source might have no metadata. Observed absent fields copy
nothing; unavailable requested fields refuse admission without an extra HEAD.

`TransferBudget::new(max_payload_bytes, buffer_bytes, timeout)` validates a payload
maximum in `0..=u64::MAX-1`, buffer size in `1..=1048576`, and a strictly positive,
representable timeout. Zero payload explicitly permits only empty objects. The
reserved counter byte records excess lookahead. There is no default budget or
unknown-length/multipart fallback. Length must be known and fit both the caller
budget and destination support/admission limits.

The helper payload buffer is caller-sized plus at most one lookahead byte,
including withheld final-fragment storage. Receipt strings, response metadata
maps and SDK buffers are separate: this is not a total-heap/RSS bound or new
metadata cap. Checked consumed/delivered counters are not persistence counters.

### Execution

1. Create one absolute deadline and refuse stopped control, missing optional
   hooks or unsupported selection/mode/predicate/metadata policy.
2. Open at most one selected read response; preserve its metadata and receipt.
   Opening can prefetch bytes: before exact admission the guarantee is zero body
   polls, not zero network traffic.
3. Validate length and requested metadata evidence; admit exact `PutOptions`
   before any source body poll. No target/option substitution is permitted.
4. Stream with backpressure through a validating reader. Retain the final fragment
   until an extra source poll proves EOF. Short/excess/source-error results never
   release an unvalidated final fragment; excess bytes are never delivered. For
   zero length, prove EOF before destination dispatch because a zero-length writer
   can acknowledge without polling its body.
5. Observe destination completion and source validation under the same control.
   Success requires validation and all declared bytes delivered, plus an observed
   successful destination outcome at an active-control checkpoint.

A writer reading exactly its declared length need not poll again; final-fragment
withholding supplies EOF proof before it can receive all bytes. Partial reads are
counted separately. Early acknowledgement without full delivery is
`LengthMismatch` with `Committed`, never success. Do not drain an abandoned body
to manufacture validation. There is at most one source open and one destination
execute, no helper HEAD, retry, plain-get downgrade or compensating delete.

### Terminal evidence and control precedence

`TransferReceipt` exposes selected source receipt/enforcement, destination
`PutResult`, metadata policy, write mode, consumed/delivered counters and
`CopyStrategy::Relay`. Only ops constructs transfer receipts and failures.
`TransferFailure` exposes typed kind, commit knowledge, optional source receipt,
counters and original optional `ExtensionError` cause. Its receipt `source()`
accessor is distinct from qualified `std::error::Error::source` linkage.

Missing hooks, source-open refusal and stops before execute are `NotStarted`.
After dispatch with no terminal provider evidence, use `Unknown`. Provider
failure retains its original `NotCommitted`, `Committed` or `Unknown` evidence and
canonical source chain through consuming decomposition. Reader/control failure
classification never reclassifies known commit into rollback or target absence.
Known destination success followed by validation/control refusal is a
`Committed` failure. Source-error/length evidence takes precedence over a generic
destination body-read error without erasing destination evidence.

Check control before/after source polls, before handing buffered bytes off, after
source-open/admission boundaries and on destination outcome observation. A
blocking call can finish late; inability to preempt it is not permission for late
timely success. A late `Ok` or cancellation latched before its observation yields
`Committed` `Deadline`/`Cancelled` failure. A cancel genuinely after the observed
timely validated checkpoint does not overturn success.

Bytes actually returned by a late source poll remain **consumed**, even if
permanently withheld. Delivered counts only bytes handed through the reader;
consumed can exceed delivered and includes a diagnostic excess byte. Dropping the
relay future returns no receipt and establishes no rollback. No non-success result
permits assuming destination absence. Canonical/nested error sanitization remains
the provider obligation in DDR-0013; relay errors omit raw body error text.

### Verification boundary

Required synthetic coverage includes exact-N/no-extra-poll writers, empty/no-poll writers, early
and partial outcomes, short/excess/source failures, preserved canonical causes,
guard/predicate/metadata admission, drop/cancellation races and blocking late polls.
These do not prove real filesystem containment, native-platform conformance or
SDK single-attempt behavior. Concrete adapters must enforce their own limits,
guards, atomic predicates, redaction and no-retry body handling. Integration of
independent S3 handles and local/S3 relay requires explicit CI/test selection; an
S3-only integration target does not automatically execute ops relay coverage.
