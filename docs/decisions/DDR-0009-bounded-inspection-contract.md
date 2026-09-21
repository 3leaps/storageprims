# DDR-0009: Bounded Inspection Contract

> **Status**: Accepted
> **Date**: 2026-09-21

## Decision

The Rust operations crate provides a guarded `preview_bytes` helper for a
selected first-N byte prefix. It observes once, binds the returned STGP-006
native-version selector when available (otherwise its validator selector), and
uses a guarded range request. It never falls back to an unguarded current-object
read. `preview_bytes_selected` accepts an already-bound selection and does not
observe or rebind it.

`preview_bytes` means the first N bytes or a range response proven clipped at
object EOF. It does not mean that the entire object was downloaded. A body that
ends before its validated range, wrong/missing range evidence, source change,
or lack of guarded-read support fails rather than returning a partial success.
An empty observation is confirmed by guarded HEAD before a successful empty
preview.

Every bounded operation uses one ledger created before its first provider call:

| Dimension          |          Default |                     Hard cap |
| ------------------ | ---------------: | ---------------------------: |
| Payload bytes      |            8 MiB |                       64 MiB |
| Helper requests    |               16 |                           64 |
| Returned output    |            1 MiB |                        8 MiB |
| Pending line bytes |            1 MiB |                        8 MiB |
| Requested lines    |              256 |                       10,000 |
| Chunk / probe      | 256 KiB / 32 KiB |                        1 MiB |
| Wall time          |       30 seconds | finite positive caller value |

Invalid zero, overflow-prone, or over-cap values fail before allocation or I/O.
The ledger charges helper requests before dispatch and admitted body/output bytes
against one monotonic deadline. Provider retries and transport prefetch remain
outside this logical helper count and cannot be represented as an exact egress
limit.

Bounded inspection failures are `StorageError::Inspection`: resource exhaustion
uses `LimitExceeded` with a dimension and configured/consumed counters; text or
encoding refusal uses `EncodingRejected`. The Unix numeric code remains
`Other = 99`; its existing last-error JSON receives only versioned inspection
kind, limit dimension, configured limit, and consumed counters. It never
contains selectors, validators, bodies, or snippets.

## Compatibility

This delivery adds no C function, ABI layout, FFI error number, capability
advertisement, release version, or binding. Existing current-object operations
remain distinct from selected bounded preview. `LineOptions` remains structurally
unchanged; `InspectionBudget` is additive for bounded APIs.

`force_stream` remains a request to use byte streaming, not decompression.
Callers must not infer transparent decompression from it.
