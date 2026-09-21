# DDR-0008: Source-Guarded Read Contract

> **Status**: Accepted
> **Date**: 2026-09-21

## Decision

storageprims provides a Rust-only optional guarded-read extension. A caller
binds either a native version or a strong validator to one configured target
and logical key, then uses guarded HEAD, GET, or range GET. The provider either
enforces that selection and returns same-response evidence with the owned
reader, or returns a canonical refusal or failure; it must never silently fall
back to current-object bytes.

`NativeVersion` is 1–1024 printable ASCII bytes excluding `"`, `,`, and `\\`,
and rejects the case-insensitive `null` sentinel. `ValidatorMatch` is exactly
one strong quoted RFC 9110 opaque tag, 3–128 bytes. Both are validated before
provider I/O and redact opaque values in debug and errors.

S3 maps native versions to `VersionId` and validators to `If-Match` for guarded
HEAD, GET, and range GET. Guarded reads use a no-retry client so a retry cannot
lose the selector or range. There is no hidden reopen or stream stitching.

An un-ranged guarded GET accepts only a complete `200` response. An unsolicited
partial status or a `Content-Range` that does not prove the entire object is
`Io(InvalidData)` before a reader is exposed. Its receipt reports the completed
body window and same-response total when known; an empty selected object remains
a valid complete read.

For ranges, the response must have a matching `Content-Range` and content
length. Known totals permit clipping only at EOF; ranges starting at or beyond
EOF, including every range of an empty object, are invalid arguments. Unknown
totals cannot be clipped. Truncation and overlong bodies remain observable from
the returned `AsyncRead` as `UnexpectedEof` and `InvalidData` respectively.
Range receipts preserve the original requested inclusive window separately from
the provider-returned window after any proven EOF clip.

## Error mapping

- Invalid selectors and cross-target/key selection reuse: `InvalidArgument`
  before I/O.
- Missing extension or selector enforcement: `UnsupportedCapability`.
- Missing selected native version or selected delete marker: `NotFound`.
- A false supported validator: `Conflict(TokenMismatch)`.
- `AccessDenied` remains `AccessDenied`; it is not converted to `NotFound`.
- A malformed apparent success, including wrong identity or range metadata:
  `Io(InvalidData)`.

Provider response bodies and opaque selector values are never copied into
canonical error details.

## Compatibility

Existing `head`, `get`, and `get_range` remain current-object operations. The
extension is Rust-only in this delivery: the existing Unix ABI adds no function,
layout, or error number, and its capability and probe JSON intentionally omit
`guarded_read`.
