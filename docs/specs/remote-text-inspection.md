# Remote Text Inspection Spec

**Status:** Draft
**Date:** 2026-03-16
**Audience:** storageprims, datarakt, future control-plane consumers

## Purpose

Define how remote inspection commands such as `head`, `tail`, and `mid` should behave when built
on top of storageprims byte primitives.

This spec does **not** change storageprims into a text-oriented library. It describes how higher
layers should consume byte streams and byte windows safely.

## Scope

In scope:

- text-vs-binary classification expectations
- byte-window and stream-based inspection patterns
- newline handling (`LF`, `CRLF`, `CR`)
- limitations for compressed and multi-byte encodings
- helper API concepts for reusable framing logic

Out of scope:

- full CSV/PSV parser semantics
- schema inference
- transcoding
- query execution

## Storage Primitive Assumptions

The underlying storage layer provides:

- `head(uri)` -> metadata
- `get(uri)` -> full byte stream
- `get_range(uri, start, end)` -> byte window

All higher-level text inspection builds on those byte primitives.

## Classification Model

The initial classification model is intentionally small:

- `utf8`
- `utf16le`
- `utf16be`
- `binary_or_unknown`

### Detection guidance

- honor BOM when present
- otherwise use best-effort heuristics
- if confidence is low, classify as `binary_or_unknown`
- never silently pretend unknown binary is valid text

The classifier is advisory. Callers may still override encoding choice explicitly.

## Inspection Modes

### 1. Stream mode

Used when reading from `get`.

Flow:

1. Open byte stream
2. Read chunks incrementally
3. Classify/decode bytes
4. Frame lines or records incrementally
5. Stop once the command has enough data

Best for:

- `head`
- full sequential preview
- streaming inspection during relay workflows

### 2. Window mode

Used when reading from `get_range`.

Flow:

1. Request byte window
2. Classify/decode window
3. Frame partial lines/records
4. Expand or shift window if needed

Best for:

- sampling around an offset (`mid`)
- suffix inspection (`tail`) where feasible
- low-cost peeking on constrained networks

## Command Semantics

### `head`

Recommended meaning:

- return the first `N` logical records after optional decode/framing

Suggested algorithm:

1. call `head` for metadata
2. request an initial byte window or open a stream
3. classify encoding
4. frame records until `N` complete records are available
5. if insufficient, read more bytes

### `mid`

Recommended meaning:

- return a sample of logical records around a byte offset or sampling strategy chosen by the application

Suggested algorithm:

1. choose a byte offset/window
2. use `get_range`
3. decode and discard partial leading record
4. emit the next `N` complete records

Important note:

- `mid` is not a stable row-number seek unless the application has an index or row-offset map

### `tail`

Recommended meaning:

- return the last `N` logical records when feasible, with format-specific limitations documented clearly

Suggested algorithm:

1. inspect object size
2. request a suffix window with `get_range`
3. decode and frame records
4. if too few complete records are available, expand window backward and retry

Important note:

- `tail` may be expensive or impossible to do precisely for some compressed formats without full decode or an index

## Newline Handling

The framing layer should recognize:

- `LF` (`\n`)
- `CRLF` (`\r\n`)
- `CR` (`\r`)

Rules:

- normalize line endings for framing purposes
- preserve original bytes separately if exact reconstruction matters
- handle chunk/window boundaries where `\r` and `\n` may be split across reads

## Multi-Byte Encoding Caveats

The framing layer must expect boundary problems such as:

- incomplete UTF-8 sequences at the end of a chunk/window
- incomplete UTF-16 code units at the boundary
- BOM only in the first window, not later windows

Therefore:

- framing helpers must support carry-over buffers across reads
- window-based helpers may need overlap bytes before/after the requested region

## Compression Caveats

### Uncompressed text

`head`, `mid`, and `tail` are all feasible with enough byte-window logic.

### Gzip-like compressed text

- `head` is usually feasible from the start of the stream
- `mid` and `tail` are not generally cheap or precise without indexes or full/partial decompression strategy
- applications must document these limitations clearly

### Splittable compression or indexed formats

May allow better future support, but should be treated as format-specific enhancements above this base spec.

## Helper API Sketch

This is a conceptual helper API above storageprims, not a proposed storageprims core API.

```rust
pub enum EncodingGuess {
    Utf8,
    Utf16Le,
    Utf16Be,
    BinaryOrUnknown,
}

pub struct DetectionResult {
    pub encoding: EncodingGuess,
    pub has_bom: bool,
    pub confidence: f32,
}

pub fn detect_encoding(prefix: &[u8]) -> DetectionResult;

pub struct FramedLine {
    pub text: String,
    pub terminated: bool,
}

pub trait LineFramer {
    fn push(&mut self, bytes: &[u8]) -> Result<Vec<FramedLine>, FrameError>;
    fn finish(&mut self) -> Result<Vec<FramedLine>, FrameError>;
}

pub fn new_line_framer(encoding: EncodingGuess) -> Box<dyn LineFramer>;
```

Potential future extensions:

- byte-window helpers with overlap management
- row-framing adapters for delimited formats
- explicit caller override of encoding

## Design Guardrails

- storageprims remains byte-first
- helper logic may be shared, but not embedded into provider/FFI core contracts
- application commands must document limitations clearly, especially for compressed files and non-indexed mid/tail access
