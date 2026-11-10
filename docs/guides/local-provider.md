# The local filesystem provider

`storageprims-local` stores objects as files under one directory, through the
same `StorageProvider` contract as the S3 provider. Use it to keep data on
local disk, to stage files before they go to a bucket, or to test storage code
without a network (see [Testing with the local provider](testing-with-the-local-provider.md)).

`storageprims-local` is first published with storageprims 0.2.0. Until that
release, depend on the repository:

```toml
[dependencies]
storageprims-core = { git = "https://github.com/3leaps/storageprims" }
storageprims-local = { git = "https://github.com/3leaps/storageprims" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

After the release, use `storageprims-core = "0.2"` and
`storageprims-local = "0.2"`. Tests that use `#[tokio::test]` need `tokio` with
`macros` and `rt` (or `rt-multi-thread`) as a dev-dependency.

## Opening a root

```rust
use storageprims_local::LocalProvider;

let store = LocalProvider::new("/var/lib/myapp/objects")?;
```

The root directory must already exist. `LocalProvider::new` opens it and keeps
the directory handle; every later operation resolves keys relative to that
handle. The final component of the root path must not itself be a symlink
(or a Windows reparse point); symlinks earlier in the path, such as macOS
`/tmp -> /private/tmp`, are fine. `LocalProvider` is `Clone`, and clones share
the open root.

A key maps to a path under the root: `reports/2026/summary.txt` is the file
`<root>/reports/2026/summary.txt`. Parent directories are created as needed on
write.

The full round trip, compiled by CI, is
[`examples/local_roundtrip.rs`](../../crates/storageprims-local/examples/local_roundtrip.rs).

## Keys

Local keys are stricter than S3 keys. A key must:

- be 1 to 1024 bytes of UTF-8, without a NUL byte;
- not start with `/`, and not contain empty, `.` or `..` segments (so no
  trailing `/` and no `a//b`);
- not use a segment beginning with `.storageprims-tmp` in any letter case,
  which is reserved for the provider's staging files;
- on Windows, use only segments Windows can represent: no segment may end in
  `.` or a space, contain a control character or any of `\ : < > " | ? *`, or be
  a reserved device name such as `CON`, `NUL`, `COM1` or `LPT1` (with or without
  an extension).

If data might move between operating systems, follow the Windows rules
everywhere.

Anything else is refused with `StorageError::InvalidArgument` before the
filesystem is touched. Code that should run unchanged against S3 and local
storage should use keys that satisfy these rules.

## What it guarantees

**Containment.** Every operation stays inside the root. Paths are resolved one
component at a time from the opened root handle without following symlinks or
reparse points, so a symlink inside the root cannot redirect a read or write
elsewhere. A key that reaches a symlink, a reparse point, or a special file
(device, socket, FIFO) is refused. A listing that meets one fails the request
with `StorageError::Other` rather than silently skipping it; so does a listing
that meets a file name that is not valid UTF-8 or would make a key longer than
1024 bytes.

**Atomic writes.** `put` writes to a staging file in the destination directory,
syncs it, and renames it into place. Readers see either the old object or the
complete new one, never a partial file.

**Create-only writes.** `PutPrecondition::MustNotExist` uses the operating
system's no-replace rename (macOS, Linux and Android, and Windows), so two
writers racing for the same key cannot both succeed. The loser gets
`StorageError::Conflict { kind: ConflictKind::AlreadyExists, .. }`. If the
underlying filesystem does not support a no-replace rename, the write fails
with `ProviderUnavailable` rather than falling back to check-then-rename. On
other Unix systems the provider does not advertise `Capability::ConditionalPut`
and refuses `MustNotExist` with `UnsupportedCapability` instead of emulating it.

**Diagnostics.** Error messages and error sources do not contain the host root
path.

## What it does not do

| Feature                                | Local behaviour                                                                                                                                                              |
| -------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `PutPrecondition::Match`               | Refused with `InvalidArgument`; the provider has no object validators                                                                                                        |
| `content_type`, user `metadata` on put | Refused with `InvalidArgument`, not silently dropped                                                                                                                         |
| ETags                                  | `etag` is always `None` in `head`, `list` and `put` results                                                                                                                  |
| Guarded reads                          | Not available (`guarded_reads()` is `None`). The `storageprims-ops` line and preview helpers need guarded reads, so they return `UnsupportedCapability` for a local provider |
| Delimiters                             | Only `"/"`; `supports_delimiter` is false for anything else                                                                                                                  |
| Multipart upload                       | Not needed and not offered                                                                                                                                                   |

## Listing

- Results are ordered by the bytes of the key, the same order S3 uses. There is
  no Unicode normalization or case folding.
- `max_keys` of `None` or `0` means 1000; values above 1000 are refused.
- Each `list` or `list_delimited` call may examine at most 100,000 directory
  entries and run for at most 10 seconds. A request that would exceed either
  limit fails with `StorageError::Inspection` rather than returning a partial
  page. Every entry in each directory the walk opens counts, including entries
  that do not match the prefix, so a single directory with more than 100,000
  entries cannot be listed even with a narrow prefix. Spread keys across
  `/`-separated segments to keep each directory well below that.
- Continuation tokens only work in the process that issued them, and only for
  the same request. Do not persist them.
- With the `"/"` delimiter, directories appear as common prefixes, including
  empty ones. `delete` removes only the file, so after the last object under
  `reports/2026/` is deleted, `reports/2026/` is still reported; parent
  directories created by an aborted write remain too. S3 reports a prefix only
  while some object key begins with it.

## Durability and partial failure

On Unix, after the rename the provider also syncs the parent directory. On
Windows it syncs the staged file before the rename but does not sync the
directory entry, so a power loss immediately after a successful `put` is less
strictly covered than on Unix.

If something fails after the rename has already published the object (for
example the Unix directory sync), plain `put` returns an error (normally `Io`)
even though the new object is visible. Writes through `admitted_puts()` or `relay_copy`
report `CommitKnowledge::Committed` in that case. See
[Commit knowledge](how-the-libraries-fit.md#commit-knowledge).

## Who may write to the root

The local provider assumes the processes writing under a root cooperate. It
protects against mistakes, such as stray symlinks and racing create-only writers,
not against a hostile writer with direct access to the same directory. In
particular, a hard link planted inside the root by another writer is not
detected. If untrusted parties can write to the directory, give each consumer
its own access-controlled root.

## Platforms

| Platform                  | Status                                                                                                       |
| ------------------------- | ------------------------------------------------------------------------------------------------------------ |
| Linux (x86_64, aarch64)   | Supported, tested in CI                                                                                      |
| macOS (aarch64)           | Supported, tested in CI                                                                                      |
| Windows (x86_64, aarch64) | Supported, tested in CI; directory-entry durability as above                                                 |
| Other Unix                | Untested; expected to build, with create-only `put` unavailable                                              |
| Other platforms           | `LocalProvider::new` returns `ProviderUnavailable`, and `LocalProvider` does not implement `StorageProvider` |

The C ABI (`storageprims-ffi`) does not expose the local provider.

## Further reading

[DDR-0012](../decisions/DDR-0012-local-provider.md) records the full design,
including the listing algorithm and the Windows implementation details.
