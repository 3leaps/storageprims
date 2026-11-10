#![cfg(feature = "integration")]
//! LocalStack integration tests for bounded one-object relay across independent S3 and local handles.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aws_credential_types::Credentials;
use aws_sdk_s3::types::{BucketVersioningStatus, VersioningConfiguration};
use aws_sdk_s3::Client;
use storageprims_core::{
    BoxedByteStream, Cancellation, CommitKnowledge, ConflictKind, CopyStrategy, CredentialSource,
    ExtensionErrorKind, GuardedReadProvider, ProviderConfig, ProviderKind, PutOptions,
    PutPrecondition, ReadEnforcement, ReadRequest, SourceSelector, StorageError, StorageProvider,
    TargetConfig,
};
use storageprims_local::LocalProvider;
use storageprims_ops::{
    relay_copy, MetadataPolicy, RelayRequest, TransferBudget, TransferFailureKind,
};
use storageprims_s3::S3Provider;
use tokio::io::AsyncReadExt;

const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:4566";
const TEST_REGION: &str = "us-east-1";

fn endpoint_url() -> String {
    std::env::var("STORAGEPRIMS_S3_TEST_ENDPOINT").unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string())
}

fn unique_suffix() -> u128 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let count = COUNTER.fetch_add(1, Ordering::Relaxed) as u128;
    (ts << 16) ^ count ^ (std::process::id() as u128)
}

fn admin_client(endpoint: &str) -> Client {
    let config = aws_sdk_s3::config::Builder::new()
        .behavior_version_latest()
        .region(aws_sdk_s3::config::Region::new(TEST_REGION))
        .endpoint_url(endpoint)
        .force_path_style(true)
        .credentials_provider(Credentials::from_keys("test", "test", None))
        .build();
    Client::from_conf(config)
}

async fn create_bucket(endpoint: &str, bucket: &str, versioned: bool) {
    let client = admin_client(endpoint);
    client
        .create_bucket()
        .bucket(bucket)
        .send()
        .await
        .expect("create test bucket");
    if versioned {
        client
            .put_bucket_versioning()
            .bucket(bucket)
            .versioning_configuration(
                VersioningConfiguration::builder()
                    .status(BucketVersioningStatus::Enabled)
                    .build(),
            )
            .send()
            .await
            .expect("enable bucket versioning");
    }
}

async fn independent_s3_provider(
    endpoint: &str,
    bucket: &str,
    root_prefix: &str,
    access_key: &str,
    secret_key: &str,
) -> S3Provider {
    S3Provider::from_config(ProviderConfig {
        provider: ProviderKind::S3,
        target: TargetConfig {
            container: Some(bucket.to_string()),
            root_prefix: Some(root_prefix.to_string()),
            region: Some(TEST_REGION.to_string()),
            endpoint: Some(endpoint.to_string()),
            force_path_style: Some(true),
            ..TargetConfig::default()
        },
        credentials: CredentialSource::InlineStatic {
            values: BTreeMap::from([
                ("AWS_ACCESS_KEY_ID".to_string(), access_key.to_string()),
                ("AWS_SECRET_ACCESS_KEY".to_string(), secret_key.to_string()),
            ]),
        },
    })
    .await
    .expect("independent S3 handle builds")
}

fn reader_from_bytes(bytes: &[u8]) -> BoxedByteStream {
    Box::new(Cursor::new(bytes.to_vec()))
}

async fn collect_bytes(mut stream: BoxedByteStream) -> Vec<u8> {
    let mut out = Vec::new();
    stream.read_to_end(&mut out).await.expect("stream reads");
    out
}

