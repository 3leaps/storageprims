#![cfg(feature = "integration")]

use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use aws_credential_types::Credentials;
use aws_sdk_s3::Client;
use storageprims_core::{
    BoxedByteStream, Capability, ConflictKind, CopyRequest, CredentialSource, CredentialSourceKind,
    GetRangeRequest, ProbeScope, ProviderConfig, ProviderKind, PutOptions, PutPrecondition,
    StorageError, StorageProvider, TargetConfig,
};
use storageprims_s3::S3Provider;
use tokio::io::AsyncReadExt;

const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:4566";
const TEST_REGION: &str = "us-east-1";

#[tokio::test]
async fn s3_provider_round_trips_against_localstack() {
    let test_context = TestContext::new().await;
    let provider = test_context.provider().await;

    let mut metadata = BTreeMap::new();
    metadata.insert("suite".to_string(), "storageprims-s3".to_string());

    let put_result = provider
        .put(
            "fixtures/data.txt",
            boxed_reader("hello from localstack"),
            PutOptions {
                content_length: Some("hello from localstack".len() as u64),
                content_type: Some("text/plain".to_string()),
                metadata: metadata.clone(),
                precondition: PutPrecondition::None,
            },
        )
        .await
        .expect("put succeeds");
    assert_eq!(put_result.path, "fixtures/data.txt");
    assert!(put_result.etag.is_some());
    assert_eq!(put_result.size, Some("hello from localstack".len() as u64));

    let head = provider
        .head("fixtures/data.txt")
        .await
        .expect("head succeeds");
    assert_eq!(head.path, "fixtures/data.txt");
    assert_eq!(head.content_type.as_deref(), Some("text/plain"));
    assert_eq!(
        head.metadata.get("suite").map(String::as_str),
        Some("storageprims-s3")
    );

    let get = provider
        .get("fixtures/data.txt")
        .await
        .expect("get succeeds");
    let body = read_stream(get).await;
    assert_eq!(body, b"hello from localstack");

    let range = provider
        .get_range(GetRangeRequest {
            key: "fixtures/data.txt".to_string(),
            offset: 6,
            length: 4,
        })
        .await
        .expect("range succeeds");
    let range_body = read_stream(range).await;
    assert_eq!(range_body, b"from");

    let list = provider
        .list(storageprims_core::ListOptions {
            prefix: Some("fixtures/".to_string()),
            continuation_token: None,
            max_keys: Some(100),
        })
        .await
        .expect("list succeeds");
    assert!(list
        .objects
        .iter()
        .any(|object| object.path == "fixtures/data.txt"));

    let native_copy = provider
        .copy(CopyRequest {
            source: "fixtures/data.txt".to_string(),
            destination: "fixtures/copy with spaces+#.txt".to_string(),
        })
        .await
        .expect("native copy succeeds");
    assert_eq!(
        native_copy.strategy,
        storageprims_core::CopyStrategy::Native
    );

    let copied = provider
        .get("fixtures/copy with spaces+#.txt")
        .await
        .expect("get copied object succeeds");
    let copied_body = read_stream(copied).await;
    assert_eq!(copied_body, b"hello from localstack");

    provider
        .delete("fixtures/copy with spaces+#.txt")
        .await
        .expect("delete succeeds");
    provider
        .delete("fixtures/copy with spaces+#.txt")
        .await
        .expect("delete remains idempotent");
}

