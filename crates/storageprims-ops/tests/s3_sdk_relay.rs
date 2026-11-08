//! Real S3 SDK and local-provider relay composition tests using scripted HTTP endpoints.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use storageprims_core::{
    Cancellation, CommitKnowledge, CredentialSource, ExtensionErrorKind, ProviderConfig,
    ProviderKind, PutOptions, PutPrecondition, ReadEnforcement, ReadRequest, StorageProvider,
    TargetConfig,
};
use storageprims_local::LocalProvider;
use storageprims_ops::{
    relay_copy, MetadataPolicy, RelayRequest, TransferBudget, TransferFailureKind,
};
use storageprims_s3::S3Provider;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn inline_creds(access_key: &str, secret_key: &str) -> CredentialSource {
    CredentialSource::InlineStatic {
        values: BTreeMap::from([
            ("AWS_ACCESS_KEY_ID".to_string(), access_key.to_string()),
            ("AWS_SECRET_ACCESS_KEY".to_string(), secret_key.to_string()),
        ]),
    }
}

async fn build_s3_handle(
    endpoint: String,
    bucket: &str,
    root_prefix: Option<&str>,
    access_key: &str,
) -> S3Provider {
    S3Provider::from_config(ProviderConfig {
        provider: ProviderKind::S3,
        target: TargetConfig {
            container: Some(bucket.to_string()),
            root_prefix: root_prefix.map(ToString::to_string),
            region: Some("us-east-1".to_string()),
            endpoint: Some(endpoint),
            force_path_style: Some(true),
            ..TargetConfig::default()
        },
        credentials: inline_creds(access_key, "synthetic-secret"),
    })
    .await
    .expect("S3 provider builds")
}

