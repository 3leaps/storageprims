# DDR-0006: Provider Configuration Surface and Credential Representation

> **Status**: Proposed
> **Date**: 2026-03-16
> **Authors**: entarch, deliverylead, ffiarch, Architecture Council

## Context

storageprims has to work in at least two materially different execution modes:

- **CLI / in-process use**: the caller may rely on ambient environment, shared SDK config, local profiles, and machine-local auth state
- **Control-plane / separate-process use**: the process performing storage operations may not share the caller's environment, profile state, or filesystem layout

The known consumer landscape makes this unavoidable:

- `datarakt` will need both direct CLI workflows and longer-lived control-plane/server workflows
- `gonimbus` and `fulseed` already show the real-world split between enterprise profile/config-based auth and simpler S3-compatible key/secret auth
- cloud-native systems may want default chains, profile selectors, service-account files, managed identities, or explicitly supplied secret material depending on deployment shape

storageprims therefore needs a contract for **how provider configuration and credentials are represented**, without turning the library into a secrets manager or a deployment platform.

## Design

### Overview

storageprims adopts a provider configuration model with three layers:

1. **Target configuration**: non-secret provider/location settings
2. **Credential source descriptor**: how authentication should be resolved
3. **Resolved secret material**: optional inline secret-bearing values for process-isolated use cases

This allows the same canonical contract to serve:

- ambient/default-chain CLI use
- profile/config-based enterprise use
- credential-file-based container or service deployments
- explicit inline credential passing when caller and storage process are distinct

### 1. Target configuration is distinct from credentials

Provider configuration MUST distinguish non-secret connection/target fields from credential fields.

Examples of non-secret target settings include:

- provider
- account / bucket / container / prefix target fields
- region
- endpoint URL
- force-path-style or equivalent provider options
- retry/resiliency knobs where they belong in provider config

This separation matters because these fields are often safe to log, cache, diff, and transport,
while credential material is not.

### 2. Credential source is explicit

The canonical config model SHOULD express authentication through an explicit credential-source
descriptor rather than assuming hidden ambient state.

Conceptually:

```json
{
  "auth": {
    "mode": "default_chain | profile | credentials_file | inline_static | env | inline_env_map"
  }
}
```

The exact schema may evolve, but the contract should preserve the difference between:

- use the provider SDK's native default resolution
- use a named profile/configuration
- use a credential file by path/reference
- use explicitly supplied secret-bearing values
- use explicitly supplied environment-variable material

### 3. Reference-based auth is preferred where possible

For both security and portability, storageprims SHOULD prefer auth modes that point to existing
provider-native mechanisms rather than embedding secrets directly.

Preferred examples:

- AWS profile name
- GCP ADC / gcloud configuration name
- Azure default credential chain
- credential file path/reference
- explicit env-file or process-env injection done outside storageprims

This aligns with how enterprise users already work and preserves native SDK behavior.

### 4. Inline secret-bearing auth is allowed for process-isolated use cases

Inline secret-bearing credential material MUST be allowed in the canonical configuration model,
because control-plane and FFI deployments may need to hand credentials to a separate process that
does not share the caller's environment or local profile state.

Examples include:

- S3-compatible access key / secret key / session token
- cloud credential JSON or equivalent structured material when the caller chooses inline delivery
- explicit environment-variable maps supplied over a control-plane boundary

This is an interoperability requirement, not the preferred happy path.

### 5. CLI and control-plane channels are not equivalent

The same credential representation may be acceptable in one transport and inappropriate in another.

Therefore:

- the **library and FFI** may accept in-memory secret-bearing values
- the **control plane** may accept inline secret-bearing values or references, subject to security policy
- the **CLI** should prefer ambient env, profiles, config names, and file references rather than secret-bearing arguments

This record defines the representation model. Channel safety and redaction rules are governed by the credential-boundary SDR.

### 6. Environment support must work in both ambient and explicit modes

storageprims SHOULD support two distinct environment patterns:

