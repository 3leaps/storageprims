# DDR-0007: Binary-First Remote Text Inspection and Helper Boundary

> **Status**: Proposed
> **Date**: 2026-03-16
> **Authors**: entarch, deliverylead, Architecture Council

## Context

storageprims is being shaped first for datarakt, where remote inspection of very large flat files
is a primary use case.

Representative pressures include:

- multi-GB PSV/CSV files stored remotely
- bandwidth-constrained or metered developer connections
- need for `head`, `tail`, sampling, and limited inspection without full download
- need to distinguish likely text files from binary blobs before higher-level parsing

This creates an architectural question:

- should storageprims itself provide text-style reads and delimiter framing?
- or should it remain byte-oriented and let higher layers frame records?

There is also a practical reuse concern: if encoding detection and line-framing stay entirely in
applications, the same chunk-boundary and line-ending logic may be reimplemented repeatedly.

## Design

### Overview

storageprims remains **binary-first**.

- Core storageprims primitives operate on bytes and metadata.
- Text detection, decoding, and record framing happen above the core storage layer.
- A shared helper layer is allowed and likely useful, but it must remain outside storageprims core.

### 1. Core primitives stay byte-oriented

The core launch primitives remain:

- `head`
- `get`
- `get_range`
- `put`
- `copy`

These primitives MUST NOT promise text-aware semantics such as:

- line reads
- record reads
- automatic delimiter detection
- encoding-normalized string output

Their job is to return metadata and raw bytes.

### 2. Text inspection starts from bytes

Any remote text inspection flow should follow this pattern:

1. Obtain metadata and/or an initial byte window
2. Classify the content as likely text or binary/unknown
3. If text-like, decode bytes using a best-effort encoding decision
4. Frame lines or records above the byte stream/window
5. If needed, request more bytes and continue framing

This applies equally to:

- stream-based inspection from `get`
- window-based inspection from `get_range`

### 3. Encoding detection is best-effort, not authoritative

Higher layers may perform best-effort encoding classification, but the system must acknowledge that
this process is heuristic.

The initial useful categories are:

- UTF-8
- UTF-16 LE
- UTF-16 BE
- binary / unknown

Additional legacy encodings may be added later if real use cases demand them, but they are not part
of the initial storageprims contract.

### 4. Shared helper layer is allowed above storageprims core

To avoid duplicated implementation across datarakt and future tools, a reusable helper layer may be
introduced above storageprims.

That helper may provide:

- likely text-vs-binary classification
- best-effort encoding detection
- LF / CRLF / CR normalization utilities
- incremental line framing across chunk boundaries
- byte-window framing helpers that tolerate partial boundaries

But it MUST NOT redefine the core storage contract away from bytes-first primitives.

### 5. Record semantics remain application-level

Even with a shared helper layer, record-aware behaviors remain application-level because they depend
on format semantics:

- CSV/PSV quoting and escaping
- JSONL rules
- compression wrappers
- row-oriented sampling policy
- tail behavior for compressed or indexed formats

Therefore:

- storageprims core stays byte-oriented
- helper layer may help with decoding and line framing
- datarakt or a format-aware engine still owns record parsing and inspection semantics

### 6. Range reads are byte windows, not safe text boundaries

`get_range` provides raw byte windows.

Callers must still handle:

- partial multi-byte code units
- partial line endings
- partial rows
- quoting/escaping boundaries

This is a feature, not a flaw: it keeps storageprims honest and general-purpose.

## Trade-offs

### Pros

- Keeps storageprims cleanly scoped as a storage primitive library.
- Still enables datarakt-style remote inspection without full downloads.
- Allows reuse of tricky encoding/framing logic without polluting provider/FFI contracts.
- Works for both stream-based and range-based inspection.

### Cons

- Applications still need a higher-level inspection layer.
- Best-effort encoding detection may be wrong or ambiguous.
- Some users may expect line-aware behavior from the storage layer and need documentation to reset expectations.

## Alternatives Considered

### Alternative 1: Add text-aware line reads to storageprims core

Rejected.

That would mix content semantics into the storage primitive and create hard-to-generalize behavior around encodings, delimiters, quoting, and compression.

### Alternative 2: Leave all decoding/framing logic entirely to applications

Partially rejected.

The core premise remains correct, but a small shared helper layer is justified if it prevents repeated implementations of the same binary-to-text boundary logic.

## Implementation Notes

- The first shared helper, if built, should focus on encoding classification and incremental line framing only.
- CSV/PSV parser semantics should stay outside that helper initially.
- Datarakt should be the proving ground before any helper is generalized for wider reuse.

## References

- `.plans/bootstrap/datarakt-driver.md`
- `docs/decisions/ADR-0001-canonical-core-contract-and-provider-neutral-surface.md`
- `docs/decisions/ADR-0003-ffi-design-for-metadata-and-streaming-data-plane.md`
- `docs/decisions/DDR-0006-provider-configuration-surface-and-credential-representation.md`
- `/Users/davethompson/dev/fulmenhq/datarakt/README.md`
