#![cfg(feature = "integration")]

use std::collections::BTreeMap;
use std::io::Cursor;
use std::time::{SystemTime, UNIX_EPOCH};

use aws_credential_types::Credentials;
use aws_sdk_s3::Client;
use storageprims_core::{
    BoxedByteStream, CopyRequest, CredentialSource, GetRangeRequest, ProviderConfig, ProviderKind,
    PutOptions, StorageProvider, TargetConfig,
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
            },
        )
        .await
        .expect("put succeeds");
    assert_eq!(put_result.path, "fixtures/data.txt");
    assert!(put_result.etag.is_some());

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
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be monotonic enough for tests")
        .as_nanos()
}