1. **Ambient environment resolution**
   - typical CLI or same-process usage
   - examples: `AWS_PROFILE`, `AWS_ACCESS_KEY_ID`, `GOOGLE_APPLICATION_CREDENTIALS`

2. **Explicit environment map delivery**
   - for process-isolated control-plane or FFI use
   - caller passes a bounded env map intentionally as part of the request/config

This distinction matters because control-plane deployments cannot assume ambient environment parity with the invoking user.

### 7. Generic secret-manager integration stays above storageprims

storageprims SHOULD NOT directly integrate with every external secret system such as:

- Seclusor
- 1Password
- Bitwarden / Vaultwarden
- cloud secret-manager APIs

Instead, callers may use those systems to produce one of the credential representations that
storageprims understands:

- ambient env vars
- explicit env maps
- file references
- inline resolved secret material

This keeps storageprims focused on storage access rather than secret retrieval orchestration.

### 8. Provider-specific selectors must remain honest

The config model must allow provider-native selectors rather than forcing all auth into one fake universal shape.

Examples:

- AWS profile names and endpoints
- S3-compatible endpoints, path-style flags, and static credential selectors
- GCP configuration names or ADC file paths
- Azure tenant/client selectors where needed

Uniformity should exist at the representation level, not by erasing provider-specific concepts that users already depend on.

AWS-native and S3-compatible deployments should therefore be treated as related but not identical
configuration modes. `storageprims-s3` should preserve AWS-first defaults while still allowing
explicit endpoint/auth overrides for compatible systems when the caller opts into them.

## Trade-offs

### Pros

- Works for both datarakt CLI and datarakt control-plane deployment shapes.
- Preserves enterprise profile/config workflows while still supporting S3-compatible static credentials.
- Gives bindings a clean contract for process-isolated operation.
- Avoids coupling storageprims to any one secret-management product.

### Cons

- Adds config-model complexity compared with ambient-env-only assumptions.
- Requires careful documentation so users understand preferred vs allowed auth modes.
- Some provider-specific selectors reduce the neatness of a purely universal schema.

## Alternatives Considered

### Alternative 1: Ambient environment only

Rejected.

This fails for separate-process control-plane and FFI scenarios.

### Alternative 2: Inline secrets only

Rejected.

This would be worse for enterprise users and would fight native SDK credential chains.

### Alternative 3: Build secret-store integrations directly into storageprims

Rejected.

That would blur the line between storage access and secrets management, and would expand the dependency and threat surface unnecessarily.

## Implementation Notes

- Provider implementations must reject represented credential modes they do not support during
  construction, before filesystem or provider I/O. The S3 provider currently rejects
  `credentials_file` as `InvalidArgument` on `credentials.mode`.
- The S3 provider implements the default chain and profile selector, plus explicit in-memory
  credential and environment-based modes for process-isolated callers. The in-memory modes are
  caller-risk channels rather than the recommended happy path.
- Explicit env-map delivery should be designed with redaction and bounded scope in mind.
- Documentation should distinguish "recommended" auth modes from merely "supported" ones.
- Diagnostic and probe results expose only the credential-source class, never profile names,
  credential-file paths, environment-variable names, or inline map contents.
- CLI and language-binding validation remains outstanding, so this record remains `Proposed`.

## Decision Points

This record should remain `Proposed` until:

- the `CredentialSource` shape is exercised in CLI, FFI, and Go binding paths
- AWS-native and S3-compatible configurations both fit cleanly without collapsing into one misleading auth model
- inline-secret and reference-based flows are both validated across same-process and separate-process usage

## References

- `docs/decisions/ADR-0001-canonical-core-contract-and-provider-neutral-surface.md`
- `docs/decisions/ADR-0003-ffi-design-for-metadata-and-streaming-data-plane.md`
- `https://github.com/3leaps/gonimbus/blob/main/docs/auth/aws-profiles.md`
- `https://github.com/fulmenhq/fulseed/blob/main/docs/guides/s3/authentication.md`
- `https://github.com/fulmenhq/fulseed/blob/main/docs/guides/gcs/authentication.md`
- `https://github.com/3leaps/seclusor/blob/main/README.md`
