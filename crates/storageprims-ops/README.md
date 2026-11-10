# storageprims-ops

Provider-neutral operations built on the `storageprims-core` contracts:

- `relay_copy`: copy one object between two providers (for example local to
  S3) with an explicit size, buffer and time budget, and a receipt that says
  whether the destination write was committed. Works with both built-in
  providers. See
  [writing location-independent code](https://github.com/3leaps/storageprims/blob/main/docs/guides/location-independent-code.md).
- Line-oriented helpers (`head_lines`, `tail_lines`, `mid_lines`,
  `count_lines`) and byte previews (`preview_bytes`, `preview_bytes_selected`).
  These require a provider that can pin reads to an observed object version,
  such as `S3Provider`; the local provider does not offer them.

See the [storageprims repository](https://github.com/3leaps/storageprims) for
workspace documentation.

Licensed under MIT OR Apache-2.0.
