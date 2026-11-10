# storageprims-local

Root-contained local filesystem provider for `storageprims`.

Guides: [the local filesystem provider](https://github.com/3leaps/storageprims/blob/main/docs/guides/local-provider.md) (keys,
guarantees, limits, platforms), [testing with the local provider](https://github.com/3leaps/storageprims/blob/main/docs/guides/testing-with-the-local-provider.md),
and [writing location-independent code](https://github.com/3leaps/storageprims/blob/main/docs/guides/location-independent-code.md).

`LocalProvider::new(root)` opens and retains a non-inheritable directory handle
without following a symlink or reparse point on the root leaf itself (allowing
standard host symlinked ancestor prefixes such as `/tmp` -> `/private/tmp` on
Unix), resolving all subsequent reads, staged atomic writes, deletions, and
bounded lexical or `/` delimiter listings component-by-component relative to
opened directory handles without following symlinks or reparse points inside the
root. Listing continuation tokens are opaque and process-local, and encountering
any symlink, reparse point, or special file inside a scanned directory fails the
listing page (`StorageError::Other`) rather than skipping the entry. Once atomic
rename publication succeeds, any subsequent Unix parent-directory `fsync`
failure reports `CommitKnowledge::Committed` on admitted writes (`admit_put` /
`relay_copy`) and returns `StorageError::Io` on plain `put` without deleting the
published destination. Diagnostic strings and `Error::source()` chains omit the
configured host root path.

Licensed under MIT OR Apache-2.0.
