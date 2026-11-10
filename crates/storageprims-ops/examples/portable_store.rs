//! Location-independent storage code: the same functions run against a local
//! directory or an S3 bucket, chosen once from a URI.
//!
//! Run with no argument to use two temporary local roots:
//!
//! ```text
//! cargo run -p storageprims-ops --example portable_store
//! ```
//!
//! Or pass a destination URI, for example `file:///tmp/exports` (the directory
//! must exist; `file://` URIs take Unix-style absolute paths) or
//! `s3://my-bucket/exports` (uses the AWS default credential chain):
//!
//! ```text
//! cargo run -p storageprims-ops --example portable_store -- s3://my-bucket/exports
//! ```

use std::{io::Cursor, time::Duration};

use storageprims_core::{
    Cancellation, CommitKnowledge, ConflictKind, CredentialSource, ListOptions, ProviderConfig,
    ProviderKind, PutOptions, PutPrecondition, ReadRequest, StorageError, StorageProvider,
    StorageUri, TargetConfig,
};
use storageprims_local::LocalProvider;
use storageprims_ops::{relay_copy, MetadataPolicy, RelayRequest, TransferBudget};
use storageprims_s3::S3Provider;

/// Choose a provider at the edge of the program. Everything after this point
/// sees only `&dyn StorageProvider`.
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

/// Write an object once. A second call with the same key is reported, not
/// silently overwritten, on every provider that supports create-only puts.
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
        Err(StorageError::Conflict {
            kind: ConflictKind::AlreadyExists,
            ..
        }) => Ok(false),
        Err(error) => Err(error),
    }
}

/// Count objects under a prefix, following continuation tokens.
async fn count_objects(store: &dyn StorageProvider, prefix: &str) -> Result<usize, StorageError> {
    let mut count = 0;
    let mut continuation_token = None;
    loop {
        let page = store
            .list(ListOptions {
                prefix: Some(prefix.to_string()),
                continuation_token,
                max_keys: None,
            })
            .await?;
        count += page.objects.len();
        if !page.is_truncated {
            return Ok(count);
        }
        continuation_token = page.continuation_token;
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let scratch =
        std::env::temp_dir().join(format!("storageprims-portable-{}", std::process::id()));
    // Create the scratch root exclusively: an existing directory is refused, so
    // the cleanup below can only remove what this run created.
    std::fs::create_dir(&scratch)?;
    let staging_root = scratch.join("staging");
    let local_destination = scratch.join("destination");
    std::fs::create_dir(&staging_root)?;
    std::fs::create_dir(&local_destination)?;

    let result = run(
        &staging_root,
        &local_destination,
        std::env::args().nth(1).as_deref(),
    )
    .await;
    std::fs::remove_dir_all(&scratch)?;
    result
}

async fn run(
    staging_root: &std::path::Path,
    local_destination: &std::path::Path,
    destination_uri: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let staging = LocalProvider::new(staging_root)?;
    // With no URI, open the local destination from a native path, which works
    // on every platform (including Windows drive paths).
    let destination: Box<dyn StorageProvider> = match destination_uri {
        Some(uri) => open_store(uri).await?,
        None => Box::new(LocalProvider::new(local_destination)?),
    };

    // Stage a file locally, using the same code that would write to S3.
    let created = publish_once(&staging, "export.csv", b"id,value\n1,42\n".to_vec()).await?;
    println!("staged locally: {created}");

    // Move it to the destination with a bounded, single-object relay.
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
            // This attempt published nothing. Retry only if the failure is
            // transient: an admission refusal or an existing destination will
            // fail the same way again.
            CommitKnowledge::NotStarted | CommitKnowledge::NotCommitted => {
                return Err(format!("relay did not commit: {:?}", failure.kind()).into())
            }
            // Committed, Unknown, or a future variant: the destination may
            // hold the object; check it before any retry.
            _ => return Err(format!("relay outcome needs checking: {:?}", failure.kind()).into()),
        },
    }

    println!(
        "{} object(s) under exports/",
        count_objects(destination.as_ref(), "exports/").await?
    );
    Ok(())
}
