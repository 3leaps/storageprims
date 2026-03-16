## Summary

<!-- 1-3 sentences: what and why -->

## Changes

<!-- Bulleted list of changes -->

## Quality Gates

- [ ] `cargo fmt --check` passes
- [ ] `cargo clippy -- -Dwarnings` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo deny check` passes (if dependencies changed)
- [ ] ADR/DDR/SDR referenced (if architectural decision involved)

## Provider Impact

<!-- Which providers affected? Cross-provider parity maintained? -->

- [ ] S3
- [ ] GCS
- [ ] Azure Blob
- [ ] Local
- [ ] N/A

## FFI Impact

- [ ] No FFI changes
- [ ] FFI control-plane change (JSON contract)
- [ ] FFI data-plane change (streaming)
- [ ] Binding update required (Go / TypeScript)

## Review Routing

- [ ] Standard review (devrev)
- [ ] Security review needed (secrev) — FFI, auth, unsafe, credentials
- [ ] Architecture review needed (entarch) — cross-repo, new ADR

## Attribution

Role: <!-- devlead | entarch | devrev | secrev | qa | cicd | deliverylead -->
