# Provider Hardening Checklist

Use this checklist when a first-pass provider implementation moves into hardening.
It is intended to be reused by S3, GCS, Azure Blob, and local-provider follow-on briefs.

## Contract Proof

- Prove canonical error mappings for provider-specific service errors with unit seams.
- Do not rely on emulator behavior alone for semantic assertions the emulator does not stably expose.
- Cover both happy-path and negative-path operations for `list`, `head`, `get`, `get_range`, `put`, `delete`, and `copy`.

## Credential Handling

- Test `CredentialSource::Env`, `Profile`, and `DefaultChain` under isolated env/profile state.
- Clear ambient `AWS_*` or provider-specific auth state before asserting credential selection.
- Add assertions that secret-bearing values are not echoed in surfaced configuration or credential errors.

## Streaming and Uploads

- Verify declared content length is enforced against the actual stream.
- Exercise boundary cases for short, exact, and overlong streams.
- If multipart or unknown-length uploads are advertised, prove that path explicitly or document it as deferred.

## Integration Coverage

- Exercise pagination with continuation tokens.
- Exercise range boundaries, including first-byte, last-byte, whole-object, and invalid-range cases.
- Cover missing-object, missing-container, and provider-unavailable failures.
- Document emulator limitations instead of weakening canonical assertions.

## Retry and Backoff

- Preserve structured throttle signals such as `Retry-After` when the SDK exposes them.
- Add unit coverage for throttled responses with and without retry metadata.

## Runner and CI

- Use `cargo nextest` as the default runner for provider integration lanes.
- Keep the normal workspace test lane on `cargo test` unless there is a specific reason to change it.
- Codify the integration pattern in Make and CI so later providers reuse the same lane shape.

## Review Notes

- Prefer safe simplifications before introducing `unsafe` in stream/body adapters.
- Record any deferred items or emulator limitations explicitly in the follow-on brief or PR notes.
