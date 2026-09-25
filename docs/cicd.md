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
- `make test-integration-ffi`

This sequence is designed to catch the common blind spot where formatting or
generated headers are stale locally even though narrower Rust-only checks pass.

## Notes

- `test-integration-s3` and `test-integration-ffi` expect LocalStack to be
  available through `docker compose -f docker-compose.localstack.yml up -d localstack`.
- `make pr-final` is a local contributor gate. CI still runs its own workflow
  jobs and remains the source of truth for branch protection.
- If `make fmt` or `make cbindgen` changes the tree, those changes should be
  committed before pushing the PR update.

## Release workflow credentials

The tag-triggered release workflow creates the unsigned GitHub draft before the
local signing ceremony begins. `make release` requires that draft and stops at
`release-download` when it is absent.

The strict release guard fetches the release tag and `origin/main` to confirm
their exact relationship. The read-only validation job retains its read-scoped
checkout credential for that guard. The draft job has a write-scoped token, so
it provides an authenticated Git header only while running the guard and removes
that header before later draft steps create or update the release.

After a tag workflow failure, confirm the draft exists and its unsigned asset
inventory is complete before running `make release`. Repair the workflow and
recreate the annotated tag only when the failed workflow did not create a draft.
