# How the libraries fit together

storageprims gives Rust code one object-storage contract and several providers
that implement it. Application code depends on the contract; the provider is
chosen once, where the program is configured.

## The crates

| Crate                       | What it contains                                                                                                      | Depend on it when                                                           |
| --------------------------- | --------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------- |
| `storageprims-core`         | The `StorageProvider` trait, optional capability traits, request and result types, `StorageUri`, `StorageError`       | Always. Library code that accepts "some storage" should depend only on this |
| `storageprims-s3`           | `S3Provider` for AWS S3 and S3-compatible endpoints                                                                   | The program talks to a bucket                                               |
| `storageprims-local`        | `LocalProvider`, a provider rooted at a local directory                                                               | The program stores objects on disk, or tests storage code without a bucket  |
| `storageprims-ops`          | Provider-neutral helpers: bounded single-object `relay_copy` between two providers, and line and byte preview helpers | You copy between providers or inspect text objects                          |
| `storageprims-ffi` (`ffi/`) | A C ABI over the S3 provider for Unix-like systems                                                                    | You call storageprims from another language. Not published to crates.io     |

```text
storageprims-core            contract, types, errors, URIs
   ├── storageprims-s3       S3Provider
   ├── storageprims-local    LocalProvider
   └── storageprims-ops      relay_copy, line and preview helpers
storageprims-ffi             C ABI (S3 only)
```

## The provider contract

Every provider implements `storageprims_core::StorageProvider`:

| Method                            | Does                                                                                                     |
| --------------------------------- | -------------------------------------------------------------------------------------------------------- |
| `put(key, body, options)`         | Write an object from a byte stream                                                                       |
| `get(key)` / `get_range(request)` | Read an object, or a byte range of it, as a stream                                                       |
| `head(key)`                       | Read object metadata (`size`, optional `etag`, `content_type`, `last_modified`)                          |
| `list(options)`                   | List objects under a prefix, one page at a time                                                          |
| `delete(key)`                     | Delete an object                                                                                         |
| `copy(request)`                   | Copy an object within the same provider                                                                  |
| `probe()`                         | Check that the configured container is reachable. It does not prove permission for individual operations |

Every method returns a boxed future, so the trait is object-safe: code can hold
`&dyn StorageProvider` or `Box<dyn StorageProvider>` and not know which provider
it has. Bodies are `BoxedByteStream`, a boxed `tokio::io::AsyncRead`; for bytes in
memory, `Box::new(std::io::Cursor::new(bytes))` is enough.

Keys are relative to the provider's configured root: the bucket and optional
`root_prefix` for S3, the root directory for the local provider.

### Listing and continuation

`list` returns one page. Keep calling it with the returned `continuation_token`
while `is_truncated` is true. Tokens are only meaningful for the same provider
and request; treat them as opaque. Pages are not a snapshot: objects written or
deleted between pages may or may not appear. `max_keys: None` or `Some(0)`
selects the provider default.

## Optional capabilities

Some operations exist on only some providers. There are two ways to discover
them, and portable code should check before relying on one.

**Capability flags.** `capabilities()` and `has_capability(Capability::…)`
report:

