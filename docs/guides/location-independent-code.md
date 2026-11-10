# Writing location-independent code

The same storage code can run against a local directory or an S3 bucket. The
approach: choose the provider once, at the edge of the program, and write
everything else against the `StorageProvider` trait, using only the behaviour
both providers share or checking a capability first.

The code in this guide comes from
[`crates/storageprims-ops/examples/portable_store.rs`](../../crates/storageprims-ops/examples/portable_store.rs),
which CI compiles. Run it with no argument to use temporary local directories
(any platform), or pass `file:///existing/dir` or `s3://bucket/prefix`.

## 1. Choose the provider at the edge

```rust
async fn open_store(uri: &str) -> Result<Box<dyn StorageProvider>, StorageError> {
    let parsed = StorageUri::parse(uri)?;
    match parsed.provider {
        ProviderKind::Local => Ok(Box::new(LocalProvider::new(&parsed.path)?)),
        ProviderKind::S3 => {
            let config = ProviderConfig {
                provider: ProviderKind::S3,
                target: TargetConfig::default(),
                credentials: CredentialSource::DefaultChain,
            };
            Ok(Box::new(S3Provider::from_uri(&parsed, config).await?))
        }
        other => Err(StorageError::InvalidUri {
            uri: uri.to_string(),
            reason: format!("{other:?} has no provider in this example"),
        }),
    }
}
```

`StorageUri::parse` recognises `s3://bucket/prefix` and `file:///absolute/path`.
`S3Provider::from_uri` uses the bucket and prefix from the URI; `LocalProvider`
takes a filesystem path, so the example passes the parsed path. `file://` URIs
take Unix-style absolute paths only; on Windows, build the local provider with
`LocalProvider::new(path)` from a native path instead of a URI. For
S3-compatible endpoints (MinIO, LocalStack, others), set `TargetConfig`'s
`endpoint`, `region` and `force_path_style`.

Everything below receives `&dyn StorageProvider` and never learns which one it
has.

## 2. Use the portable subset

| Do                                                                                                                                | Because                                                                                |
| --------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------- |
| Use keys that satisfy the [local key rules](local-provider.md#keys), and keep the key plus any S3 `root_prefix` within 1024 bytes | The local rules are the stricter ones; S3 counts its prefix toward the 1024-byte limit |
| Always set `PutOptions::content_length`                                                                                           | S3 requires it; local accepts it                                                       |
| Treat `etag` as optional                                                                                                          | Local never returns one                                                                |
| Leave `content_type` and `metadata` empty unless the code checks the destination first                                            | Local refuses them                                                                     |
| Page through `list` until `is_truncated` is false                                                                                 | Both providers page; local pages hold at most 1000 keys                                |
| Keep continuation tokens in memory only                                                                                           | Local tokens are process-local                                                         |
| Match errors by variant, with a wildcard arm                                                                                      | `StorageError` is non-exhaustive and messages are not stable                           |

A create-only write that finds an existing key fails the same way on both
providers. On S3, a write that races another in-flight conditional write can
instead fail with `ConflictKind::Other`; this example returns that as an error
for the caller to re-check:

```rust
async fn publish_once(
    store: &dyn StorageProvider,
    key: &str,
    bytes: Vec<u8>,
) -> Result<bool, StorageError> {
    let options = PutOptions {
        content_length: Some(bytes.len() as u64),
        precondition: PutPrecondition::MustNotExist,
        ..PutOptions::default()
    };
    match store.put(key, Box::new(Cursor::new(bytes)), options).await {
        Ok(_) => Ok(true),
        Err(StorageError::Conflict { kind: ConflictKind::AlreadyExists, .. }) => Ok(false),
        Err(error) => Err(error),
    }
}
```

## 3. Check capabilities, not provider types

When a feature is optional, ask the provider:

- `store.has_capability(Capability::ConditionalPut)` before relying on
  `MustNotExist` (it is unavailable on some Unix platforms for the local
  provider);
- `require_delimiter_listing(store)?` and then `supports_delimiter(d)` before a
  delimiter listing;
- `store.admitted_puts().map(|p| p.put_support())` to see whether the
  destination keeps `content_type`, user metadata, or `Match` preconditions.

Avoid `match store.provider_kind()` for behaviour. It couples the code to
today's providers, and capability checks keep working when a provider gains a
feature.

## 4. Move objects between providers

`StorageProvider::copy` copies within one provider. To move an object from one
provider to another (local to S3, S3 to local, bucket to bucket through a
different provider instance), use `storageprims_ops::relay_copy`. It streams one
object through a bounded buffer, with an explicit size, buffer and time budget,
and reports exactly how far the write got:

```rust
let budget = TransferBudget::new(16 * 1024 * 1024, 64 * 1024, Duration::from_secs(60))?;
let request = RelayRequest::new(
    ReadRequest::Current("export.csv".into()),
    "exports/export.csv".into(),
    PutPrecondition::MustNotExist,
    MetadataPolicy::None,
    budget,
);
let cancel = Cancellation::new(); // keep a clone to cancel from elsewhere
match relay_copy(&staging, destination.as_ref(), request, cancel.clone()).await {
    Ok(receipt) => println!("relayed {} bytes", receipt.destination_bytes_handed_off()),
    Err(failure) => match failure.commit() {
        // This attempt published nothing. Retry only if failure.kind() is
        // transient: an admission refusal or an existing destination will
        // fail the same way again.
        CommitKnowledge::NotStarted | CommitKnowledge::NotCommitted => { /* maybe retry */ }
        // Committed, Unknown, or a future variant: the destination may hold
        // the object; check it before any retry.
        _ => { /* reconcile */ }
    },
}
```

Notes:

- There is no default budget. `TransferBudget::new` takes the maximum object
  size, a buffer size of 1 byte to 1 MiB, and a timeout.
- The source must offer `read_responses()` and the destination
  `admitted_puts()`. Both built-in providers do.
- Use `MetadataPolicy::None` when the destination is a local provider;
  `ContentTypeOnly` and `ContentTypeAndUserMetadata` are refused for
  destinations that cannot store them.
- Cancel a transfer by calling `cancel()` on a clone of the `Cancellation` you
  passed in.

## 5. A common shape: stage locally, publish remotely

Write and validate output under a local root with exactly the code that would
write to S3, then relay the finished objects to the bucket with a create-only
precondition. If publication fails, the local copy is intact and the
`CommitKnowledge` tells you whether the remote side needs checking before a
retry. Tests run the whole pipeline with two local roots and no network.

## See also

- [How the libraries fit together](how-the-libraries-fit.md)
- [The local filesystem provider](local-provider.md)
- [Testing with the local provider](testing-with-the-local-provider.md)
- [DDR-0011 bounded relay](../decisions/DDR-0011-bounded-relay.md)
