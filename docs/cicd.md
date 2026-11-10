# CI/CD

## PR Closeout

`storageprims` uses `make pr-final` as the canonical local closeout gate before:

- opening a PR
- pushing follow-up commits to an open PR
- asking for final merge review

The target is intentionally built from existing repo checks so contributors do not
have to remember an ad hoc command list.

Tests and fixtures use independently invented synthetic values. Do not place
private paths, internal references, or identifying data in test inputs, expected
output, diagnostics, or deny lists. See [PDR-0003](decisions/PDR-0003-public-source-review.md)
and the [3 Leaps OSS Sensitive Local Data Policy](https://github.com/3leaps/oss-policies/blob/main/SENSITIVE-LOCAL-DATA.md).

## `make pr-final`

`make pr-final` currently runs:

- `make fmt`
- `make cbindgen`
- `make fmt-check`
- `make lint`
- `make test`
- `make deny`
- `make test-integration-s3`
- `make test-integration-ops`
- `make test-integration-ffi`

This sequence is designed to catch the common blind spot where formatting or
generated headers are stale locally even though narrower Rust-only checks pass.

## Notes

- `test-integration-s3`, `test-integration-ops`, and `test-integration-ffi` expect LocalStack to be
  available through `docker compose -f docker-compose.localstack.yml up -d localstack`.
- `make pr-final` is a local contributor gate. CI still runs its own workflow
  jobs and remains the source of truth for branch protection.
- If `make fmt` or `make cbindgen` changes the tree, those changes should be
  committed before pushing the PR update.

## Release workflow credentials

Every workflow declares `permissions: contents: read`, and no job persists
checkout credentials. The tag-triggered release workflow restores the
runner-local annotated tag with the read-only job token (the GitHub API answer
must agree with origin), verifies it against the public pin committed in the
tagged commit, and builds the FFI archives and SBOM from that commit. It
uploads them as workflow artifacts with a `release-identity` record of the tag
object and commit. It does not create, edit, or publish a GitHub release.

Binding the tag to `origin/main` is a maintainer step: the strict guard runs in
the local ceremony with the maintainer's own credentials. `make release`
stages the artifacts of the one successful push run of the tag workflow at the
verified commit, then creates the draft with a create-only request; an
existing release or draft, an unprovable absence, or a partial upload stops
for inspection without replacing anything. Before signatures are uploaded,
the draft's unsigned assets must equal the verified artifacts byte for byte.

After a tag workflow failure, stop and investigate before running
`make release`. Never recreate or move a published signed tag. See the
[release checklist](../RELEASE_CHECKLIST.md) for the distinct local-tag,
remote-push, and signing steps.
