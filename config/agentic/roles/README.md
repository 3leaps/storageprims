# Role Catalog (storageprims)

Agentic role prompts for AI agent sessions in this repository.

Roles extend [crucible baseline roles](https://crucible.3leaps.dev/catalog/roles/) with
storageprims-specific scope, responsibilities, and validation requirements.

## Available Roles

| Role                                   | Slug           | Category   | Purpose                                                |
| -------------------------------------- | -------------- | ---------- | ------------------------------------------------------ |
| [Development Lead](devlead.yaml)       | `devlead`      | agentic    | Provider implementation, FFI layer, integration        |
| [Enterprise Architect](entarch.yaml)   | `entarch`      | agentic    | SDK evaluation, decision records, consumer integration |
| [Development Reviewer](devrev.yaml)    | `devrev`       | review     | Code review, ADR compliance, provider parity           |
| [Security Review](secrev.yaml)         | `secrev`       | review     | FFI safety, credential handling, auth chains           |
| [Quality Assurance](qa.yaml)           | `qa`           | review     | Cross-provider testing, parity validation              |
| [FFI Architect](ffiarch.yaml)          | `ffiarch`      | agentic    | Bindings, cross-language integration, cbindgen         |
| [Delivery Lead](deliverylead.yaml)     | `deliverylead` | governance | Gate tracking, sprint cadence, consumer readiness      |
| [Information Architect](infoarch.yaml) | `infoarch`     | agentic    | Documentation, schemas, standards                      |
| [Release Engineering](releng.yaml)     | `releng`       | automation | Release workflows, artifact signing, crates.io         |
| [CI/CD Automation](cicd.yaml)          | `cicd`         | automation | Pipelines, runners, platform matrix                    |

## Key Customizations for storageprims

All roles include storageprims-specific extensions beyond the crucible baselines:

### Cross-Language Awareness

Every role that touches code must consider FFI and binding consequences. Rust API
decisions propagate through the C-ABI FFI layer into Go bindings.

### Decision Record Compliance

Provider implementations must map to the canonical core contract (ADR-0001) and
error taxonomy (ADR-0004). The devrev role explicitly verifies this on every PR.

### Credential Safety

All roles that touch provider auth chains reference SDR-0001 (credential boundary)
and DDR-0006 (provider config). The secrev role enforces redaction verification.

## Usage

Reference roles in session prompts or AGENTS.md:

```yaml
roles:
  - slug: devlead
    source: config/agentic/roles/devlead.yaml
```

Or load directly in a session:

```
Role: devlead (config/agentic/roles/devlead.yaml)
```

## Role Selection Guide

| Task                    | Primary Role | May Escalate To                                |
| ----------------------- | ------------ | ---------------------------------------------- |
| Provider implementation | devlead      | secrev (credentials), qa (testing)             |
| Decision records        | entarch      | devlead (implementation validation)            |
| Code review             | devrev       | secrev (FFI/auth), entarch (decision drift)    |
| FFI layer / Go bindings | ffiarch      | secrev (memory safety), devlead (impl)         |
| Security review         | secrev       | human maintainers (critical)                   |
| Test design             | qa           | devlead (implementation questions)             |
| CI/CD changes           | cicd         | releng (release workflows), secrev (secrets)   |
| Release preparation     | releng       | deliverylead (gate status), human (approval)   |
| Documentation           | infoarch     | entarch (decision content), devlead (accuracy) |
| Delivery coordination   | deliverylead | entarch (decisions), devlead (timelines)       |

## Schema

Role files conform to the [role-prompt schema](https://schemas.3leaps.dev/agentic/v0/role-prompt.schema.json).
