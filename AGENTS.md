# storageprims - AI Agent Guide

## Warm-Up Sequence

Read these in order before starting any task:

1. **This file** — storageprims operational protocols
2. **`AGENTS.local.md`** — machine-local overrides and coordination hub paths (gitignored)
3. **Your role definition** — `config/agentic/roles/<your-role>.yaml`
4. **Decision records** — `docs/decisions/README.md` for current status of all ADRs/DDRs/SDRs
5. **Your role state** — `.plans/roles/<your-role>/STATE.md` (if present, gitignored)
6. **Coordination hub** — see `AGENTS.local.md` for chat channels, review requests, and org-level state

### Session End

1. Update `.plans/roles/<your-role>/STATE.md` with current state.
2. Update org-level role state and chat channels per `AGENTS.local.md` coordination hub instructions.

## Operating Model

| Aspect         | Setting                                  |
| -------------- | ---------------------------------------- |
| Mode           | Supervised (human reviews before commit) |
| Classification | code-substantive                         |
| Role Required  | Yes                                      |
| Default Role   | devlead                                  |
| Identity       | Per session (no persistent memory)       |

See [agent-identity standard](https://crucible.3leaps.dev/repository/agent-identity) for modes and attribution.

## Roles

Role definitions live in `config/agentic/roles/` and extend
[crucible baseline roles](https://crucible.3leaps.dev/catalog/roles/) with
storageprims-specific scope.

See [`config/agentic/roles/README.md`](config/agentic/roles/README.md) for the
full catalog, selection guide, and escalation paths.

| Role           | Focus                                           | Source                                   |
| -------------- | ----------------------------------------------- | ---------------------------------------- |
| `devlead`      | Provider implementation, FFI, integration       | `config/agentic/roles/devlead.yaml`      |
| `entarch`      | SDK evaluation, decisions, consumer integration | `config/agentic/roles/entarch.yaml`      |
| `devrev`       | Code review, ADR compliance, parity             | `config/agentic/roles/devrev.yaml`       |
| `secrev`       | FFI safety, credential handling, auth chains    | `config/agentic/roles/secrev.yaml`       |
| `qa`           | Cross-provider testing, parity validation       | `config/agentic/roles/qa.yaml`           |
| `ffiarch`      | Bindings, cross-language integration            | `config/agentic/roles/ffiarch.yaml`      |
| `deliverylead` | Gate tracking, delivery coordination            | `config/agentic/roles/deliverylead.yaml` |
| `infoarch`     | Docs, schemas, standards                        | `config/agentic/roles/infoarch.yaml`     |
| `releng`       | Release workflows, artifact signing, CI/CD      | `config/agentic/roles/releng.yaml`       |

## PR Workflow

storageprims uses a PR-based workflow — no direct pushes to `main`.

### Branch Naming

```
<type>/<slug>-<role>-YYYYMMDD
```

Types: `feat`, `fix`, `docs`, `chore`, `test`, `refactor`, `security`

Examples:

- `feat/s3-provider-devlead-20260317`
- `docs/adr-0001-entarch-20260316`
- `fix/uri-parsing-azure-devrev-20260320`

### Review Flow

```
author (devlead) → devrev (code review)
                       ├→ secrev (if FFI, auth, unsafe, credentials)
                       └→ human merge (@3leapsdave rebase-merges)
```

- All PRs require at least one agent review (devrev).
- FFI boundary changes, `unsafe`, credential handling, and provider auth trigger secrev.
- Human always performs the merge.
- Merge strategy: **rebase-merge** only.

## Multi-Machine Development

This repo is developed on two machines:

- **macOS arm64** — primary development
- **Linux arm64** — Azure storage testing via VPN (Azure Blob only accessible from this machine)

Both machines share a coordination hub (see `AGENTS.local.md` for location and setup).
Machine-specific overrides (SSH aliases, etc.) go in `AGENTS.local.md`.
CI covers both platforms via GitHub Actions.

## Project Overview

**storageprims** is a cross-language cloud storage primitives library implemented in Rust with first-class bindings for Go and TypeScript.

**Core differentiator**: One Rust implementation, consumed everywhere — uniform interface across S3, GCS, Azure Blob, and local filesystem.

**Key principle**: Reliable cloud storage access without reimplementation per tool or language.

## Quick Reference

| Task           | Command            |
| -------------- | ------------------ |
| Build          | `cargo build`      |
| Test           | `cargo test`       |
| Format         | `cargo fmt`        |
| Lint           | `cargo clippy`     |
| License check  | `cargo deny check` |
| Security audit | `cargo audit`      |
| Full check     | `make check`       |
| PR final       | `make pr-final`    |

## Session Protocol

### Before Changes

- Read relevant code and ADRs first
- Understand cross-provider implications of changes
- Keep changes minimal and focused
- Consider FFI boundary impacts

### Before Committing

- Run `cargo fmt && cargo clippy` (or `make check`)
- Run `cargo test`
- Run `cargo deny check licenses`
- Verify no unintended changes with `git diff`
- Use proper commit attribution (see below)

### Before Pushing Or Updating A PR

- Run `make pr-final`
- Treat `make pr-final` as the required closeout gate for PR creation and PR updates
- If `make pr-final` changes generated artifacts or formatting, include those changes before pushing

## Commit Attribution

Follow [3leaps commit style](https://crucible.3leaps.dev/repository/commit-style):

```
<type>(<scope>): <subject>

<body>

Generated by <Model> via <Interface> under supervision of @<maintainer>

Co-Authored-By: <Model> <noreply@3leaps.net>
Role: <role>
Committer-of-Record: @<maintainer>
```

> **Attribution email policy**: All AI model Co-Authored-By trailers MUST use `noreply@3leaps.net` as the
> email address, regardless of model vendor. This prevents third-party email squatting on GitHub's
> contributor attribution. The model name in the name field provides the transparency; the email is
> plumbing under our control. Do NOT use vendor noreply addresses (`noreply@anthropic.com`,
> `noreply@openai.com`, etc.).

**Example:**

```
feat(s3): implement StorageProvider for S3 using aws-sdk-rust

Adds S3 provider with list, head, get, get_range, put, delete, and copy
operations using the AWS SDK default credential chain.

Changes:
- Implement StorageProvider trait for S3Provider
- Add URI parsing for s3:// scheme
- Integration tests against localstack

Generated by Claude Opus via Claude Code under supervision of @3leapsdave

Co-Authored-By: Claude Opus <noreply@3leaps.net>
Role: devlead
Committer-of-Record: @3leapsdave
```

---

## DO / DO NOT

### DO

- Run `cargo fmt && cargo clippy` before commits
- Read files before editing them
- Keep changes focused on the task
- Run tests before PRs
- Document provider-specific behavior
- Consider FFI memory safety implications
- Reference ADRs when making architectural decisions
- Test error mapping consistency across providers

### DO NOT

- Push without maintainer approval
- Skip quality gates or license checks
- Commit secrets or credentials
- Add GPL/LGPL/AGPL dependencies
- Touch code outside task scope without justification
- Change FFI contracts without ADR review
- Use `unsafe` without clear justification and review
- **EVER commit anything from `.plans/`** — this directory is gitignored and MUST stay local; planning docs are ephemeral working files, not repository artifacts
- Commit `AGENTS.local.md` (gitignored — session-specific guidance)
- Hardcode credentials or access keys in tests (use environment-based auth or mocks)

## Critical Rules

### Never Push Without Approval

All work lands via PR. Push to feature branches freely; push to `main` is blocked by branch protection.

```bash
git checkout -b feat/my-change-devlead-20260317   # Create feature branch
git add <files>                                    # OK
git commit -m "..."                                # OK
git push -u origin HEAD                            # OK (pushes feature branch)
# Then: gh pr create ...                           # Open PR for review
# main push is blocked — human rebase-merges via GitHub
```

### License Compliance

All dependencies must pass `cargo deny check`:

```bash
cargo deny check licenses    # Must pass
cargo deny check advisories  # Must pass
```

### FFI Safety

FFI boundary changes require extra scrutiny:

- Memory ownership must be explicit
- All returned strings freed via `storageprims_free_string()`
- Control plane: complex types serialized as JSON strings across FFI
- Data plane: OS pipes/file descriptors for streaming (no FFI overhead for bulk data)
- See architecture docs for the control plane / data plane split

### Provider Parity

Changes must maintain uniform behavior across providers:

- Same error types regardless of provider
- Same trait surface for all providers
- Provider-specific behavior documented explicitly
- Integration tests cover all providers for each operation

## Key Files

| Path                         | Purpose                                              |
| ---------------------------- | ---------------------------------------------------- |
| `crates/storageprims-core/`  | Core traits, error types, URI parsing, provider enum |
| `crates/storageprims-s3/`    | AWS S3 implementation                                |
| `crates/storageprims-gcs/`   | Google Cloud Storage implementation                  |
| `crates/storageprims-azb/`   | Azure Blob Storage implementation                    |
| `crates/storageprims-local/` | Local filesystem provider                            |
| `crates/storageprims-cli/`   | Diagnostic CLI                                       |
| `ffi/storageprims-ffi/`      | C-ABI exports, runtime management                    |
| `bindings/`                  | Go, TypeScript wrappers                              |
| `docs/decisions/`            | Decision Records (ADR, DDR, SDR)                     |
| `config/agentic/roles/`      | Role catalog (YAML prompt definitions)               |
| `deny.toml`                  | License and security policy                          |

## Standards Reference

- **Online**: https://crucible.3leaps.dev/
- **FulmenHQ patterns**: https://github.com/fulmenhq/crucible
- **Local decisions**: `docs/decisions/` (ADR, DDR, SDR)

## Contact

- **Lead maintainer**: See MAINTAINERS.md
- **Repository**: https://github.com/3leaps/storageprims

---

**Last Updated**: March 16, 2026
