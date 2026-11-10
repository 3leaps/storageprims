# DDR-0013: Receipt-Bearing Reads and Admitted Writes

> **Status**: Accepted
> **Date**: 2026-10-01
> **Authors**: entarch, devlead, devrev, secrev

## Decision

The Rust `StorageProvider` trait has two independently discoverable extensions:

```rust
fn read_responses(&self) -> Option<&dyn ReadResponseProvider> { None }
fn admitted_puts(&self) -> Option<&dyn AdmittedPutProvider> { None }
```

Existing required methods and response fields remain unchanged. No extension
capability variant or C ABI constructor/function is introduced. The declarations,
public constructors and accessors are in
[`extensions.rs`](../../crates/storageprims-core/src/extensions.rs) and
[`control.rs`](../../crates/storageprims-core/src/control.rs).

### Selected read response

`ReadResponseProvider` supplies an I/O-free, credential-resolution-free
`read_support()` profile and
`read_response(ReadRequest, OperationControl) -> FutureResult<'_, ReadResponse, ExtensionError>`.
`ReadSupport::new` explicitly supplies current, native-version and validator-match
booleans; there is no optimistic default. `ReadRequest` selects either the current
whole object or an existing `GuardedReadSelection`. A guarded selection remains
bound to its original configured target and key; providers validate it rather
than rebinding it to another handle or silently reading unguarded bytes.

`ReadResponse::new` takes an owned `BoxedByteStream`, existing `SourceReceipt`,
`ReadEnforcement` and `ReadMetadata`. Accessors and `into_parts` expose those
components. Receipt and metadata come from the selected response, not a separate
HEAD. Existing `SourceReceipt`, `SourceObservation` and `GuardedReadResponse`
fields are unchanged. Enforcement reports selection enforcement, not a digest or
snapshot guarantee. An unguarded read may observe in-place mutation.

`ReadMetadata::new` takes content type as `Evidence<Option<String>>` and user
metadata as `Evidence<BTreeMap<String, String>>`. `Available(None)` and an
available empty map mean observed absence; `Unavailable` means the read path did
not supply that evidence. Stream failures after opening remain reader I/O errors.

### Provider-bound admitted write

`AdmittedPutProvider` supplies I/O-free `put_support()` and
`admit_put(key, PutOptions, WriteMode) -> Result<Box<dyn PreparedPut>, ExtensionError>`.
Support explicitly states mode, unconditional/create-only/validator-match support,
content-type/user-metadata support and maximum content length. Only `SinglePart`
is defined. Admission requires known length and validates exact options without
body polling, credential resolution, existence/permission probes or filesystem
access. Support is declarative, not authorization or a commit promise.

Provider support profiles must not invent functionality: a no-sidecar local
adapter reports current reads, no source guards and known absent metadata; its
single-part maximum is `u64::MAX`, subject to representation and filesystem limits,
with no additional library write ceiling. An S3 adapter reports supported guards
and selected-GET metadata, and a single-PUT ceiling of `5 * 1024^3` bytes or a lower
configured endpoint bound. Ordinary S3 put must enforce that ceiling before body
polling as well. These adapter requirements do not claim that a concrete adapter
already exposes the new hooks.

`PreparedPut::execute(self: Box<Self>, body, OperationControl)` returns
`FutureResult<'static, PutResult, PutFailure>`. The single-use object owns the
original provider binding, target and options; execution cannot replace them and
does not borrow a stack provider. Execution performs runtime permission checks,
safe target access and atomic enforcement of the accepted predicate. It must not
acknowledge success while ignoring a body failure. No replayability, retry or
compensating delete is implied.

`PutFailure` retains typed `ExtensionError` and `CommitKnowledge`.
`into_parts(self) -> (CommitKnowledge, ExtensionError)` moves both original values
into composition without cloning or reconstructing a cause from its kind.
Borrowed `commit()` and `error()` accessors remain available.

Knowledge describes this attempt, not target existence: `NotStarted` means no
write dispatch; `NotCommitted` is a known noncommit; `Committed` includes known
commit followed by contracted-completion failure; `Unknown` retains uncertainty.
Indeterminate network/rename results are `Unknown`, not `NotCommitted` or rollback. Canonical `StorageError`
variants and nested source linkage are retained, never inferred from message text.

Providers must sanitize canonical causes **before wrapping**, including every
exposed nested source: absolute roots and credentials must be absent from
`Display`, `Debug` and source-chain formatting. Wrapper formatting does not
sanitize arbitrary `StorageError.detail` or OS-error messages. New response,
request and metadata debug formatting omits sensitive values.

### Cooperative control

`Cancellation` is a clonable monotonic latch. `new`/`Default` starts uncanceled;
`cancel` is idempotent and `cancelled()` does not lose a notification racing
registration. `OperationControl::new` requires one absolute `std::time::Instant`
deadline and that latch; there is no infinite/default control or per-step reset.
Providers refuse already stopped control before I/O and observe it between steps.

Synchronous calls cannot necessarily be interrupted. Control is checked before
and after polls and on result observation. A result first observed after deadline
or with cancellation latched during its poll is not timely success. Known commit
evidence survives as a failure; a future dropped before outcome observation
provides no receipt or rollback proof. An already observed timely completion is
not overturned by a genuinely later cancellation. No detached cleanup mutation
is authorized by stopping an operation.

### Rust source compatibility and projections

`StorageError`, `Capability` and `InspectionLimitDimension` become
`#[non_exhaustive]`. `InspectionLimitDimension::Entries` adds JSON `entries`.
Both changes require downstream wildcard matches: this is a breaking-minor Rust
source change, **not** universal source compatibility. Other old enums and public
struct fields are not retroactively changed. New enums and request/support/
response/control/receipt/failure structs are non-exhaustive; private fields have
public construction/access APIs for external providers.

Existing Serde spellings, accepted old JSON shapes and numeric C error codes remain
unchanged. Inspection still maps to `Other = 99`; `entries` is an additive
structured dimension, not a claim that all error JSON is byte-identical. Default
absent hooks preserve untouched provider implementations. Delimiter discovery
defaults to `supports_delimiter(_) == false`, without disabling direct
`list_delimited` calls by legacy implementors.

External implementors and consumers add wildcard match arms for extensible enums,
use constructors/accessors for new structs, and handle missing optional hooks
explicitly. Implementing discovery requires overriding the real `StorageProvider`
hooks; no blanket extension trait substitutes for them. Signature compilation is
not evidence of runtime guard enforcement, safe error production or commit behavior.
