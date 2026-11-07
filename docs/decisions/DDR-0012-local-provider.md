# DDR-0012: Rooted Local Provider and Bounded Lexical Listing

> **Status**: Proposed
> **Date**: 2026-10-01
> **Authors**: entarch, devlead, devrev, secrev

## Context

Consumers that process both local directory batches and cloud object stores need a
single `StorageProvider` surface without maintaining private filesystem adapters or
pretending a local directory is an S3 service. A local filesystem backend has
different security, atomicity, metadata, and directory-enumeration properties from
remote object stores:

- path canonicalization does not prevent ancestor or symlink substitution races;
- in-place or same-directory file operations share native volume case, Unicode, and
  short-name aliasing semantics;
- directories have no server-side pagination cursor or object-store snapshot; and
- a plain directory without sidecar files does not persist `ETag`, `Content-Type`,
  or arbitrary user metadata.

## Decision

Implement `LocalProvider` in `storageprims-local`, depending on `storageprims-core`
and never on `storageprims-ops`. The Rust constructor is:

```rust
impl LocalProvider {
    pub fn new(root: impl AsRef<std::path::Path>) -> Result<Self>;
}
```

It opens and retains a root directory handle without cloud credentials. No local C
ABI constructor is exposed in this cut. `LocalProvider` advertises
`Capability::DelimiterListing`, `Capability::CredentialProbe`, and (on platforms
providing atomic no-replace rename: macOS, Linux/Android, and Windows)
`Capability::ConditionalPut`; it does not advertise `Capability::GuardedRead` or
`Capability::MultipartUpload`.

### Containment, namespace, and error redaction

- **Handle-relative no-follow traversal**: Root construction opens the configured
  directory without following symlinks or reparse points (`O_RDONLY | O_DIRECTORY |
O_NOFOLLOW | O_CLOEXEC` on Unix; non-inheritable directory handle opened with
  `FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT` and verified against
  reparse tags on Windows). Every subsequent lookup, directory creation, staging
  file creation, publication, deletion, and scan step resolves component-by-component
  relative to retained directory handles without following symlinks, junctions, or
  mount-point reparse tags—even when a link target stays inside the root. Platforms
  that cannot enforce handle-relative no-follow semantics fail closed.
- **Regular-file objects only**: Byte objects are regular disk files. Directories
  provide namespace hierarchy and are never readable or writable as byte objects.
  FIFOs, sockets, device nodes, symlinks, and reparse points are refused.
- **Key grammar and length bound**: Keys must contain `1..=1024` UTF-8 bytes
  (including `/` separators), with no leading or trailing `/`, no empty, `.`, or
  `..` segments, and no NUL bytes. On Windows, segments additionally reject `\`, `:`,
  `<`, `>`, `"`, `|`, `?`, `*`, control characters below `0x20`, trailing `.` or
  space, and reserved device base names (`CON`, `PRN`, `AUX`, `NUL`, `CONIN$`,
  `CONOUT$`, `COM1..9`/`¹²³`, `LPT1..9`/`¹²³`). `put` and `admit_put` reject keys
  exceeding 1024 UTF-8 bytes as `StorageError::InvalidArgument` before creating any
  parent directory or temporary file. Native per-segment length limits also refuse
  rather than truncating.
- **Path-free diagnostics**: `Debug` formatting, `ProbeResult`, receipts,
  continuation tokens, and every `StorageError` (including nested `Error::source()`
  causes) omit the absolute root path and raw OS path strings while preserving
  canonical error classes (`NotFound`, `AccessDenied`, `Conflict`, `InvalidArgument`,
  `Inspection`, `Io`, `Other`). `probe()` verifies retained root handle reachability
  and directory type with `CredentialSourceKind::None` and `endpoint: None`; as in
  core, a successful probe does not certify per-operation write authorization.

### Cooperating writers and temporary-file ownership

Local writes support application-owned roots and roots shared by cooperating
writers. Other actors may concurrently read, create, overwrite, or delete
destination objects across multiple provider instances or processes, provided they
leave provider-owned internal temporary entries and their contents untouched
throughout staging, publication, and cleanup. This excludes accidental as well as
deliberate renaming, unlinking, replacement, or modification of internal temporary
entries by another actor.

- **Same-directory staging and atomic publication**: `put`, `admit_put`, and
  in-root `copy` create an exclusive, non-inheritable temporary file with an
  unpredictable name and restrictive permissions (`0o600` on Unix) inside the
  already-opened destination parent directory. Collision on temporary creation
  refuses without adopting or deleting the existing entry. After full payload
  staging (and exact `content_length` verification when declared), the provider
  flushes and syncs the temporary file before publishing atomically on the opened
  parent directory handle:
  - unconditional overwrite (`PutPrecondition::None`) replaces atomically via
    handle-relative rename;
  - create-only (`PutPrecondition::MustNotExist`) uses an atomic single-winner
    no-replace rename on the opened parent handle (`renameatx_np(RENAME_EXCL)` on
    macOS, `renameat2(RENAME_NOREPLACE)` on Linux, handle-relative
    `SetFileInformationByHandle` with `ReplaceIfExists = false` on Windows), mapping
    destination collision to `StorageError::Conflict` with
    `ConflictKind::AlreadyExists` (checking existence before replacing is never an
    acceptable fallback);
  - `PutPrecondition::Match` and non-empty `content_type` or user `metadata` are
    refused without silent dropping or sidecar files.
