# DDR-0010: Delimiter Listing Contract

> **Status**: Accepted-as-amended
> **Date**: 2026-09-21 (amended 2026-10-01)
> **Authors**: entarch, devlead, secrev

## Context

The universal `StorageProvider::list` operation is literal-prefix object
enumeration. Some Rust consumers also need one provider-native hierarchy level,
including both object rows and common prefixes, without turning the library into
a recursive bucket crawler.

## Decision

Delimiter listing is an optional Rust extension. `DelimiterListingProvider`
accepts an owned request containing a literal prefix, one required delimiter, an
opaque continuation token, and the existing page-size option. It returns one
native provider page containing separate object and common-prefix collections.
`StorageProvider::delimiter_lists` defaults to `None`, preserving existing
provider implementations and allowing object-safe discovery through
`&dyn StorageProvider`.

The universal `ListOptions` and `ListResult` types remain unchanged. The Unix C
ABI does not expose or advertise delimiter listing.

### Page semantics

- The provider performs exactly one native page request. The library does not
  refill, descend into returned prefixes, or emulate grouping by walking keys.
- Object order and common-prefix order are preserved independently. No combined
  interleaving is promised.
- `max_keys` is one upper bound shared by objects and common prefixes. Omitted
  or zero uses the provider default; S3 applies its default bound of 1000 both
  to the native request and whole-page validation.
- Continuation tokens are opaque and request-scoped. A continued request must
  replay the same target, literal prefix, delimiter, and page-size choice.
- Listing is not snapshot-isolated.

### Projection and validation

Configured roots use the existing `/` segment boundary, even when the listing
delimiter differs. The root is stripped exactly once. Returned strings remain
literal: no path normalization, separator trimming, percent-decoding, or
directory-marker inference occurs.

The entire page fails as `StorageError::Other` if an object key or common prefix
is missing, outside the configured root or requested literal prefix, if a common
prefix is empty after projection, if the same projected path appears in both
collections, if the combined row count exceeds an admitted bound, or if the
truncation flag is missing or its token pair is inconsistent. Malformed rows
are never filtered into a shorter successful page.

A provider-issued next token must also contain 1 through 8192 bytes before the
page can succeed. Malformed provider output fails as `Other`; replaying a valid
opaque token is byte-preserving, while an invalid caller-supplied token remains
`InvalidArgument` for `continuation_token`.

### Admission and errors

Before provider I/O:

- delimiters must contain 1 through 1024 UTF-8 bytes and no NUL, C0, or DEL;
- present continuation tokens must contain 1 through 8192 bytes;
- prefix and page-size validation use the existing list rules.

Request debug and display output redact continuation tokens. Errors do not echo
raw prefix, delimiter, or token values. Provider rejection of a continuation
token maps to `InvalidArgument` for `continuation_token`; the universal list
error mapping is unchanged.

### Amendment: Delimiter discovery and bounded local `/` directory paging

`DelimiterListingProvider` adds an advisory discovery method with a conservative
default:

```rust
fn supports_delimiter(&self, _delimiter: &str) -> bool {
    false
}
```

Unknown external support reports `false` without disabling direct `list_delimited`
calls on legacy implementations, and `list_delimited` still validates every
request directly. An S3 adapter overrides `supports_delimiter` to return `true`
for valid delimiters (`1..=1024` UTF-8 bytes with no NUL, C0, or DEL); this
contract record does not claim that a concrete S3 adapter exposes that override
prior to its extension update. `LocalProvider` overrides `supports_delimiter` to
return `true` only for `"/"` and rejects any other delimiter as
`StorageError::InvalidArgument` for `delimiter` before filesystem I/O.

Because a local directory has no server-side page cursor, `LocalProvider` implements
one `"/"` delimiter page as a bounded single-directory scan over the containing
directory before the final `"/"` in `prefix` (see
[DDR-0012](DDR-0012-local-provider.md)):

- `max_keys` omitted or `0` resolves to `1000`, explicit `1..=1000` is accepted,
  and values above `1000` fail with `InvalidArgument` before I/O (S3 `max_keys`
  acceptance and single-native-page behavior remain unchanged).
- Objects and `name/` directory common prefixes share `K` and a `K + 1` candidate
  selection pool in UTF-8 byte order while preserving separate sorted `objects` and
  `common_prefixes` output collections; no descent into subdirectories occurs.
- Native directories—including empty directories—appear in `common_prefixes` as
  explicit filesystem hierarchy entries rather than cloud zero-byte marker
  inference.
- Each request enforces the `100,000` examined-entry and `10,000`-millisecond
  monotonic deadline caps, failing with `StorageError::Inspection`
  (`InspectionKind::LimitExceeded`) rather than returning an unproven short page.

## Consequences

Rust callers can browse one provider-native prefix level through explicit,
callable discovery. Other providers remain source-compatible and return
`UnsupportedCapability` without I/O. A future non-Rust consumer requires a
separate additive ABI decision.

## References

- [DDR-0003: List Pagination Semantics and Continuation-Token Contract](DDR-0003-list-pagination-semantics-and-continuation-token-contract.md)
- [DDR-0004: Optional Capability Model](DDR-0004-optional-capability-model.md)
