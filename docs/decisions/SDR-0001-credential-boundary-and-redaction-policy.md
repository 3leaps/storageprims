# SDR-0001: Credential Boundary and Redaction Policy

> **Status**: Proposed
> **Date**: 2026-03-16
> **Authors**: entarch, secrev, deliverylead, Architecture Council

## Threat Model

### Assets

The primary assets are:

- access keys, secret keys, session tokens, service-account material, and equivalent secret-bearing credentials
- profile-linked auth state where disclosure could reveal sensitive account structure
- credential-bearing environment maps and env-like files
- control-plane requests carrying credential references or inline secret material

### Threat Actors

- local users with access to shell history or process inspection
- operators or platform systems that collect logs, crash dumps, traces, or request payloads
- remote callers to a storage control plane
- accidental disclosure by developers through config files, PRs, or diagnostics

### Attack Vectors

- secrets passed as CLI args and exposed via shell history or process listings
- secrets logged in structured logs, error messages, panic traces, or telemetry
- control-plane requests persisted in logs or audit stores without redaction
- env-like files stored insecurely or referenced carelessly
- broad environment forwarding across process boundaries

## Security Decision

storageprims distinguishes **credential representation** from **credential transport safety**.

The system will support multiple credential forms, but not all delivery channels are equally safe.

### 1. CLI must avoid secret-bearing arguments

CLI surfaces built on storageprims MUST NOT encourage or require raw secret-bearing credentials in direct arguments when a safer channel exists.

Forbidden or strongly disallowed examples:

- `--secret-key ...`
- `--session-token ...`
- inline JSON service-account payloads in CLI flags

Preferred CLI channels:

- ambient provider env vars
- profile/config selectors
- credential-file paths
- env-file paths where explicitly supported

### 2. Library, FFI, and control-plane may carry in-memory secrets

In-memory secret-bearing values are allowed for:

- direct library calls
- FFI/binding calls
- control-plane requests

because these channels may be necessary when the storage process is isolated from the caller.

However, these channels are **caller-risk channels** and must be treated as sensitive by default.

### 3. References are preferred over inline secrets when practical

Where the process model allows it, callers SHOULD prefer credential references over inline secrets.

Preferred references include:

- profile/config names
- credential-file paths
- explicit env-file paths
- provider-native default chains

Inline secret-bearing delivery remains supported for cases where those references are not enough.

### 4. Redaction is mandatory for secret-bearing material

storageprims and adapters built on it MUST redact secret-bearing values from:

- logs
- error messages
- debug output
- panic/reporting surfaces where possible
- structured control-plane request/response diagnostics

At most, systems may expose minimal masked hints such as:

- final 4 chars of an access key ID
- auth source class such as `profile`, `adc`, `default_chain`, or `inline_static`

Secret values themselves MUST NOT appear in normal diagnostics.

### 5. Env maps must be explicit and bounded

If storageprims supports explicit environment-map delivery across a control-plane or FFI boundary,
that map MUST be intentionally scoped.

It should not mean "forward the whole caller environment." Instead:

- only explicitly provided keys are forwarded
- docs should call out the risk clearly
- implementations should avoid logging the full map

### 6. Env-like files are allowed but remain operator-managed risk

Users may choose env-like files or secret files as part of their deployment model.

storageprims may support file references, but it does not guarantee:

- file permission correctness
- secure lifecycle management
- safe secret storage policy

Those remain operator responsibilities.

### 7. External secret tools stay outside the library boundary

Tools such as Seclusor, 1Password, Bitwarden/Vaultwarden, and cloud secret managers are valid upstream secret sources.

But storageprims itself will not:

- fetch from them directly as a built-in requirement
- bake their APIs into the core credential contract

Applications may use those tools to prepare env vars, files, or inline payloads before calling storageprims.

## Implementation

### Controls

- redact secret-bearing auth fields in config serialization and logs
- keep secret-bearing CLI flags absent
- distinguish safe references from secret-bearing inline material in the config model
- expose auth-source metadata without exposing secrets
- document caller-risk channels explicitly for control-plane and FFI use

### Validation

- tests should assert that redacted config/debug rendering does not leak secrets
- tests should assert forbidden CLI secret flags are absent or rejected
- control-plane request logging should be reviewed for auth payload redaction
- docs should mark supported channels as recommended vs risky

## Risk Assessment

### Residual Risk

Residual risk remains because:

- callers may still choose to send inline secret material
- operator environments and file permissions may be misconfigured
- upstream frameworks may log request metadata unless configured carefully

### Risk Acceptance

This residual risk is acceptable because separate-process operation requires some way to deliver credentials, and forbidding inline in-memory transport entirely would make legitimate control-plane and FFI use cases unworkable.

The mitigation is to make those channels explicit, bounded, and redacted rather than pretending they do not exist.

## Alternatives Considered

### Alternative 1: Ban inline secret transport entirely

Rejected.

This would make process-isolated control-plane and binding scenarios much harder or impossible.

### Alternative 2: Treat all supported channels as equally safe

Rejected.

That would hide real operator risk and lead to poor CLI and logging choices.

### Alternative 3: Integrate a secrets manager directly into storageprims

Rejected.

That would expand the trust boundary and blur library responsibilities.

## References

- `docs/decisions/DDR-0006-provider-configuration-surface-and-credential-representation.md`
- `/Users/davethompson/dev/3leaps/seclusor/docs/decisions/SDR-0002-secret-input-channels-and-cli-arg-policy.md`
- `/Users/davethompson/dev/3leaps/seclusor/docs/appnotes/02-runtime-deployment-patterns.md`
- `/Users/davethompson/dev/3leaps/gonimbus/docs/auth/aws-profiles.md`
- `/Users/davethompson/dev/fulmenhq/fulseed/docs/guides/s3/authentication.md`
- `/Users/davethompson/dev/fulmenhq/fulseed/docs/guides/gcs/authentication.md`