| Capability         | Meaning                                                                                                                                                                   |
| ------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ConditionalPut`   | `put` honours `PutPrecondition` atomically on the provider side. Which preconditions are accepted still varies (see below)                                                |
| `DelimiterListing` | Native delimiter (common-prefix) listing is available                                                                                                                     |
| `GuardedRead`      | Reads can be bound to a version or validator observed earlier                                                                                                             |
| `CredentialProbe`  | `probe()` is implemented (the trait default returns `UnsupportedCapability`). The local provider advertises it too: its probe checks that the root directory is reachable |
| `MultipartUpload`  | Reserved; no provider implements multipart upload yet                                                                                                                     |

**Extension hooks.** Methods on `StorageProvider` that return `Option<&dyn …>`:

| Hook                | Trait                      | Used for                                                              |
| ------------------- | -------------------------- | --------------------------------------------------------------------- |
| `delimiter_lists()` | `DelimiterListingProvider` | `list_delimited` with a delimiter; call `supports_delimiter(d)` first |
| `guarded_reads()`   | `GuardedReadProvider`      | Version- or validator-bound reads                                     |
| `read_responses()`  | `ReadResponseProvider`     | Whole-object reads that also report what the provider observed        |
| `admitted_puts()`   | `AdmittedPutProvider`      | Writes that report how far they got (see `CommitKnowledge` below)     |

`require_delimiter_listing(provider)` and
`require_guarded_reads(provider, operation)` return the extension or an
`UnsupportedCapability` error, which is the shortest way to fail clearly.

### Conditional writes

`PutOptions::precondition` takes a `PutPrecondition`:

- `None` (default): create or replace.
- `MustNotExist`: create only. If the key exists, `put` fails with
  `StorageError::Conflict { kind: ConflictKind::AlreadyExists, .. }`.
- `Match { token }`: replace only if the current object's validator equals
  `token`. A mismatch fails with `ConflictKind::TokenMismatch`.

`ConditionalPut` says the provider enforces preconditions atomically, not that
it accepts all of them. The local provider accepts `MustNotExist` but refuses
`Match`. `AdmittedPutProvider::put_support()` reports exactly which conditions a
provider accepts.

## Errors

Operations return `storageprims_core::Result<T>`, an alias for
`Result<T, StorageError>`. Match on the variant (or on `error.code()`, a
`StorageErrorCode`), never on the message text:

| Variant                              | Typical cause                                                              |
| ------------------------------------ | -------------------------------------------------------------------------- |
| `InvalidUri`, `InvalidArgument`      | The request was rejected before any storage was touched                    |
| `NotFound`, `ContainerNotFound`      | The object, or the bucket or root, does not exist                          |
| `AccessDenied`, `InvalidCredentials` | Authorization failed                                                       |
| `Throttled`, `ProviderUnavailable`   | Retry later, or the provider cannot serve this request here                |
| `UnsupportedCapability`              | The provider does not offer what was asked for                             |
| `Conflict`                           | A precondition failed; inspect `kind` (`AlreadyExists`, `TokenMismatch`)   |
| `Inspection`                         | A bounded operation hit one of its limits (bytes, requests, entries, time) |
| `Io`, `Other`                        | Lower-level failure                                                        |

`StorageError` is `#[non_exhaustive]`: include a wildcard arm when matching.
Error messages omit credentials and the local root path.

## Commit knowledge

A failed write does not always mean nothing was written. Writes made through
`admitted_puts()` and `storageprims_ops::relay_copy` report a `CommitKnowledge`
for the attempt:

| Value          | Meaning for this attempt                                                |
| -------------- | ----------------------------------------------------------------------- |
| `NotStarted`   | Nothing was sent to the destination                                     |
| `NotCommitted` | Data may have been sent, but the attempt did not publish an object      |
| `Committed`    | The object was published, even though a later step reported an error    |
| `Unknown`      | The outcome cannot be determined; check the destination before retrying |

It describes the attempt, not the destination. A non-committed attempt does not
prove the key is absent: another writer may have created it.

Plain `put` reports only success or a `StorageError`. On the local provider,
a `put` that fails after the file was already published (for example, the
directory sync after the rename failed on Unix) returns an `Io` error with the
new object visible.

## Where to go next

- [The local filesystem provider](local-provider.md)
- [Testing with the local provider](testing-with-the-local-provider.md)
- [Writing location-independent code](location-independent-code.md)
- Design records: [DDR-0004 optional capabilities](../decisions/DDR-0004-optional-capability-model.md),
  [DDR-0011 bounded relay](../decisions/DDR-0011-bounded-relay.md),
  [DDR-0012 local provider](../decisions/DDR-0012-local-provider.md),
  [DDR-0013 core extension contracts](../decisions/DDR-0013-core-extension-contracts.md)