#[tokio::test]
async fn relay_copy_enforces_single_sdk_put_attempt_and_withholds_invalid_source_tail() {
    // 1. Source GET succeeds (replayable 0-byte and streamed 9-byte), destination PUT returns 500:
    // listener remains open until relay_copy returns; SDK makes 1 PUT attempt per call and reports Unknown.
    let src_listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let src_addr = src_listener.local_addr().expect("addr");
    let src_server = tokio::spawn(async move {
        for body in ["", "ninebytes"] {
            let (mut sock, _) = src_listener.accept().await.expect("accepts");
            let mut buf = [0_u8; 2048];
            let _ = sock.read(&mut buf).await;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/plain\r\nETag: \"src-etag\"\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            sock.write_all(resp.as_bytes()).await.expect("writes");
        }
    });

    let dst_listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let dst_addr = dst_listener.local_addr().expect("addr");
    let dst_attempts = Arc::new(AtomicUsize::new(0));
    let dst_attempts_clone = dst_attempts.clone();
    let dst_stop = Arc::new(tokio::sync::Notify::new());
    let dst_stop_clone = dst_stop.clone();
    let dst_server = tokio::spawn(async move {
        loop {
            tokio::select! {
                biased;
                _ = dst_stop_clone.notified() => break,
                accepted = dst_listener.accept() => {
                    let Ok((mut sock, _)) = accepted else { break };
                    dst_attempts_clone.fetch_add(1, Ordering::SeqCst);
                    let mut buf = [0_u8; 4096];
                    let _ = sock.read(&mut buf).await;
                    let xml = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Error><Code>InternalError</Code><Message>err</Message></Error>";
                    let resp = format!(
                        "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        xml.len(),
                        xml
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                }
            }
        }
    });

    let src_s3 = build_s3_handle(
        format!("http://{src_addr}"),
        "src-bucket",
        Some("src-root"),
        "src-ak",
    )
    .await;
    let dst_s3 = build_s3_handle(
        format!("http://{dst_addr}"),
        "dst-bucket",
        Some("dst-root"),
        "dst-ak",
    )
    .await;

    let budget = TransferBudget::new(64, 4, Duration::from_secs(5)).expect("valid budget");
    let fail_zero = relay_copy(
        &src_s3,
        &dst_s3,
        RelayRequest::new(
            ReadRequest::Current("empty.txt".to_string()),
            "copy-empty.txt".to_string(),
            PutPrecondition::MustNotExist,
            MetadataPolicy::ContentTypeOnly,
            budget,
        ),
        Cancellation::new(),
    )
    .await
    .expect_err("500 destination response on replayable 0-byte PUT must fail without retry");
    assert_eq!(dst_attempts.swap(0, Ordering::SeqCst), 1);
    assert_eq!(fail_zero.kind(), TransferFailureKind::Destination);
    assert_eq!(fail_zero.commit(), CommitKnowledge::Unknown);

    let fail = relay_copy(
        &src_s3,
        &dst_s3,
        RelayRequest::new(
            ReadRequest::Current("item.txt".to_string()),
            "copy.txt".to_string(),
            PutPrecondition::MustNotExist,
            MetadataPolicy::ContentTypeOnly,
            budget,
        ),
        Cancellation::new(),
    )
    .await
    .expect_err("500 destination response must fail");

    src_server.await.expect("src server done");
    dst_stop.notify_one();
    dst_server.await.expect("dst server done");
    assert_eq!(dst_attempts.load(Ordering::SeqCst), 1);
    assert_eq!(fail.kind(), TransferFailureKind::Destination);
    assert_eq!(fail.commit(), CommitKnowledge::Unknown);
    assert_eq!(fail.source_bytes_consumed(), 9);
    assert_eq!(fail.destination_bytes_handed_off(), 9);

    // 1b. x-amz-missing-meta != 0 on source GET fails ContentTypeAndUserMetadata admission before destination dispatch.
    let mm_src_listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let mm_src_addr = mm_src_listener.local_addr().expect("addr");
    let mm_src_server = tokio::spawn(async move {
        let (mut sock, _) = mm_src_listener.accept().await.expect("accepts");
        let mut buf = [0_u8; 2048];
        let _ = sock.read(&mut buf).await;
        let resp = concat!(
            "HTTP/1.1 200 OK\r\n",
            "Content-Length: 5\r\n",
            "Content-Type: text/plain\r\n",
            "ETag: \"mm-etag\"\r\n",
            "x-amz-missing-meta: 2\r\n",
            "Connection: close\r\n",
            "\r\n",
            "hello"
        );
        let _ = sock.write_all(resp.as_bytes()).await;
    });
    let mm_dst_listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let mm_dst_addr = mm_dst_listener.local_addr().expect("addr");
    let mm_dst_attempts = Arc::new(AtomicUsize::new(0));
    let mm_dst_attempts_clone = mm_dst_attempts.clone();
    let mm_dst_stop = Arc::new(tokio::sync::Notify::new());
    let mm_dst_stop_clone = mm_dst_stop.clone();
    let mm_dst_server = tokio::spawn(async move {
        loop {
            tokio::select! {
                biased;
                _ = mm_dst_stop_clone.notified() => break,
                accepted = mm_dst_listener.accept() => {
                    if accepted.is_ok() {
                        mm_dst_attempts_clone.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }
        }
    });
    let mm_src = build_s3_handle(
        format!("http://{mm_src_addr}"),
        "src-bucket",
        None,
        "src-ak",
    )
    .await;
    let mm_dst = build_s3_handle(
        format!("http://{mm_dst_addr}"),
        "dst-bucket",
        None,
        "dst-ak",
    )
    .await;
    let mm_fail = relay_copy(
        &mm_src,
        &mm_dst,
        RelayRequest::new(
            ReadRequest::Current("missing-meta.txt".to_string()),
            "out.txt".to_string(),
            PutPrecondition::None,
            MetadataPolicy::ContentTypeAndUserMetadata,
            budget,
        ),
        Cancellation::new(),
    )
    .await
    .expect_err("x-amz-missing-meta must refuse user-metadata preservation before dispatch");
    mm_src_server.await.expect("mm src server done");
    mm_dst_stop.notify_one();
    mm_dst_server.await.expect("mm dst server done");
    assert_eq!(mm_fail.kind(), TransferFailureKind::Admission);
    assert_eq!(mm_fail.commit(), CommitKnowledge::NotStarted);
    assert_eq!(mm_fail.source_bytes_consumed(), 0);
    assert_eq!(mm_fail.destination_bytes_handed_off(), 0);
    assert_eq!(mm_dst_attempts.load(Ordering::SeqCst), 0);

    // 2. Truncated S3 GET stream (Content-Length: 8, sends 5 bytes then closes):
    // ValidatingReader withholds final fragment so destination PUT never receives full 8 bytes.
    let short_src_listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let short_src_addr = short_src_listener.local_addr().expect("addr");
    let short_src_server = tokio::spawn(async move {
        let (mut sock, _) = short_src_listener.accept().await.expect("accepts");
        let mut buf = [0_u8; 2048];
        let _ = sock.read(&mut buf).await;
        let resp = concat!(
            "HTTP/1.1 200 OK\r\n",
            "Content-Length: 8\r\n",
            "ETag: \"short-etag\"\r\n",
            "Connection: close\r\n",
            "\r\n",
            "12345"
        );
        let _ = sock.write_all(resp.as_bytes()).await;
        drop(sock);
    });

    let short_dst_listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let short_dst_addr = short_dst_listener.local_addr().expect("addr");
    let short_dst_wire = tokio::spawn(async move {
        let Ok(Ok((mut sock, _))) =
            tokio::time::timeout(Duration::from_millis(300), short_dst_listener.accept()).await
        else {
            return Vec::new();
        };
        let mut received = Vec::new();
        let mut buf = [0_u8; 4096];
        while let Ok(Ok(n)) =
            tokio::time::timeout(Duration::from_millis(150), sock.read(&mut buf)).await
        {
            if n == 0 {
                break;
            }
            received.extend_from_slice(&buf[..n]);
        }
        received
    });

    let short_src = build_s3_handle(
        format!("http://{short_src_addr}"),
        "src-bucket",
        None,
        "src-ak",
    )
    .await;
    let short_dst = build_s3_handle(
        format!("http://{short_dst_addr}"),
        "dst-bucket",
        None,
        "dst-ak",
    )
    .await;

    let short_fail = relay_copy(
        &short_src,
        &short_dst,
        RelayRequest::new(
            ReadRequest::Current("truncated.bin".to_string()),
            "out.bin".to_string(),
            PutPrecondition::None,
            MetadataPolicy::None,
            TransferBudget::new(64, 3, Duration::from_secs(5)).unwrap(),
        ),
        Cancellation::new(),
    )
    .await
    .expect_err("truncated source must fail relay");

    short_src_server.await.expect("short src server done");
    let wire = short_dst_wire.await.expect("short dst capture done");
    assert_eq!(short_fail.kind(), TransferFailureKind::SourceBody);
    assert!(short_fail.destination_bytes_handed_off() < 8);
    if let Some(pos) = wire.windows(4).position(|w| w == b"\r\n\r\n") {
        assert!(!wire[pos + 4..].windows(3).any(|w| w == b"345"));
    }
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn relay_copy_bridges_local_and_scripted_s3_in_both_directions() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root_dir = std::env::temp_dir().join(format!(
        "storageprims-ops-s3-local-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root_dir).expect("temp root creates");

    let local = LocalProvider::new(&root_dir).expect("local provider opens");
    local
        .put(
            "outbound/report.txt",
            Box::new(Cursor::new(b"local-to-s3-payload".to_vec())),
            PutOptions {
                content_length: Some(19),
                ..PutOptions::default()
            },
        )
        .await
        .expect("seed local object");

    // Local -> S3
    let put_listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let put_addr = put_listener.local_addr().expect("addr");
    let put_server = tokio::spawn(async move {
        let (mut sock, _) = put_listener.accept().await.expect("accepts");
        let mut received = Vec::new();
        let mut buf = [0_u8; 4096];
        loop {
            let n = sock.read(&mut buf).await.expect("reads");
            if n == 0 {
                break;
            }
            received.extend_from_slice(&buf[..n]);
            if let Some(pos) = received.windows(4).position(|w| w == b"\r\n\r\n") {
                if received[pos + 4..].len() >= 19 {
                    break;
                }
            }
        }
        let resp = concat!(
            "HTTP/1.1 200 OK\r\n",
            "ETag: \"uploaded-etag\"\r\n",
            "Content-Length: 0\r\n",
            "Connection: close\r\n",
            "\r\n"
        );
        sock.write_all(resp.as_bytes()).await.expect("writes");
        received
    });

    let s3_dst = build_s3_handle(
        format!("http://{put_addr}"),
        "archive-bucket",
        Some("prefix-a"),
        "upload-ak",
    )
    .await;
    let receipt = relay_copy(
        &local,
        &s3_dst,
        RelayRequest::new(
            ReadRequest::Current("outbound/report.txt".to_string()),
            "ingested/report.txt".to_string(),
            PutPrecondition::MustNotExist,
            MetadataPolicy::None,
            TransferBudget::new(1024, 8, Duration::from_secs(5)).unwrap(),
        ),
        Cancellation::new(),
    )
    .await
    .expect("local to S3 relay succeeds");
    assert_eq!(receipt.enforcement(), ReadEnforcement::Unguarded);
    assert_eq!(receipt.source_bytes_consumed(), 19);
    assert_eq!(receipt.destination_bytes_handed_off(), 19);
    assert_eq!(
        receipt.destination().etag.as_deref(),
        Some("\"uploaded-etag\"")
    );

    let put_wire = put_server.await.expect("put server done");
    let header_end = put_wire
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("HTTP header delimiter");
    let headers_lower = String::from_utf8_lossy(&put_wire[..header_end]).to_ascii_lowercase();
    assert!(headers_lower.starts_with("put /archive-bucket/prefix-a/ingested/report.txt"));
    assert!(headers_lower.contains("if-none-match: *"));
    assert!(put_wire[header_end + 4..]
        .windows(b"local-to-s3-payload".len())
        .any(|w| w == b"local-to-s3-payload"));

    // S3 -> Local: MetadataPolicy::ContentTypeOnly is refused before opening S3 socket!
    let dead_s3 = build_s3_handle(
        "http://127.0.0.1:9".to_string(),
        "archive-bucket",
        None,
        "download-ak",
    )
    .await;
    let meta_refused = relay_copy(
        &dead_s3,
        &local,
        RelayRequest::new(
            ReadRequest::Current("remote.txt".to_string()),
            "inbound/refused.txt".to_string(),
            PutPrecondition::MustNotExist,
            MetadataPolicy::ContentTypeOnly,
            TransferBudget::new(1024, 64, Duration::from_secs(5)).unwrap(),
        ),
        Cancellation::new(),
    )
    .await
    .expect_err("local destination must refuse ContentTypeOnly before opening S3 source");
    assert_eq!(meta_refused.kind(), TransferFailureKind::Admission);
    assert_eq!(meta_refused.commit(), CommitKnowledge::NotStarted);
    assert_eq!(
        meta_refused.cause().map(|e| e.kind()),
        Some(ExtensionErrorKind::UnsupportedMetadata)
    );

    // S3 -> Local with MetadataPolicy::None succeeds and stages atomically.
    let get_listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let get_addr = get_listener.local_addr().expect("addr");
    let get_server = tokio::spawn(async move {
        let (mut sock, _) = get_listener.accept().await.expect("accepts");
        let mut buf = [0_u8; 2048];
        let _ = sock.read(&mut buf).await;
        let resp = concat!(
            "HTTP/1.1 200 OK\r\n",
            "Content-Length: 19\r\n",
            "Content-Type: application/octet-stream\r\n",
            "ETag: \"s3-src-etag\"\r\n",
            "Connection: close\r\n",
            "\r\n",
            "s3-to-local-payload"
        );
        sock.write_all(resp.as_bytes()).await.expect("writes");
    });

    let s3_src = build_s3_handle(
        format!("http://{get_addr}"),
        "archive-bucket",
        Some("prefix-b"),
        "download-ak",
    )
    .await;
    let inbound_receipt = relay_copy(
        &s3_src,
        &local,
        RelayRequest::new(
            ReadRequest::Current("remote/item.bin".to_string()),
            "inbound/item.bin".to_string(),
            PutPrecondition::MustNotExist,
            MetadataPolicy::None,
            TransferBudget::new(1024, 5, Duration::from_secs(5)).unwrap(),
        ),
        Cancellation::new(),
    )
    .await
    .expect("S3 to local relay succeeds");
    get_server.await.expect("get server done");
    assert_eq!(
        inbound_receipt.source().validator.as_deref(),
        Some("\"s3-src-etag\"")
    );
    assert_eq!(inbound_receipt.destination_bytes_handed_off(), 19);

    let mut local_stream = local
        .get("inbound/item.bin")
        .await
        .expect("local object readable");
    let mut local_bytes = Vec::new();
    local_stream
        .read_to_end(&mut local_bytes)
        .await
        .expect("reads local bytes");
    assert_eq!(local_bytes, b"s3-to-local-payload");

    let _ = std::fs::remove_dir_all(&root_dir);
}