- **Cleanup, durability, and commit knowledge**: Cleanup is confined strictly to the
  current attempt's owned temporary entry; unknown or stale temporary files are never
  automatically swept. Sync or write failure prior to rename unlinks the owned
  temporary entry and reports `CommitKnowledge::NotCommitted`. An indeterminate
  rename I/O failure reports `CommitKnowledge::Unknown` without unlinking. On Unix,
  the provider additionally calls `fsync` on the opened parent directory descriptor
  after atomic rename; on Windows, durability is bounded to syncing the staged file
  (`FlushFileBuffers`) prior to handle-relative atomic rename without calling
  `FlushFileBuffers` on the directory handle (which Win32 rejects on
  directory-mutation handles without `FILE_WRITE_DATA` or escalates to a volume-wide
  flush), making no stronger cross-platform post-rename directory-journal crash
  promise. Once atomic publication succeeds, any subsequent contracted post-rename
  step failure (such as Unix parent-directory `fsync`) or late `OperationControl`
  expiry/cancellation reports `CommitKnowledge::Committed`—never masquerading as
  rollback or deleting the published destination.
- **Reserved internal temporary namespace**: Segment names matching `.storageprims-tmp`
  (or `.storageprims-tmp*`) under supported native name equality—including ASCII and
  Unicode simple case folding, trailing dots/spaces, and Windows 8.3 short-name
  aliases—are reserved for provider staging. Direct-key operations reject caller
  keys in this namespace, and both flat and delimiter listing exclude matching
  entries (including stale entries left after an abrupt crash) while still charging
  visited native entries against the per-request scan ledger. This is an API
  namespace guarantee, not filesystem confidentiality against actors with direct
  host access.
- **Explicit boundary and revisit triggers**: This provider does not isolate
  mutually hostile directory writers, and same-volume hard links planted inside the
  root by another writer remain outside handle-relative symlink/reparse refusal.
  Deployments requiring hostile-writer isolation must provision access-controlled
  roots outside the library. This cooperating-writer model is a durable
  supported-use contract that remains in force until explicitly amended when a named
  consumer requires hostile shared-directory isolation, a portable or
  platform-specific mechanism demonstrably strengthens both publication and cleanup
  with operation-specific proof, or a defect violates the cooperating-writer
  contract.

### Receipt-bearing reads and admitted writes

- `ReadResponseProvider`: `read_support()` reports `ReadSupport::new(true, false, false)`.
  `ReadRequest::Guarded` is refused before any filesystem I/O with
  `ExtensionErrorKind::UnsupportedReadSelection`. `ReadRequest::Current` opens the
  regular file handle-relatively, inspects file size on that same descriptor, and
  returns `ReadEnforcement::Unguarded` with `SourceReceipt` (`native_version: None`,
  `validator: None`, `total_size: Some(size)`) and observed-absent metadata
  (`Evidence::Available(None)` and an empty `Evidence::Available` map). Same-handle
  size observation is not an immutable snapshot or content digest; concurrent
  in-place mutation by external tools can alter file bytes.
- `AdmittedPutProvider`: `put_support()` reports `WriteMode::SinglePart`,
  unconditional and create-only preconditions (`validator_match: false`), no
  `content_type` or `user_metadata` persistence, and `max_content_length: u64::MAX`
  (no artificial library ceiling beyond representation and filesystem capacity).
  `admit_put` performs pure semantic validation without filesystem I/O and returns a
  single-use `PreparedPut` bound to the retained root handle.

### Identity, order, and per-request listing bounds

Local `max_keys` absent or `0` resolves to `1000`; explicit values `1..=1000` are
accepted, and values above `1000` fail with `StorageError::InvalidArgument` before
I/O. (S3 `max_keys` acceptance and native paging remain unchanged.) Every local list
or delimiter-list call enforces a per-request work ledger of `100,000` examined
native entries (`InspectionLimitDimension::Entries`, `configured_limit = 100_000`)
and a `10`-second monotonic deadline (`InspectionLimitDimension::Deadline`,
`configured_limit = 10_000` milliseconds).

- **Byte ordering**: Results are ordered by root-relative UTF-8 bytes without
  Unicode normalization or case folding. Native case-insensitive volumes expose
  their actual enumerated entry names without fabricating distinct objects for case
  aliases.
