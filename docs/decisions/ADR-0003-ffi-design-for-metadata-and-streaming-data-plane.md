# ADR-0003: FFI Design for Metadata and Streaming Data Plane

> **Status**: Approved
> **Date**: 2026-03-16
> **Authors**: entarch, ffiarch, deliverylead, Architecture Council

## Context

storageprims must serve multiple consumption modes:

- native Rust consumers
- Go bindings as an early, high-priority integration surface for datarakt
- later cross-language consumers that may need the same core semantics

The key consumer pressure is asymmetric:

- `datarakt` needs high-performance object reads, range reads, writes, and transfers in Go
- `gonimbus` has already proven that metadata and byte streams need a strict contract, but its mixed-framing stream format is an application-level choice for CLI and helper workflows
- `storageprims` is a library primitive, not a CLI wire protocol

This creates an FFI requirement that is different from gonimbus:

1. **Metadata operations** need a simple, stable, language-neutral contract.
2. **Bulk bytes** must cross the boundary without repeated JSON serialization or per-chunk FFI calls.
3. **Go consumers** should receive ordinary streaming primitives (`io.Reader` / `io.Writer` backed by OS resources), not an application-specific framing protocol.
4. **Error handling and ownership** must stay simple enough for bindings to implement safely and consistently.

## Decision

storageprims adopts a split FFI design:

- **Control plane**: metadata and structured results cross the C-ABI boundary as JSON strings.
- **Data plane**: bulk object bytes cross the boundary through OS-native stream primitives, not through JSON or chunk-by-chunk FFI callbacks.

### 1. JSON control plane for structured operations

Structured operations MUST use JSON payloads over the FFI boundary.

This includes at least:

- list
- head
- delete
- copy result payloads
- stream open/finish metadata
- runtime and diagnostic responses where structured output is needed

The FFI contract uses:

- flat error codes for immediate success/failure signaling
- owned UTF-8 JSON strings for structured results
- a dedicated free function for returned strings

storageprims MUST NOT expose complex nested C structs as the primary public FFI model.

### 2. OS-native data plane for object bytes

Bulk content transfer MUST use OS-native streaming primitives.

Conceptually:

- `get` / `get_range` return a readable stream endpoint to the caller
- `put` returns a writable stream endpoint to the caller
- Rust performs provider I/O on the far side of that stream endpoint
- the binding side uses native language I/O over the endpoint

On Unix-like platforms, this is expected to be implemented with pipes/file descriptors.
Platform-specific equivalents may be used elsewhere, but the semantic contract remains:

- caller receives a native streaming endpoint
- caller reads or writes bytes directly
- no per-chunk FFI crossings are required for payload movement

### 3. No mixed-framing protocol at the library FFI boundary

storageprims will NOT adopt gonimbus-style mixed framing (JSON headers interleaved with raw
byte chunks) as its primary FFI contract.

That framing is useful for CLI pipelines and helper wire contracts, but storageprims is a
primitive library. Its binding consumers need a cleaner separation:

- metadata is obtained as structured JSON
- bytes are obtained as a plain stream

If an application wants mixed framing, progress events, or higher-level transfer events,
that belongs above storageprims.

### 4. Stream lifecycle model

Data-plane operations MUST have explicit lifecycle semantics.

The canonical lifecycle is:

1. Open the stream operation
2. Receive a native stream endpoint, and where needed, opening metadata
3. Consume or produce bytes using native I/O
4. Finalize the operation explicitly to receive terminal status and result metadata

Implications:

- `put` requires an explicit finish step so provider-side completion errors are observable
- `get` and `get_range` may also have explicit completion/finalization where needed to surface late I/O failures predictably
- bindings MUST close endpoints and invoke finalization/cleanup correctly

Opening metadata MAY include fields needed immediately by consumers, such as size, etag,
content type, or resolved range metadata. This is allowed specifically to avoid forcing an
extra head request when the open operation already has authoritative metadata.

### 5. Error handling model

The FFI surface MUST use:

- flat error codes as the function return value
- thread-local last-error detail for rich messages
- owned error/result strings freed by `storageprims_free_string()`

Error codes represent coarse classes suitable for bindings. Richer provider/context detail
is carried in the canonical Rust error taxonomy and reflected into JSON/error detail.

The single source of truth for FFI-facing error codes SHOULD be a code-defined enum or constant
set in shared storageprims Rust code, exported consistently through the FFI layer and bindings.
Schema-driven code definitions are intentionally deferred; v0 should remain code-first so error
codes evolve with the canonical error taxonomy in one place.

### 6. Runtime and ownership rules

The FFI crate owns the async runtime used to drive provider I/O for FFI consumers.

The FFI contract MUST define:

- initialization and shutdown behavior
- ABI version reporting
- ownership of returned strings
- responsibility for closing stream endpoints
- responsibility for finalizing opened stream operations

Bindings MUST NOT assume that dropping a native reader/writer alone is sufficient to learn
whether the provider-side operation succeeded.

### 7. Scope of this ADR

This ADR defines the architectural pattern, not every function signature.

Follow-on design work still needs to specify:

- exact open/finalize function shapes
- how opening metadata is encoded per operation
- platform-specific stream endpoint mechanics
- binding-layer API design in Go and TypeScript

## Consequences

### Positive

- Fits datarakt's Go-first streaming needs without turning storageprims into a CLI wire protocol.
- Keeps metadata and payload bytes cleanly separated.
- Avoids expensive and awkward per-chunk FFI calls for large transfers.
- Preserves a simple ownership model already proven in the prims family.

### Negative

- Requires explicit lifecycle management for streaming operations.
- Adds implementation complexity around pipe/endpoint finalization and late-error propagation.
- Forces bindings to correctly wrap native endpoints and cleanup rules.

### Neutral

- Application-level progress events, transfer jobs, and mixed-framing streams remain valid above storageprims.
- Native Rust consumers can bypass this FFI layer entirely while preserving equivalent semantics.
- Additional SDR work is still needed for stream/resource hardening.

## Decision Points

This record is `Approved` on current evidence.

Re-open or supersede it if:

- the implemented FFI layer needs a materially different control-plane or data-plane split
- error-code constants cannot be maintained from one shared storageprims source of truth
- Go bindings or other consumers show that the lifecycle model is impractical in real use

## Alternatives Considered

### Alternative 1: JSON for everything, including content bytes

Rejected.

This would be far too expensive for large object transfers and would make Go bindings
awkward for the primary consumer.

### Alternative 2: Mixed framing at the FFI boundary

Rejected.

This is useful for CLI and wire-level helper contracts, as gonimbus has demonstrated, but it
is the wrong abstraction level for a storage primitive consumed in-process through bindings.

### Alternative 3: Direct C structs across the boundary

Rejected.

This would create layout, versioning, and ownership complexity precisely where the project
needs the most stability.

### Alternative 4: Callback-driven streaming across FFI

Rejected.

Callbacks complicate threading, ownership, and Go integration. Plain stream endpoints are a
better fit for the downstream consumers we know about.

## References

- `docs/decisions/ADR-0001-canonical-core-contract-and-provider-neutral-surface.md`
- `.plans/bootstrap/architecture.md`
- `.plans/bootstrap/integration.md`
- `https://github.com/3leaps/sysprims/blob/main/docs/decisions/ADR-0004-ffi-design.md`
- `https://github.com/3leaps/gonimbus/blob/main/docs/development/streaming/stream-contract.md`
- `https://github.com/3leaps/gonimbus/blob/main/docs/architecture/adr/ADR-0004-language-neutral-content-stream-contract.md`
- `https://github.com/fulmenhq/datarakt/blob/main/README.md`
