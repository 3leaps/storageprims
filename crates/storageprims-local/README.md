# storageprims-local

Root-contained local filesystem provider for `storageprims`.

See the [storageprims repository](https://github.com/3leaps/storageprims) for
configuration, containment guarantees, and usage documentation.

`LocalProvider::new(root)` opens and retains a non-inheritable directory handle
without following symlinks or reparse points, resolving all subsequent reads,
staged atomic writes, deletions, and bounded lexical or `/` delimiter listings
component-by-component relative to opened directory handles. Diagnostic strings
and `Error::source()` chains omit the configured host root path.

Licensed under MIT OR Apache-2.0.
