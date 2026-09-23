---
id: "PDR-0001"
title: "Publish library crates after the release tag"
status: "accepted"
date: "2026-09-23"
deciders:
  - "@3leapsdave"
scope: "storageprims release process"
tags:
  - "process"
  - "release"
  - "crates-io"
relates-to:
  - "RELEASE_CHECKLIST.md"
---

# PDR-0001: Publish library crates after the release tag

## Decision

Publish opted-in library crates manually from a clean, guarded checkout
after the annotated release tag is on origin. The first registry version is
the cut that enables publication; older tags are not backfilled. Publish
the current version in the validated dependency order from
`config/release/publishable-crates.txt`, waiting for the crates.io index and
API after **each** upload, including the final crate. The core crate precedes
the S3 provider; the operations crate follows both because its tests depend
on the versioned S3 crate.

The maintainer alone holds a short-lived, crate-scoped token outside the
repository and explicitly authorizes each irreversible upload. CI has no
registry token or publish step. The FFI crate and any future demonstration
CLI are not registry packages. GitHub release asset archives remain
per-platform, with their inventory defined in `config/release/ffi-platforms.txt`.

## Consequences

Package checks on the first registry cut may need a rerun after dependent
crates reach the index. The GitHub draft is not signed until package checks
and the release workflow are green. `make release-crates-dry-run` exercises
each package using local patches for earlier unpublished dependencies; the
real upload has no patches and is run only by the maintainer. Future library
crates need an ordered list entry and an explicit `publish = true`; list
validation fails closed on mismatches and on unpublished FFI or CLI crates.
The index and API checks establish registry availability, name, version, and
unyanked status; they do not prove byte-for-byte identity with a local archive.