- **Directory ordering in flat `list()`**: Flat `list()` emits regular-file object
  keys only. Directory branches are ordered by `name/`, not `name`, so a branch's
  entire descendant interval sorts at `name/` (yielding `a-b`, `a.txt`, `a/b`,
  `a0`). Literal `prefix` is resolved by opening complete directory components
  before the final `/` (returning an empty page when a non-link directory prefix is
  absent or names a regular file) and filtering the remaining partial segment; a
  directory branch is relevant iff its `key/` prefix and the requested `prefix`
  overlap (`key.starts_with(prefix) || prefix.starts_with(key)`).
- **Frozen bounded-window lexical DFS**:
  1. Maintain an output page and a shared candidate pool such that retained output
     entries plus buffered sibling candidates never exceed `K + 1` slots globally
     (`K` = resolved `max_keys`), rather than allocating `K`-sized heaps per depth.
  2. Scan one opened directory to EOF within the request ledger, retaining only the
     smallest eligible child labels after the frame's resume label that fit free
     pool slots (evicting the largest candidate when full and recording whether
     later eligible siblings existed).
  3. Consume the sorted window in UTF-8 byte order: files move into output slots;
     when encountering a directory, record whether later eligible siblings remain,
     clear buffered sibling candidates to return their slots to the shared pool, and
     descend handle-relatively.
  4. When a directory's window finishes and later eligible siblings existed, rescan
     that directory strictly after the last consumed child label. Every revisited,
     excluded, or non-matching entry counts against the `100,000`-entry ledger.
  5. Stop once `K + 1` matching files are found (returning the first `K` and a
     continuation token anchored at the `K`th key) or when traversal finishes with
     at most `K` files (`is_truncated: false`, `continuation_token: None`).
- **Navigation and descriptor bounds**: Active branch state stores path components
  once in a `<= 1024`-byte relative path buffer without imposing an artificial
  recursion depth cap below the 1024-byte key limit. Directories are reopened from
  the retained root/base handle per scan rather than holding open file descriptors
  for every ancestor depth.
- **Exact budget boundary and unrepresentable entries**: At `100,000` examined
  entries an EOF or proven `K + 1` page succeeds; a `100,001`st native entry needed
  to finish a scan is not retained and fails the request with
  `StorageError::Inspection` (`InspectionKind::LimitExceeded`,
  `limit_dimension: Some(InspectionLimitDimension::Entries)`,
  `configured_limit: 100_000`, `consumed: 100_001`)—never returning a short
  successful page. Blocking filesystem calls cannot be preempted portably; the
  monotonic deadline is checked around each step and rejects late completions.
  Encountering a non-UTF-8 name, unrepresentable segment, symlink/reparse point,
  special file, or root-relative key/prefix exceeding 1024 UTF-8 bytes during a
  scan counts against the examined ledger, is never retained, and fails with
  `StorageError::Other` (`provider: Some(ProviderKind::Local)`,
  `operation: Some(StorageOperation::List)`).
- **Opaque scoped continuation tokens**: Continuation tokens are versioned,
  `1..=8192` bytes, path-free, and scoped to `(root_identity, mode, prefix,
delimiter, resolved_max_keys)`. Omitted, `0`, and explicit `1000` resolve to the
  same page-size scope. Malformed, oversized, tampered, or cross-scope tokens fail
  as `StorageError::InvalidArgument` for `continuation_token` before walking.
  Resuming re-scans from the prefix base directory, skipping branches strictly
  before the cursor while descending into any ancestor branch containing the cursor
  key. Listing is not snapshot-isolated under concurrent mutation, and access errors
  are surfaced rather than masked as empty subtrees.

### Slash-only delimiter listing

`LocalProvider` implements `DelimiterListingProvider` for `"/"` only
(`supports_delimiter("/") == true`; any other delimiter fails as
`StorageError::InvalidArgument` for `delimiter` before filesystem I/O). One local
delimiter page opens the containing directory before the final `/` in `prefix`,
scans only that directory without descending, shares the `K` page bound and `K + 1`
lookahead pool across regular-file objects and `name/` directory common prefixes in
UTF-8 byte order, and emits separate sorted `objects` and `common_prefixes`
collections (including native empty directories as explicit filesystem prefixes, per
[DDR-0010](DDR-0010-delimiter-listing-contract.md)).

## References

- [ADR-0001: Canonical Core Contract and Provider-Neutral Surface](ADR-0001-canonical-core-contract-and-provider-neutral-surface.md)
- [DDR-0010: Delimiter Listing Contract](DDR-0010-delimiter-listing-contract.md)
- [DDR-0011: Bounded One-Object Relay](DDR-0011-bounded-relay.md)
- [DDR-0013: Receipt-Bearing Reads and Admitted Writes](DDR-0013-core-extension-contracts.md)
