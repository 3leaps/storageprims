# Testing with the local provider

Code that talks to object storage is usually tested against a mock, a container
emulator, or a real bucket. The local provider adds a fourth option: the real
`StorageProvider` implementation, on a temporary directory. Tests stay fast and
hermetic, and they exercise real behaviour (atomic replacement, create-only
races, listing and pagination, error variants) instead of a mock's idea of it.

## The pattern

1. Write application code against `&dyn StorageProvider` (or a generic
   `P: StorageProvider`), never against a concrete provider type.
2. In production, build an `S3Provider` (or a `LocalProvider`) once, at startup.
3. In tests, build a `LocalProvider` on a fresh temporary directory per test.

```rust
async fn record_result(
    store: &dyn StorageProvider,
    job_id: &str,
    result: &[u8],
) -> Result<bool, StorageError> {
    let options = PutOptions {
        content_length: Some(result.len() as u64),
        precondition: PutPrecondition::MustNotExist,
        ..PutOptions::default()
    };
    let key = format!("results/{job_id}.json");
    match store.put(&key, Box::new(Cursor::new(result.to_vec())), options).await {
        Ok(_) => Ok(true),
        Err(StorageError::Conflict { kind: ConflictKind::AlreadyExists, .. }) => Ok(false),
        Err(error) => Err(error),
    }
}

#[tokio::test]
async fn records_a_result_once() {
    let root = TempRoot::new();                     // a directory removed on drop
    let store = LocalProvider::new(root.path()).unwrap();

    assert!(record_result(&store, "job-1", b"{\"ok\":true}").await.unwrap());
    assert!(!record_result(&store, "job-1", b"{\"ok\":false}").await.unwrap());

    // The layout is plain files, so a test can also check the disk directly.
    let on_disk = std::fs::read(root.path().join("results").join("job-1.json")).unwrap();
    assert_eq!(on_disk, b"{\"ok\":true}");
}
```

The complete, compiled and executed version, including the `TempRoot` helper,
is [`crates/storageprims-local/tests/guide_testing_pattern.rs`](../../crates/storageprims-local/tests/guide_testing_pattern.rs).
A crate such as `tempfile` works just as well for the temporary directory.

Practical notes:

- Give every test its own root. Roots do not share state, so tests can run in
  parallel.
- Seed fixtures either through `put` or by writing files under the root before
  calling `LocalProvider::new`; both are visible to the provider.
- Write keys that also satisfy the local key rules (see
  [Keys](local-provider.md#keys)); the local provider refuses keys S3 would
  accept, which surfaces non-portable keys in tests rather than in production.
- Always set `content_length` on `put`. The S3 provider requires it; the local
  provider's plain `put` does not, so a test that omits it would pass locally
  and fail on S3.

## What a local test does not prove

The local provider implements the same contract, but not every optional feature
or every failure an S3 endpoint can produce. Keep a smaller suite against
S3 (or an S3-compatible emulator such as LocalStack) for:

| Behaviour                                                         | Why local tests cannot cover it                                                                             |
| ----------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------- |
| `content_type` and user metadata                                  | Local refuses them; code that sets them must be tested against S3                                           |
| `PutPrecondition::Match` and ETags                                | Local has no validators; `etag` is `None`                                                                   |
| Guarded reads and the `storageprims-ops` line and preview helpers | Local does not offer guarded reads                                                                          |
| Credentials, access denied, throttling, network failure           | Local has no authentication or network path                                                                 |
| Objects over 5 GiB, single-put limits                             | S3 limits single puts to 5 GiB; local does not                                                              |
| Delimiters other than `/`, empty directory prefixes               | Local accepts only `/`, and keeps reporting a directory as a common prefix after its last object is deleted |

A useful split is many fast tests against `LocalProvider` for application logic,
plus a few tests per provider feature against the real backend.

## Checking what a provider supports

When code paths differ by capability, test both branches by asking the provider
rather than by provider type:

```rust
if store.has_capability(Capability::ConditionalPut) {
    // create-only path
} else {
    // fallback path
}
```

`LocalProvider` advertises `ConditionalPut` on Linux, macOS, Android and
Windows, and `DelimiterListing` everywhere it runs. See
[Optional capabilities](how-the-libraries-fit.md#optional-capabilities).
