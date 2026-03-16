# Decision Records - storageprims

Architecture (ADR), Design (DDR), and Security (SDR) decision records for
storageprims.

## Index

| ID       | Type     | Title                                                                                                                                        | Status   | Date       |
| -------- | -------- | -------------------------------------------------------------------------------------------------------------------------------------------- | -------- | ---------- |
| ADR-0001 | Arch     | [Canonical Core Contract and Provider-Neutral Surface](ADR-0001-canonical-core-contract-and-provider-neutral-surface.md)                     | Proposed | 2026-03-16 |
| ADR-0002 | Arch     | [Crate Structure and Library-First Adapter Boundaries](ADR-0002-crate-structure-and-library-first-adapter-boundaries.md)                     | Proposed | 2026-03-16 |
| ADR-0003 | Arch     | [FFI Design for Metadata and Streaming Data Plane](ADR-0003-ffi-design-for-metadata-and-streaming-data-plane.md)                             | Proposed | 2026-03-16 |
| ADR-0004 | Arch     | [Error Taxonomy and Cross-Provider Mapping](ADR-0004-error-taxonomy-and-cross-provider-mapping.md)                                           | Proposed | 2026-03-16 |
| DDR-0003 | Design   | [List Pagination Semantics and Continuation-Token Contract](DDR-0003-list-pagination-semantics-and-continuation-token-contract.md)           | Proposed | 2026-03-16 |
| DDR-0004 | Design   | [Optional Capability Model](DDR-0004-optional-capability-model.md)                                                                           | Proposed | 2026-03-16 |
| DDR-0005 | Design   | [Copy Semantics Across Same-Provider and Cross-Provider Targets](DDR-0005-copy-semantics-across-same-provider-and-cross-provider-targets.md) | Proposed | 2026-03-16 |
| DDR-0006 | Design   | [Provider Configuration Surface and Credential Representation](DDR-0006-provider-configuration-surface-and-credential-representation.md)     | Proposed | 2026-03-16 |
| DDR-0007 | Design   | [Binary-First Remote Text Inspection and Helper Boundary](DDR-0007-binary-first-remote-text-inspection-and-helper-boundary.md)               | Proposed | 2026-03-16 |
| SDR-0001 | Security | [Credential Boundary and Redaction Policy](SDR-0001-credential-boundary-and-redaction-policy.md)                                             | Proposed | 2026-03-16 |

## Record Types

- **ADR**: Structural choices affecting repository boundaries, public contracts, FFI, bindings, and provider strategy
- **DDR**: API design, normalization rules, pagination semantics, capability modeling, and implementation-shaping choices
- **SDR**: Credential boundaries, redaction, safe defaults, and stream/temporary-resource hardening

## Near-Term Activity Map

This section tracks current and planned decision records, subject to use-case review.

Note: entries in the activity map are planned records. Some IDs are reserved and not yet present
as files.

## Current Consumer Drivers

- `gonimbus`: very large-bucket indexing and prefix-scoped listing; pushes storageprims toward strong pagination, delimiter-listing, and partial-failure semantics
- `fulseed`: deterministic object-store workflows; pushes storageprims toward a boring, dependency-light CRUD contract with stable auth/config behavior
- `datarakt`: Go-first transfer, inspection, and batch access; makes Go bindings and streaming design immediate priorities
- `lanyte`: native Rust plus future IPC-wrapped storage access; reinforces mechanism/policy separation and clean primitive boundaries

Working planning companion:

- `.plans/bootstrap/consumer-parity-matrix.md`

### ADR Activity

1. **ADR-0002: Crate Structure and Library-First Adapter Boundaries**
   - Confirm workspace members and dependency direction
   - Lock provider crate boundaries, FFI crate role, utility CLI role, and binding layout
   - Enforce DRY library-first adapter boundaries to prevent CLI/control-plane drift

2. **ADR-0003: FFI Design for Metadata and Streaming Data Plane**
   - Lock JSON control-plane contract strategy
   - Lock pipe/file-descriptor data-plane strategy
   - Define ownership, lifecycle, error state, and runtime-management rules
   - Incorporate the lesson from gonimbus streaming contracts while preserving a cleaner Go `io.Reader` / `io.Writer` binding surface

3. **ADR-0004: Error Taxonomy and Cross-Provider Mapping**
   - Align canonical errors with rsfulmen patterns where appropriate
   - Define FFI-facing error codes and binding mapping expectations
   - Decide what provider detail is preserved vs normalized away

4. **ADR-0005: Dependency Governance for Provider SDKs**
   - Evaluate provider SDK license posture and transitive weight
   - Define approval thresholds for new dependencies
   - Lock `cargo deny` and audit expectations

5. **ADR-0006: Language Bindings Distribution and Version Synchronization**
   - Decide in-repo packaging model for Go and later TypeScript
   - Decide version/tagging policy for Go submodules
   - Decide local-development vs release artifact expectations

6. **ADR-0007: Artifact Groups and Binding Consumer Strategy**
   - Plan for static artifacts for Go and runtime packaging for TS/Python-class consumers
   - Decide Windows toolchain split if needed
   - Keep this behind ADR-0006 unless release design accelerates early

7. **ADR-0008: Schema Contracts and Versioning for Control-Plane JSON**
   - Decide whether control-plane payloads get formal JSON Schema in v0.1
   - If yes, lock schema IDs, versioning, and validation expectations

### DDR Activity

1. **DDR-0001: Storage Scope - primitives only, not orchestration**
2. **DDR-0002: URI Parsing Model and Target Normalization Rules**
3. **DDR-0003: List Pagination Semantics and Continuation Token Contract**
   - Define literal-prefix semantics and opaque continuation tokens
   - Keep durable checkpoint/resume policy above the library
   - Preserve list performance for large-bucket consumers

4. **DDR-0004: Optional Capability Model**
   - Keep the minimum provider surface small and universal
   - Represent delimiter listing, multipart, and future advanced behaviors as explicit capabilities
   - Provide a binding-friendly capability discovery story

5. **DDR-0005: Copy Semantics Across Same-Provider and Cross-Provider Targets**
   - define `copy` as byte-preserving object movement
   - prefer native same-provider copy when it preserves semantics
   - require cross-provider relay as the common path when native copy is unavailable

6. **DDR-0006: Provider Configuration Surface and Binding Representation**
   - Decide how profile/config names, credential-file paths, endpoint URLs, and env-driven resolution are expressed
   - Keep secret-bearing material out of the default happy path where possible
   - Make separate-process control-plane use cases explicit rather than relying on hidden ambient state

7. **DDR-0007: Binary-First Remote Text Inspection and Helper Boundary**
   - keep storageprims byte-oriented
   - allow a shared helper layer for encoding detection and LF/CRLF/CR framing
   - leave record semantics and format-specific parsing above the storage primitive layer

### SDR Activity

1. **SDR-0001: Credential Boundary and Redaction Policy**
   - classify safe vs risky credential input channels
   - define what may be passed directly, what should be file/env based, and what must be redacted in logs/errors

2. **SDR-0002: Safe Defaults for Auth Chains, Retries, and Local Provider Behavior**
3. **SDR-0003: Stream, Pipe, and Temporary Resource Lifecycle Hardening**
4. **SDR-0004: Customer Metadata and User-Supplied Headers Boundary**

## Sequencing Guidance

Before S3 and Go bindings move much further, storageprims should have at least:

1. ADR-0001
2. ADR-0003
3. ADR-0004
4. DDR-0003
5. DDR-0004
6. ADR-0006

Those six decisions stop the most expensive forms of cross-language and
cross-repository drift.