#[tokio::test]
async fn s3_provider_honors_conditional_put_against_localstack() {
    let test_context = TestContext::new().await;
    let provider = test_context.provider().await;
    assert!(provider.has_capability(Capability::ConditionalPut));

    let created = provider
        .put(
            "conditional/data.txt",
            boxed_reader("first"),
            PutOptions {
                content_length: Some(5),
                precondition: PutPrecondition::MustNotExist,
                ..PutOptions::default()
            },
        )
        .await
        .expect("create-only put succeeds for an absent object");
    assert_eq!(created.size, Some(5));
    let first_etag = created.etag.expect("put returns an ETag");

    let already_exists = provider
        .put(
            "conditional/data.txt",
            boxed_reader("second"),
            PutOptions {
                content_length: Some(6),
                precondition: PutPrecondition::MustNotExist,
                ..PutOptions::default()
            },
        )
        .await
        .expect_err("create-only put conflicts for an existing object");
    assert!(matches!(
        already_exists,
        StorageError::Conflict {
            kind: ConflictKind::AlreadyExists,
            ..
        }
    ));

    let replaced = provider
        .put(
            "conditional/data.txt",
            boxed_reader("replacement"),
            PutOptions {
                content_length: Some(11),
                precondition: PutPrecondition::Match { token: first_etag },
                ..PutOptions::default()
            },
        )
        .await
        .expect("matching conditional put succeeds");
    assert_eq!(replaced.size, Some(11));

    let mismatch = provider
        .put(
            "conditional/data.txt",
            boxed_reader("rejected"),
            PutOptions {
                content_length: Some(8),
                precondition: PutPrecondition::Match {
                    token: "\"not-the-current-etag\"".to_string(),
                },
                ..PutOptions::default()
            },
        )
        .await
        .expect_err("mismatching conditional put conflicts");
    assert!(matches!(
        mismatch,
        StorageError::Conflict {
            kind: ConflictKind::TokenMismatch,
            ..
        }
    ));

    let missing = provider
        .put(
            "conditional/missing.txt",
            boxed_reader("rejected"),
            PutOptions {
                content_length: Some(8),
                precondition: PutPrecondition::Match {
                    token: "\"expected-etag\"".to_string(),
                },
                ..PutOptions::default()
            },
        )
        .await
        .expect_err("match against a missing object should fail");
    assert!(matches!(missing, StorageError::NotFound { .. }));
}

#[tokio::test]
async fn s3_provider_rejects_invalid_match_before_transport_without_disclosure() {
    let sentinel = "sentinel-fragment";
    let token = format!("\"{sentinel}\nmatch-token\"");
    let provider = S3Provider::from_config(ProviderConfig {
        provider: ProviderKind::S3,
        target: TargetConfig {
            container: Some("bucket".to_string()),
            region: Some(TEST_REGION.to_string()),
            endpoint: Some("http://127.0.0.1:9".to_string()),
            force_path_style: Some(true),
            ..TargetConfig::default()
        },
        credentials: CredentialSource::InlineStatic {
            values: credentials_map(),
        },
    })
    .await
    .expect("provider config should be valid");

    let error = provider
        .put(
            "anything.txt",
            boxed_reader("payload"),
            PutOptions {
                content_length: Some(7),
                precondition: PutPrecondition::Match { token },
                ..PutOptions::default()
            },
        )
        .await
        .expect_err("invalid match token should fail before transport");
    assert!(matches!(error, StorageError::InvalidArgument { .. }));
    assert!(!error.to_string().contains(sentinel));
    assert!(!format!("{error:?}").contains(sentinel));
}

#[tokio::test]
async fn s3_transport_failure_is_not_a_precondition_conflict() {
    let provider = S3Provider::from_config(ProviderConfig {
        provider: ProviderKind::S3,
        target: TargetConfig {
            container: Some("bucket".to_string()),
            region: Some(TEST_REGION.to_string()),
            endpoint: Some("http://127.0.0.1:9".to_string()),
            force_path_style: Some(true),
            ..TargetConfig::default()
        },
        credentials: CredentialSource::InlineStatic {
            values: credentials_map(),
        },
    })
    .await
    .expect("provider config should be valid");

    let error = provider
        .put(
            "anything.txt",
            boxed_reader("payload"),
            PutOptions {
                content_length: Some(7),
                precondition: PutPrecondition::MustNotExist,
                ..PutOptions::default()
            },
        )
        .await
        .expect_err("unreachable endpoint should fail");
    assert!(matches!(error, StorageError::ProviderUnavailable { .. }));
}