#[tokio::test]
async fn relay_copy_between_two_independent_s3_handles_enforces_guards_predicates_and_metadata() {
    let endpoint = endpoint_url();
    let src_bucket = format!("storageprims-relay-src-{}", unique_suffix());
    let dst_bucket = format!("storageprims-relay-dst-{}", unique_suffix());
    create_bucket(&endpoint, &src_bucket, true).await;
    create_bucket(&endpoint, &dst_bucket, false).await;

    let src = independent_s3_provider(
        &endpoint,
        &src_bucket,
        "tenant-alpha/exports",
        "alpha-access-key",
        "alpha-secret-key",
    )
    .await;
    let dst = independent_s3_provider(
        &endpoint,
        &dst_bucket,
        "tenant-bravo/imports",
        "bravo-access-key",
        "bravo-secret-key",
    )
    .await;

    let mut v1_meta = BTreeMap::new();
    v1_meta.insert("origin".to_string(), "alpha".to_string());
    v1_meta.insert("revision".to_string(), "1".to_string());
    let v1_payload = b"version-one-payload-across-handles";
    src.put(
        "dataset/manifest.json",
        reader_from_bytes(v1_payload),
        PutOptions {
            content_length: Some(v1_payload.len() as u64),
            content_type: Some("application/json".to_string()),
            metadata: v1_meta.clone(),
            precondition: PutPrecondition::MustNotExist,
        },
    )
    .await
    .expect("seed v1 source object");

    let v1_obs = src
        .observe_source("dataset/manifest.json")
        .await
        .expect("observe v1 source");
    let v1_etag = v1_obs.receipt.validator.clone().expect("v1 validator");
    let v1_version = v1_obs
        .receipt
        .native_version
        .clone()
        .expect("v1 version_id");

    // 1. Validator-guarded S3 -> S3 relay with MustNotExist and full metadata preservation.
    let v1_val_sel = src
        .bind_guarded_read(
            "dataset/manifest.json",
            SourceSelector::ValidatorMatch {
                token: v1_etag.clone(),
            },
        )
        .expect("bind validator selection");

    let budget = TransferBudget::new(1024 * 1024, 16, Duration::from_secs(15)).unwrap();
    let first_relay = relay_copy(
        &src,
        &dst,
        RelayRequest::new(
            ReadRequest::Guarded(v1_val_sel),
            "staged/manifest.json".to_string(),
            PutPrecondition::MustNotExist,
            MetadataPolicy::ContentTypeAndUserMetadata,
            budget,
        ),
        Cancellation::new(),
    )
    .await
    .expect("guarded S3-to-S3 relay succeeds");

    assert_eq!(first_relay.strategy(), CopyStrategy::Relay);
    assert_eq!(first_relay.enforcement(), ReadEnforcement::ValidatorMatch);
    assert_eq!(first_relay.source().path, "dataset/manifest.json");
    assert_eq!(first_relay.destination().path, "staged/manifest.json");
    assert_eq!(first_relay.source_bytes_consumed(), v1_payload.len() as u64);
    assert_eq!(
        first_relay.destination_bytes_handed_off(),
        v1_payload.len() as u64
    );
    let dst_first_etag = first_relay
        .destination()
        .etag
        .clone()
        .expect("destination put returns ETag");

    let dst_head = dst
        .head("staged/manifest.json")
        .await
        .expect("destination head succeeds");
    assert_eq!(dst_head.content_type.as_deref(), Some("application/json"));
    assert_eq!(dst_head.metadata, v1_meta);
    assert_eq!(
        collect_bytes(dst.get("staged/manifest.json").await.unwrap()).await,
        v1_payload
    );

    // 2. Repeating MustNotExist against the populated destination fails with NotCommitted AlreadyExists.
    let duplicate = relay_copy(
        &src,
        &dst,
        RelayRequest::new(
            ReadRequest::Current("dataset/manifest.json".to_string()),
            "staged/manifest.json".to_string(),
            PutPrecondition::MustNotExist,
            MetadataPolicy::None,
            budget,
        ),
        Cancellation::new(),
    )
    .await
    .expect_err("create-only destination precondition must reject overwrite");
    assert_eq!(duplicate.kind(), TransferFailureKind::Destination);
    assert_eq!(duplicate.commit(), CommitKnowledge::NotCommitted);
    assert!(matches!(
        duplicate.cause().and_then(|e| e.cause()),
        Some(StorageError::Conflict {
            kind: ConflictKind::AlreadyExists,
            ..
        })
    ));

    // 3. Mutate source to v2: stale validator guard fails at SourceOpen (NotStarted),
    // while NativeVersion guard still relays exact v1 bytes using Match precondition on destination!
    let mut v2_meta = BTreeMap::new();
    v2_meta.insert("origin".to_string(), "alpha".to_string());
    v2_meta.insert("revision".to_string(), "2".to_string());
    let v2_payload = b"version-two-updated-payload";
    src.put(
        "dataset/manifest.json",
        reader_from_bytes(v2_payload),
        PutOptions {
            content_length: Some(v2_payload.len() as u64),
            content_type: Some("application/json".to_string()),
            metadata: v2_meta.clone(),
            precondition: PutPrecondition::None,
        },
    )
    .await
    .expect("write v2 source object");

    let stale_sel = src
        .bind_guarded_read(
            "dataset/manifest.json",
            SourceSelector::ValidatorMatch { token: v1_etag },
        )
        .expect("bind stale validator selection");
    let stale_err = relay_copy(
        &src,
        &dst,
        RelayRequest::new(
            ReadRequest::Guarded(stale_sel),
            "staged/never-created.json".to_string(),
            PutPrecondition::MustNotExist,
            MetadataPolicy::None,
            budget,
        ),
        Cancellation::new(),
    )
    .await
    .expect_err("stale source validator must fail before writing destination");
    assert_eq!(stale_err.kind(), TransferFailureKind::SourceOpen);
    assert_eq!(stale_err.commit(), CommitKnowledge::NotStarted);
    assert!(matches!(
        stale_err.cause().and_then(|e| e.cause()),
        Some(StorageError::Conflict {
            kind: ConflictKind::TokenMismatch,
            ..
        })
    ));
    assert!(matches!(
        dst.head("staged/never-created.json").await,
        Err(StorageError::NotFound { .. })
    ));

    // Relay current v2 into destination using destination PutPrecondition::Match(dst_first_etag).
    let v2_relay = relay_copy(
        &src,
        &dst,
        RelayRequest::new(
            ReadRequest::Current("dataset/manifest.json".to_string()),
            "staged/manifest.json".to_string(),
            PutPrecondition::Match {
                token: dst_first_etag.clone(),
            },
            MetadataPolicy::ContentTypeAndUserMetadata,
            budget,
        ),
        Cancellation::new(),
    )
    .await
    .expect("conditional Match update on destination succeeds");
    let dst_second_etag = v2_relay
        .destination()
        .etag
        .clone()
        .expect("updated destination ETag");
    assert_eq!(
        collect_bytes(dst.get("staged/manifest.json").await.unwrap()).await,
        v2_payload
    );

    // Pinned NativeVersion v1 relay restores v1 bytes when matching dst_second_etag.
    let v1_ver_sel = src
        .bind_guarded_read(
            "dataset/manifest.json",
            SourceSelector::NativeVersion { token: v1_version },
        )
        .expect("bind native version selection");
    let pinned_relay = relay_copy(
        &src,
        &dst,
        RelayRequest::new(
            ReadRequest::Guarded(v1_ver_sel),
            "staged/manifest.json".to_string(),
            PutPrecondition::Match {
                token: dst_second_etag,
            },
            MetadataPolicy::ContentTypeAndUserMetadata,
            budget,
        ),
        Cancellation::new(),
    )
    .await
    .expect("pinned NativeVersion S3-to-S3 relay succeeds");
    assert_eq!(pinned_relay.enforcement(), ReadEnforcement::NativeVersion);
    assert_eq!(
        collect_bytes(dst.get("staged/manifest.json").await.unwrap()).await,
        v1_payload
    );
    let restored_head = dst.head("staged/manifest.json").await.unwrap();
    assert_eq!(restored_head.metadata, v1_meta);
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn relay_copy_bidirectional_between_local_provider_and_localstack_s3() {
    let endpoint = endpoint_url();
    let bucket = format!("storageprims-relay-hybrid-{}", unique_suffix());
    create_bucket(&endpoint, &bucket, false).await;

    let s3 = independent_s3_provider(
        &endpoint,
        &bucket,
        "hybrid-root",
        "hybrid-access-key",
        "hybrid-secret-key",
    )
    .await;

    let root_dir =
        std::env::temp_dir().join(format!("storageprims-it-hybrid-local-{}", unique_suffix()));
    std::fs::create_dir_all(&root_dir).expect("create local root");
    let local = LocalProvider::new(&root_dir).expect("open local provider");

    let budget = TransferBudget::new(1024 * 1024, 32, Duration::from_secs(15)).unwrap();

    // 1. Local -> S3 (non-empty and zero-byte payloads)
    let local_payload = b"rooted-local-bytes-relayed-to-localstack-s3";
    local
        .put(
            "outbox/blob.bin",
            reader_from_bytes(local_payload),
            PutOptions {
                content_length: Some(local_payload.len() as u64),
                ..PutOptions::default()
            },
        )
        .await
        .expect("seed local blob");
    local
        .put(
            "outbox/empty.bin",
            reader_from_bytes(b""),
            PutOptions {
                content_length: Some(0),
                ..PutOptions::default()
            },
        )
        .await
        .expect("seed local empty");

    let up_receipt = relay_copy(
        &local,
        &s3,
        RelayRequest::new(
            ReadRequest::Current("outbox/blob.bin".to_string()),
            "remote/blob.bin".to_string(),
            PutPrecondition::MustNotExist,
            MetadataPolicy::None,
            budget,
        ),
        Cancellation::new(),
    )
    .await
    .expect("local -> S3 relay succeeds");
    assert_eq!(
        up_receipt.destination_bytes_handed_off(),
        local_payload.len() as u64
    );
    assert_eq!(
        collect_bytes(s3.get("remote/blob.bin").await.unwrap()).await,
        local_payload
    );

    let empty_receipt = relay_copy(
        &local,
        &s3,
        RelayRequest::new(
            ReadRequest::Current("outbox/empty.bin".to_string()),
            "remote/empty.bin".to_string(),
            PutPrecondition::MustNotExist,
            MetadataPolicy::None,
            budget,
        ),
        Cancellation::new(),
    )
    .await
    .expect("zero-byte local -> S3 relay succeeds");
    assert_eq!(empty_receipt.destination_bytes_handed_off(), 0);
    assert_eq!(
        collect_bytes(s3.get("remote/empty.bin").await.unwrap()).await,
        b""
    );

    // 2. S3 -> Local refuses unsupported metadata preservation before touching destination,
    // and succeeds with MetadataPolicy::None + guarded S3 ValidatorMatch!
    let obs = s3
        .observe_source("remote/blob.bin")
        .await
        .expect("observe S3 blob");
    let s3_sel = s3
        .bind_guarded_read(
            "remote/blob.bin",
            SourceSelector::ValidatorMatch {
                token: obs.receipt.validator.expect("S3 validator"),
            },
        )
        .expect("bind S3 validator selection");

    let refused = relay_copy(
        &s3,
        &local,
        RelayRequest::new(
            ReadRequest::Current("remote/blob.bin".to_string()),
            "inbox/refused.bin".to_string(),
            PutPrecondition::MustNotExist,
            MetadataPolicy::ContentTypeOnly,
            budget,
        ),
        Cancellation::new(),
    )
    .await
    .expect_err("local destination must refuse ContentTypeOnly");
    assert_eq!(refused.kind(), TransferFailureKind::Admission);
    assert_eq!(refused.commit(), CommitKnowledge::NotStarted);
    assert_eq!(
        refused.cause().map(|e| e.kind()),
        Some(ExtensionErrorKind::UnsupportedMetadata)
    );
    assert!(matches!(
        local.head("inbox/refused.bin").await,
        Err(StorageError::NotFound { .. })
    ));

    let down_receipt = relay_copy(
        &s3,
        &local,
        RelayRequest::new(
            ReadRequest::Guarded(s3_sel),
            "inbox/blob.bin".to_string(),
            PutPrecondition::MustNotExist,
            MetadataPolicy::None,
            budget,
        ),
        Cancellation::new(),
    )
    .await
    .expect("guarded S3 -> local relay succeeds");
    assert_eq!(down_receipt.enforcement(), ReadEnforcement::ValidatorMatch);
    assert_eq!(
        down_receipt.destination_bytes_handed_off(),
        local_payload.len() as u64
    );
    assert_eq!(
        collect_bytes(local.get("inbox/blob.bin").await.unwrap()).await,
        local_payload
    );

    let _ = std::fs::remove_dir_all(&root_dir);
}
