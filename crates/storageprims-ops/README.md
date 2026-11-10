# storageprims-ops

Provider-neutral operations built on the `storageprims-core` contracts:

- `relay_copy`: bounded single-object transfer between two providers (the
  source needs `read_responses()` and the destination `admitted_puts()`; both
  built-in providers qualify), with
  an explicit size, buffer and time budget and a commit-knowledge receipt. See
  [writing location-independent code](https://github.com/3leaps/storageprims/blob/main/docs/guides/location-independent-code.md).
- Line-oriented helpers (`head_lines`, `tail_lines`, `mid_lines`,
  `count_lines`) and byte previews (`preview_bytes`, `preview_bytes_selected`).
  These require a provider with guarded reads, such as `S3Provider`; the local
  provider does not offer them.

See the [storageprims repository](https://github.com/3leaps/storageprims) for
workspace documentation.

Licensed under MIT OR Apache-2.0.