#[tokio::test]
async fn s3_provider_paginates_list_results_against_localstack() {
    let test_context = TestContext::new().await;
    let provider = test_context.provider().await;

    for key in ["fixtures/a.txt", "fixtures/b.txt", "fixtures/c.txt"] {
        provider
            .put(
                key,
                boxed_reader(key),
                PutOptions {
                    content_length: Some(key.len() as u64),
                    ..PutOptions::default()
                },
            )
            .await
            .expect("seed object for pagination");
    }

    let first_page = provider
        .list(storageprims_core::ListOptions {
            prefix: Some("fixtures/".to_string()),
            continuation_token: None,
            max_keys: Some(2),
        })
        .await
        .expect("first page succeeds");
    assert_eq!(first_page.objects.len(), 2);
    assert!(first_page.is_truncated);
    assert!(first_page.continuation_token.is_some());

    let second_page = provider
        .list(storageprims_core::ListOptions {
            prefix: Some("fixtures/".to_string()),
            continuation_token: first_page.continuation_token.clone(),
            max_keys: Some(2),
        })
        .await
        .expect("second page succeeds");
    assert_eq!(second_page.objects.len(), 1);
    assert!(!second_page.is_truncated);
    assert!(second_page.continuation_token.is_none());

    let first_paths = first_page
        .objects
        .iter()
        .map(|object| object.path.as_str())
        .collect::<Vec<_>>();
    let second_paths = second_page
        .objects
        .iter()
        .map(|object| object.path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(first_paths, vec!["fixtures/a.txt", "fixtures/b.txt"]);
    assert_eq!(second_paths, vec!["fixtures/c.txt"]);
}

#[tokio::test]
async fn s3_provider_omits_zero_max_keys_and_rejects_overflow_before_transport() {
    let test_context = TestContext::new().await;
    let provider = test_context.provider().await;

    for key in ["defaults/a.txt", "defaults/b.txt"] {
        provider
            .put(
                key,
                boxed_reader(key),
                PutOptions {
                    content_length: Some(key.len() as u64),
                    ..PutOptions::default()
                },
            )
            .await
            .expect("seed object for default page-size test");
    }

    let page = provider
        .list(storageprims_core::ListOptions {
            prefix: Some("defaults/".to_string()),
            continuation_token: None,
            max_keys: Some(0),
        })
        .await
        .expect("zero max_keys uses the provider default");
    assert_eq!(page.objects.len(), 2);

    let unreachable = S3Provider::from_config(ProviderConfig {
        provider: ProviderKind::S3,
        target: TargetConfig {
            container: Some("no-network".to_string()),
            region: Some(TEST_REGION.to_string()),
            endpoint: Some("http://127.0.0.1:9".to_string()),
            force_path_style: Some(true),
            ..TargetConfig::default()
        },
        credentials: CredentialSource::InlineStatic {
            values: credentials_map(),
        },
    })
    .await
    .expect("provider config should be valid");

    let error = unreachable
        .list(storageprims_core::ListOptions {
            prefix: None,
            continuation_token: None,
            max_keys: Some(u32::MAX),
        })
        .await
        .expect_err("overflow must fail before transport");
    assert!(matches!(
        error,
        StorageError::InvalidArgument {
            operation: Some(storageprims_core::StorageOperation::List),
            ref argument,
            ..
        } if argument == "max_keys"
    ));
}

#[tokio::test]
async fn s3_provider_contains_rooted_list_to_exact_prefix_segment() {
    let test_context = TestContext::new().await;
    let unrooted_provider = test_context.provider().await;

    for key in ["team/x", "team2/x"] {
        unrooted_provider
            .put(
                key,
                boxed_reader(key),
                PutOptions {
                    content_length: Some(key.len() as u64),
                    ..PutOptions::default()
                },
            )
            .await
            .expect("seed collision fixture");
    }

    let rooted_provider = test_context.provider_with_root("team").await;
    let rooted_list = rooted_provider
        .list(storageprims_core::ListOptions::default())
        .await
        .expect("rooted list succeeds");
    let rooted_paths = rooted_list
        .objects
        .iter()
        .map(|object| object.path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(rooted_paths, vec!["x"]);

    let unrooted_list = unrooted_provider
        .list(storageprims_core::ListOptions::default())
        .await
        .expect("unrooted list succeeds");
    let unrooted_paths = unrooted_list
        .objects
        .iter()
        .map(|object| object.path.as_str())
        .collect::<Vec<_>>();
    assert!(unrooted_paths.contains(&"team/x"));
    assert!(unrooted_paths.contains(&"team2/x"));
}

#[tokio::test]
async fn s3_provider_exercises_range_boundaries_against_localstack() {
    let test_context = TestContext::new().await;
    let provider = test_context.provider().await;

    provider
        .put(
            "ranges/data.bin",
            boxed_reader("abcdef"),
            PutOptions {
                content_length: Some(6),
                ..PutOptions::default()
            },
        )
        .await
        .expect("seed range object");

    let first_byte = provider
        .get_range(GetRangeRequest {
            key: "ranges/data.bin".to_string(),
            offset: 0,
            length: 1,
        })
        .await
        .expect("first byte range succeeds");
    assert_eq!(read_stream(first_byte).await, b"a");

    let final_byte = provider
        .get_range(GetRangeRequest {
            key: "ranges/data.bin".to_string(),
            offset: 5,
            length: 1,
        })
        .await
        .expect("final byte range succeeds");
    assert_eq!(read_stream(final_byte).await, b"f");

    let whole_object = provider
        .get_range(GetRangeRequest {
            key: "ranges/data.bin".to_string(),
            offset: 0,
            length: 6,
        })
        .await
        .expect("full-object range succeeds");
    assert_eq!(read_stream(whole_object).await, b"abcdef");

    let zero_length = match provider
        .get_range(GetRangeRequest {
            key: "ranges/data.bin".to_string(),
            offset: 0,
            length: 0,
        })
        .await
    {
        Ok(_) => panic!("zero-length range should fail"),
        Err(error) => error,
    };
    assert!(matches!(zero_length, StorageError::InvalidArgument { .. }));

    let out_of_range = match provider
        .get_range(GetRangeRequest {
            key: "ranges/data.bin".to_string(),
            offset: 9,
            length: 1,
        })
        .await
    {
        Ok(_) => panic!("out-of-range request should fail"),
        Err(error) => error,
    };
    assert!(
        !matches!(out_of_range, StorageError::ProviderUnavailable { .. }),
        "out-of-range request should fail as a service error, not transport failure: {out_of_range:?}"
    );
}

#[tokio::test]
async fn s3_provider_relay_copy_across_buckets() {
    let source_context = TestContext::new().await;
    let destination_context = TestContext::new().await;
    let provider = source_context.provider().await;

    provider
        .put(
            "relay/source.bin",
            boxed_reader("relay-payload"),
            PutOptions {
                content_length: Some("relay-payload".len() as u64),
                ..PutOptions::default()
            },
        )
        .await
        .expect("seed source object");

    let destination_uri = format!("s3://{}/relay/destination.bin", destination_context.bucket);
    let copy_result = provider
        .copy(CopyRequest {
            source: "relay/source.bin".to_string(),
            destination: destination_uri.clone(),
        })
        .await
        .expect("relay copy succeeds");

    assert_eq!(copy_result.strategy, storageprims_core::CopyStrategy::Relay);
    assert_eq!(copy_result.bytes_copied, Some("relay-payload".len() as u64));

    let destination_provider = destination_context.provider().await;
    let copied = destination_provider
        .get("relay/destination.bin")
        .await
        .expect("destination object exists");
    let copied_body = read_stream(copied).await;
    assert_eq!(copied_body, b"relay-payload");
}

#[tokio::test]
async fn s3_provider_surfaces_missing_key_and_bucket_failures() {
    let test_context = TestContext::new().await;
    let provider = test_context.provider().await;

    let missing_key = provider
        .head("missing.txt")
        .await
        .expect_err("missing key should fail");
    assert!(matches!(missing_key, StorageError::NotFound { .. }));

    let missing_bucket_provider = S3Provider::from_config(ProviderConfig {
        provider: ProviderKind::S3,
        target: TargetConfig {
            container: Some(format!("missing-{}", unique_suffix())),
            region: Some(TEST_REGION.to_string()),
            endpoint: Some(test_context.endpoint.clone()),
            force_path_style: Some(true),
            ..TargetConfig::default()
        },
        credentials: CredentialSource::InlineStatic {
            values: credentials_map(),
        },
    })
    .await
    .expect("missing bucket provider config should be valid");
    let missing_bucket = missing_bucket_provider
        .head("whatever.txt")
        .await
        .expect_err("missing bucket should fail");
    assert!(
        !matches!(missing_bucket, StorageError::ProviderUnavailable { .. }),
        "missing bucket should fail as a service error, not transport failure: {missing_bucket:?}"
    );
}

#[tokio::test]
async fn s3_provider_surfaces_provider_unavailable_for_unreachable_endpoint() {
    let provider = S3Provider::from_config(ProviderConfig {
        provider: ProviderKind::S3,
        target: TargetConfig {
            container: Some("bucket".to_string()),
            region: Some(TEST_REGION.to_string()),
            endpoint: Some("http://127.0.0.1:9".to_string()),
            force_path_style: Some(true),
            ..TargetConfig::default()
        },
        credentials: CredentialSource::InlineStatic {
            values: credentials_map(),
        },
    })
    .await
    .expect("provider config should be valid");

    let error = provider
        .head("anything.txt")
        .await
        .expect_err("unreachable endpoint should fail");
    assert!(matches!(error, StorageError::ProviderUnavailable { .. }));
}

#[tokio::test]
async fn s3_provider_probe_checks_configured_bucket_against_localstack() {
    let test_context = TestContext::new().await;
    let provider = test_context.provider().await;

    let result = provider.probe().await.expect("probe succeeds");
    assert_eq!(result.provider, ProviderKind::S3);
    assert_eq!(result.probe_method, "s3:HeadBucket");
    assert_eq!(
        result.endpoint.as_deref(),
        Some(test_context.endpoint.as_str())
    );
    assert_eq!(result.credential_source, CredentialSourceKind::InlineStatic);
    assert_eq!(result.scope, ProbeScope::ConfiguredContainer);
    assert!(result.latency_ms < 30_000);
    assert_eq!(
        result.capabilities,
        vec![
            Capability::CredentialProbe,
            Capability::ConditionalPut,
            Capability::GuardedRead,
        ]
    );
}

struct TestContext {
    bucket: String,
    endpoint: String,
}

impl TestContext {
    async fn new() -> Self {
        let endpoint = std::env::var("STORAGEPRIMS_S3_TEST_ENDPOINT")
            .unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string());
        let bucket = format!("storageprims-it-{}", unique_suffix());
        create_bucket(&endpoint, &bucket).await;
        Self { bucket, endpoint }
    }

    async fn provider(&self) -> S3Provider {
        S3Provider::from_config(self.config())
            .await
            .expect("provider config should be valid")
    }

    async fn provider_with_root(&self, root_prefix: &str) -> S3Provider {
        let mut config = self.config();
        config.target.root_prefix = Some(root_prefix.to_string());
        S3Provider::from_config(config)
            .await
            .expect("rooted provider config should be valid")
    }

    fn config(&self) -> ProviderConfig {
        ProviderConfig {
            provider: ProviderKind::S3,
            target: TargetConfig {
                container: Some(self.bucket.clone()),
                region: Some(TEST_REGION.to_string()),
                endpoint: Some(self.endpoint.clone()),
                force_path_style: Some(true),
                ..TargetConfig::default()
            },
            credentials: CredentialSource::InlineStatic {
                values: credentials_map(),
            },
        }
    }
}

async fn create_bucket(endpoint: &str, bucket: &str) {
    let config = aws_sdk_s3::config::Builder::new()
        .behavior_version_latest()
        .region(aws_sdk_s3::config::Region::new(TEST_REGION))
        .endpoint_url(endpoint)
        .force_path_style(true)
        .credentials_provider(Credentials::from_keys("test", "test", None))
        .build();
    let client = Client::from_conf(config);
    client
        .create_bucket()
        .bucket(bucket)
        .send()
        .await
        .expect("create test bucket");
}

fn credentials_map() -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    values.insert("AWS_ACCESS_KEY_ID".to_string(), "test".to_string());
    values.insert("AWS_SECRET_ACCESS_KEY".to_string(), "test".to_string());
    values
}

fn boxed_reader(value: &str) -> BoxedByteStream {
    Box::new(Cursor::new(value.as_bytes().to_vec()))
}

async fn read_stream(mut stream: BoxedByteStream) -> Vec<u8> {
    let mut buffer = Vec::new();
    stream
        .read_to_end(&mut buffer)
        .await
        .expect("stream should be readable");
    buffer
}

fn unique_suffix() -> u128 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be monotonic enough for tests")
        .as_nanos();
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed) as u128;

    (timestamp << 16) ^ counter ^ (std::process::id() as u128)
}
