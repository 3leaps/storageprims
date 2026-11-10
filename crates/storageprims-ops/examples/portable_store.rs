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
//! must exist) or `s3://my-bucket/exports` (uses the AWS default credential
//! chain):
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
    let staging_root = scratch.join("staging");
    let local_destination = scratch.join("destination");
    std::fs::create_dir_all(&staging_root)?;
    std::fs::create_dir_all(&local_destination)?;

    let destination_uri = match std::env::args().nth(1) {
        Some(uri) => uri,
        None => format!("file://{}", local_destination.display()),
    };
    let result = run(&staging_root, &destination_uri).await;
    std::fs::remove_dir_all(&scratch)?;
    result
}

async fn run(
    staging_root: &std::path::Path,
    destination_uri: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let staging = LocalProvider::new(staging_root)?;
    let destination = open_store(destination_uri).await?;

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
    match relay_copy(&staging, destination.as_ref(), request, Cancellation::new()).await {
        Ok(receipt) => println!(
            "relayed {} bytes to {}",
            receipt.destination_bytes_handed_off(),
            destination_uri
        ),
        Err(failure) => match failure.commit() {
            // The destination may or may not hold the object; check before retrying.
            CommitKnowledge::Committed | CommitKnowledge::Unknown => {
                return Err(format!("relay outcome needs checking: {:?}", failure.kind()).into())
            }
            // Nothing was written by this attempt.
            _ => return Err(format!("relay did not commit: {:?}", failure.kind()).into()),
        },
    }

    println!(
        "{} object(s) under exports/",
        count_objects(destination.as_ref(), "exports/").await?
    );
    Ok(())
}
