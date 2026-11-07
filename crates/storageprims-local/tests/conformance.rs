//! Integration and conformance suite for `LocalProvider`.

use std::{
    collections::BTreeMap,
    io::Cursor,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    task::{Context, Poll},
    time::{Duration, Instant},
};
use storageprims_core::{
    require_delimiter_listing, AdmittedPutProvider, BoxedByteStream, Cancellation, Capability,
    CommitKnowledge, ConflictKind, CopyRequest, CopyStrategy, CredentialSourceKind,
    DelimiterListRequest, Evidence, ExtensionErrorKind, GetRangeRequest, ListOptions,
    OperationControl, ProbeScope, ProviderKind, PutOptions, PutPrecondition, ReadEnforcement,
    ReadRequest, ReadResponseProvider, StorageError, StorageOperation, StorageProvider, WriteMode,
};
use storageprims_local::LocalProvider;
use tokio::io::{AsyncRead, AsyncReadExt, ReadBuf};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct TempFixture(PathBuf);

impl TempFixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "storageprims-local-it-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn body_bytes(bytes: impl Into<Vec<u8>>) -> BoxedByteStream {
    Box::new(Cursor::new(bytes.into()))
}

async fn collect_stream(mut stream: BoxedByteStream) -> Vec<u8> {
    let mut out = Vec::new();
    stream.read_to_end(&mut out).await.unwrap();
    out
}

fn assert_error_chain_redacts_root(err: &(dyn std::error::Error + 'static), root: &Path) {
    let needle = root.to_string_lossy();
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(cause) = current {
        let formatted = format!("{cause:?} {cause}");
        assert!(
            !formatted.contains(needle.as_ref()),
            "root path leaked in error chain: {formatted}"
        );
        current = cause.source();
    }
}

struct FailingStream {
    prefix: Vec<u8>,
    emitted: bool,
}

impl AsyncRead for FailingStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if !self.emitted {
            self.emitted = true;
            buf.put_slice(&self.prefix);
            Poll::Ready(Ok(()))
        } else {
            Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "synthetic mid-stream read failure",
            )))
        }
    }
}

#[tokio::test]
async fn capabilities_probe_and_basic_crud_conformance() {
    let fx = TempFixture::new();
    let provider = LocalProvider::new(fx.path()).unwrap();

    assert_eq!(provider.provider_kind(), ProviderKind::Local);
    assert_eq!(
        provider.capabilities(),
        vec![
            Capability::DelimiterListing,
            Capability::CredentialProbe,
            Capability::ConditionalPut,
        ]
    );
    assert!(provider.guarded_reads().is_none());
    assert!(provider.read_responses().is_some());
    assert!(provider.admitted_puts().is_some());

    let delim = require_delimiter_listing(&provider).unwrap();
    assert!(delim.supports_delimiter("/"));
    assert!(!delim.supports_delimiter("-"));
    assert!(!delim.supports_delimiter(""));

    let probe = provider.probe().await.unwrap();
    assert_eq!(probe.provider, ProviderKind::Local);
    assert_eq!(probe.endpoint, None);
    assert_eq!(probe.credential_source, CredentialSourceKind::None);
    assert_eq!(probe.scope, ProbeScope::ConfiguredContainer);

    // Put nested object, head, get, get_range, copy, delete.
    let payload = b"0123456789abcdef".to_vec();
    let put_res = provider
        .put(
            "nested/dir/object.bin",
            body_bytes(payload.clone()),
            PutOptions {
                content_length: Some(payload.len() as u64),
                ..PutOptions::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(put_res.path, "nested/dir/object.bin");
    assert_eq!(put_res.size, Some(16));
    assert_eq!(put_res.etag, None);

    let meta = provider.head("nested/dir/object.bin").await.unwrap();
    assert_eq!(meta.path, "nested/dir/object.bin");
    assert_eq!(meta.size, 16);
    assert!(meta.last_modified.as_deref().unwrap().ends_with('Z'));

    let got = collect_stream(provider.get("nested/dir/object.bin").await.unwrap()).await;
    assert_eq!(got, payload);

    // Exact range and clamped-past-EOF range.
    let slice_mid = collect_stream(
        provider
            .get_range(GetRangeRequest {
                key: "nested/dir/object.bin".into(),
                offset: 4,
                length: 6,
            })
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(slice_mid, b"456789");

    let slice_clamped = collect_stream(
        provider
            .get_range(GetRangeRequest {
                key: "nested/dir/object.bin".into(),
                offset: 12,
                length: 100,
            })
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(slice_clamped, b"cdef");

    // Range boundary rejections.
    assert!(matches!(
        provider
            .get_range(GetRangeRequest {
                key: "nested/dir/object.bin".into(),
                offset: 0,
                length: 0,
            })
            .await,
        Err(StorageError::InvalidArgument {
            argument,
            ..
        }) if argument == "length"
    ));
    assert!(matches!(
        provider
            .get_range(GetRangeRequest {
                key: "nested/dir/object.bin".into(),
                offset: 16,
                length: 1,
            })
            .await,
        Err(StorageError::InvalidArgument {
            argument,
            ..
        }) if argument == "offset"
    ));
    assert!(matches!(
        provider
            .get_range(GetRangeRequest {
                key: "nested/dir/object.bin".into(),
                offset: u64::MAX,
                length: 2,
            })
            .await,
        Err(StorageError::InvalidArgument {
            argument,
            ..
        }) if argument == "length"
    ));

    // In-root copy and self-copy.
    let copied = provider
        .copy(CopyRequest {
            source: "nested/dir/object.bin".into(),
            destination: "copied/out.bin".into(),
        })
        .await
        .unwrap();
    assert_eq!(copied.strategy, CopyStrategy::Native);
    assert_eq!(copied.bytes_copied, Some(16));
    assert_eq!(
        collect_stream(provider.get("copied/out.bin").await.unwrap()).await,
        payload
    );

    let self_copied = provider
        .copy(CopyRequest {
            source: "copied/out.bin".into(),
            destination: "copied/out.bin".into(),
        })
        .await
        .unwrap();
    assert_eq!(self_copied.bytes_copied, Some(16));
    assert_eq!(
        collect_stream(provider.get("copied/out.bin").await.unwrap()).await,
        payload
    );

    // Delete is idempotent for absent keys and absent ancestor directories.
    provider.delete("copied/out.bin").await.unwrap();
    provider.delete("copied/out.bin").await.unwrap();
    provider
        .delete("never-existed-dir/never-existed.bin")
        .await
        .unwrap();
    let missing_err = provider.head("copied/out.bin").await.unwrap_err();
    assert!(matches!(missing_err, StorageError::NotFound { .. }));
    assert_error_chain_redacts_root(&missing_err, fx.path());
}

#[tokio::test]
async fn key_length_ceiling_refuses_before_creating_parent_or_temp_and_cleans_up_failed_writes() {
    let fx = TempFixture::new();
    let provider = LocalProvider::new(fx.path()).unwrap();

    // A 1025-byte key whose individual segments are all <= 200 bytes must still fail
    // before creating any parent directory or temporary file.
    let prefix_dir = "uncreated-parent";
    let mid = format!(
        "{}/{}/{}/{}",
        "m".repeat(200),
        "n".repeat(200),
        "p".repeat(200),
        "q".repeat(200)
    );
    let rem = 1025 - prefix_dir.len() - 1 - mid.len() - 1;
    let oversize_key = format!("{prefix_dir}/{mid}/{}", "x".repeat(rem));
    assert_eq!(oversize_key.len(), 1025);

    let err = provider
        .put(
            &oversize_key,
            body_bytes(b"abc".to_vec()),
            PutOptions::default(),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        StorageError::InvalidArgument { ref argument, .. } if argument == "key"
    ));
    assert!(!fx.path().join(prefix_dir).exists());

    // Exact 1024-byte key with <= 200-byte segments succeeds, and overlong single segment (>255) refuses without truncating.
    let exact_rem = 1024 - "ok-parent/".len() - mid.len() - 1;
    let exact_key = format!("ok-parent/{mid}/{}", "y".repeat(exact_rem));
    assert_eq!(exact_key.len(), 1024);
    provider
        .put(
            &exact_key,
            body_bytes(b"ok".to_vec()),
            PutOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        collect_stream(provider.get(&exact_key).await.unwrap()).await,
        b"ok"
    );

    let overlong_segment_key = format!("seg-parent/{}", "z".repeat(300));
    assert!(provider
        .put(
            &overlong_segment_key,
            body_bytes(b"no".to_vec()),
            PutOptions::default(),
        )
        .await
        .is_err());

    // Declared length mismatch and mid-stream error clean up staging entries and leave target absent.
    let short_err = provider
        .put(
            "mismatch.bin",
            body_bytes(b"short".to_vec()),
            PutOptions {
                content_length: Some(10),
                ..PutOptions::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(short_err, StorageError::InvalidArgument { .. }));
    assert!(!fx.path().join("mismatch.bin").exists());

    let stream_err = provider
        .put(
            "broken.bin",
            Box::new(FailingStream {
                prefix: b"part".to_vec(),
                emitted: false,
            }),
            PutOptions::default(),
        )
        .await
        .unwrap_err();
    assert!(matches!(stream_err, StorageError::Io { .. }));
    assert_error_chain_redacts_root(&stream_err, fx.path());
    assert!(!fx.path().join("broken.bin").exists());

    for entry in std::fs::read_dir(fx.path()).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().to_string();
        assert!(
            !name.starts_with(".storageprims-tmp"),
            "unexpected leftover staging file: {name}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_create_only_enforces_single_winner_without_temp_residue() {
    let fx = TempFixture::new();
    let provider = Arc::new(LocalProvider::new(fx.path()).unwrap());

    let contenders = 16usize;
    let barrier = Arc::new(tokio::sync::Barrier::new(contenders));
    let mut handles = Vec::with_capacity(contenders);

    for idx in 0..contenders {
        let p = Arc::clone(&provider);
        let b = Arc::clone(&barrier);
        handles.push(tokio::spawn(async move {
            let payload = format!("contender-{idx:02}").into_bytes();
            let len = payload.len() as u64;
            b.wait().await;
            p.put(
                "race/winner.txt",
                body_bytes(payload.clone()),
                PutOptions {
                    content_length: Some(len),
                    precondition: PutPrecondition::MustNotExist,
                    ..PutOptions::default()
                },
            )
            .await
            .map(|_| payload)
        }));
    }

    let mut winners = Vec::new();
    let mut conflicts = 0usize;
    for handle in handles {
        match handle.await.unwrap() {
            Ok(payload) => winners.push(payload),
            Err(StorageError::Conflict {
                provider: ProviderKind::Local,
                operation: StorageOperation::Put,
                kind: ConflictKind::AlreadyExists,
                ..
            }) => conflicts += 1,
            Err(other) => panic!("unexpected error in create-only race: {other:?}"),
        }
    }

    assert_eq!(winners.len(), 1);
    assert_eq!(conflicts, contenders - 1);
    let stored = collect_stream(provider.get("race/winner.txt").await.unwrap()).await;
    assert_eq!(stored, winners[0]);

    for entry in std::fs::read_dir(fx.path().join("race")).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().to_string();
        assert_eq!(name, "winner.txt");
    }
}

#[tokio::test]
async fn read_response_and_admitted_put_contracts() {
    let fx = TempFixture::new();
    let provider = LocalProvider::new(fx.path()).unwrap();

    // Admission performs semantic validation only with zero filesystem I/O.
    let support = provider.put_support();
    assert_eq!(support.mode(), WriteMode::SinglePart);
    assert!(support.unconditional());
    assert!(support.create_only());
    assert!(!support.validator_match());
    assert!(!support.content_type());
    assert!(!support.user_metadata());
    assert_eq!(support.max_content_length(), u64::MAX);

    assert_eq!(
        provider
            .admit_put(
                "meta.bin",
                PutOptions {
                    content_length: Some(3),
                    content_type: Some("text/plain".into()),
                    ..PutOptions::default()
                },
                WriteMode::SinglePart,
            )
            .err()
            .unwrap()
            .kind(),
        ExtensionErrorKind::UnsupportedMetadata
    );
    let mut user_meta = BTreeMap::new();
    user_meta.insert("k".into(), "v".into());
    assert_eq!(
        provider
            .admit_put(
                "meta.bin",
                PutOptions {
                    content_length: Some(3),
                    metadata: user_meta,
                    ..PutOptions::default()
                },
                WriteMode::SinglePart,
            )
            .err()
            .unwrap()
            .kind(),
        ExtensionErrorKind::UnsupportedMetadata
    );
    assert_eq!(
        provider
            .admit_put(
                "match.bin",
                PutOptions {
                    content_length: Some(3),
                    precondition: PutPrecondition::Match {
                        token: "\"etag\"".into(),
                    },
                    ..PutOptions::default()
                },
                WriteMode::SinglePart,
            )
            .err()
            .unwrap()
            .kind(),
        ExtensionErrorKind::UnsupportedPredicate
    );
    assert_eq!(
        provider
            .admit_put("len.bin", PutOptions::default(), WriteMode::SinglePart)
            .err()
            .unwrap()
            .kind(),
        ExtensionErrorKind::LengthRequired
    );

    // Verify admit_put does NOT create directories or temp files until execute() runs.
    let prepared = provider
        .admit_put(
            "deferred-dir/item.bin",
            PutOptions {
                content_length: Some(7),
                precondition: PutPrecondition::MustNotExist,
                ..PutOptions::default()
            },
            WriteMode::SinglePart,
        )
        .unwrap();
    assert!(!fx.path().join("deferred-dir").exists());

    // Pre-cancelled control yields CommitKnowledge::NotStarted without creating deferred-dir.
    let pre_cancelled = Cancellation::new();
    pre_cancelled.cancel();
    let prepared_cancel = provider
        .admit_put(
            "never-started/item.bin",
            PutOptions {
                content_length: Some(7),
                ..PutOptions::default()
            },
            WriteMode::SinglePart,
        )
        .unwrap();
    let not_started = prepared_cancel
        .execute(
            body_bytes(b"payload".to_vec()),
            OperationControl::new(Instant::now() + Duration::from_secs(5), pre_cancelled),
        )
        .await
        .unwrap_err();
    assert_eq!(not_started.commit(), CommitKnowledge::NotStarted);
    assert_eq!(not_started.error().kind(), ExtensionErrorKind::Cancelled);
    assert!(!fx.path().join("never-started").exists());

    // Execute admitted put and verify ReadResponseProvider metadata and receipt invariants.
    let control =
        OperationControl::new(Instant::now() + Duration::from_secs(5), Cancellation::new());
    let put_res = prepared
        .execute(body_bytes(b"payload".to_vec()), control.clone())
        .await
        .unwrap();
    assert_eq!(put_res.path, "deferred-dir/item.bin");
    assert_eq!(put_res.size, Some(7));

    let read_sup = provider.read_support();
    assert!(read_sup.current());
    assert!(!read_sup.native_version());
    assert!(!read_sup.validator_match());

    let response = provider
        .read_response(
            ReadRequest::Current("deferred-dir/item.bin".into()),
            control,
        )
        .await
        .unwrap();
    assert_eq!(response.enforcement(), ReadEnforcement::Unguarded);
    assert_eq!(response.receipt().path, "deferred-dir/item.bin");
    assert_eq!(response.receipt().total_size, Some(7));
    assert_eq!(response.receipt().native_version, None);
    assert_eq!(response.receipt().validator, None);
    assert!(matches!(
        response.metadata().content_type(),
        Evidence::Available(None)
    ));
    assert!(matches!(
        response.metadata().user_metadata(),
        Evidence::Available(map) if map.is_empty()
    ));
    let (reader, _, _, _) = response.into_parts();
    assert_eq!(collect_stream(reader).await, b"payload");
}

#[tokio::test]
async fn listing_directory_order_reserved_exclusion_deep_hierarchy_and_delimiter() {
    let fx = TempFixture::new();
    std::fs::create_dir_all(fx.path().join("a")).unwrap();
    std::fs::create_dir_all(fx.path().join("empty")).unwrap();
    std::fs::write(fx.path().join("a-b"), "1").unwrap();
    std::fs::write(fx.path().join("a.txt"), "22").unwrap();
    std::fs::write(fx.path().join("a/b"), "333").unwrap();
    std::fs::write(fx.path().join("a/z"), "4444").unwrap();
    std::fs::write(fx.path().join("a0"), "55555").unwrap();

    // Plant stale and case/Unicode reserved temporary entries: must be invisible to list & delimiter list!
    std::fs::write(fx.path().join(".storageprims-tmp-stale"), "crash-residue").unwrap();
    std::fs::write(fx.path().join(".STORAGEPRIMS-TMP-UPPER"), "upper-residue").unwrap();
    std::fs::write(
        fx.path().join(".\u{017f}torageprims-tmp-longs"),
        "unicode-residue",
    )
    .unwrap();
    std::fs::create_dir_all(fx.path().join(".storageprims-tmp-dir")).unwrap();
    std::fs::write(fx.path().join(".storageprims-tmp-dir/inside"), "hidden").unwrap();

    let provider = LocalProvider::new(fx.path()).unwrap();

    // Flat list orders directories at `name/`, omits empty directories, and excludes reserved entries.
    let all = provider.list(ListOptions::default()).await.unwrap();
    let keys: Vec<String> = all.objects.iter().map(|o| o.path.clone()).collect();
    assert_eq!(keys, vec!["a-b", "a.txt", "a/b", "a/z", "a0"]);
    assert!(!all.is_truncated);
    assert!(all.continuation_token.is_none());

    // Page 1-by-1 across the entire flat list.
    let mut paged_keys = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let page = provider
            .list(ListOptions {
                prefix: None,
                continuation_token: token.clone(),
                max_keys: Some(1),
            })
            .await
            .unwrap();
        if let Some(ref t) = page.continuation_token {
            assert!(!t.contains(fx.path().to_string_lossy().as_ref()));
        }
        for obj in page.objects {
            paged_keys.push(obj.path);
        }
        if !page.is_truncated {
            assert!(page.continuation_token.is_none());
            break;
        }
        token = page.continuation_token;
        assert!(token.is_some());
    }
    assert_eq!(paged_keys, keys);

    // Delimiter listing ("/") includes native empty directories as explicit common_prefixes
    // and shares max_keys across objects and common_prefixes in UTF-8 byte order!
    let delim = require_delimiter_listing(&provider).unwrap();
    let full_delim = delim
        .list_delimited(DelimiterListRequest {
            prefix: None,
            delimiter: "/".into(),
            continuation_token: None,
            max_keys: None,
        })
        .await
        .unwrap();
    let delim_objs: Vec<String> = full_delim.objects.iter().map(|o| o.path.clone()).collect();
    assert_eq!(delim_objs, vec!["a-b", "a.txt", "a0"]);
    assert_eq!(full_delim.common_prefixes, vec!["a/", "empty/"]);

    // Page delimiter listing with max_keys = 2:
    // Combined sorted order is ["a-b", "a.txt", "a/", "a0", "empty/"].
    let p1 = delim
        .list_delimited(DelimiterListRequest {
            prefix: None,
            delimiter: "/".into(),
            continuation_token: None,
            max_keys: Some(2),
        })
        .await
        .unwrap();
    assert_eq!(
        p1.objects
            .iter()
            .map(|o| o.path.as_str())
            .collect::<Vec<_>>(),
        vec!["a-b", "a.txt"]
    );
    assert!(p1.common_prefixes.is_empty());
    assert!(p1.is_truncated);

    let p2 = delim
        .list_delimited(DelimiterListRequest {
            prefix: None,
            delimiter: "/".into(),
            continuation_token: p1.continuation_token.clone(),
            max_keys: Some(2),
        })
        .await
        .unwrap();
    assert_eq!(
        p2.objects
            .iter()
            .map(|o| o.path.as_str())
            .collect::<Vec<_>>(),
        vec!["a0"]
    );
    assert_eq!(p2.common_prefixes, vec!["a/"]);
    assert!(p2.is_truncated);

    let p3 = delim
        .list_delimited(DelimiterListRequest {
            prefix: None,
            delimiter: "/".into(),
            continuation_token: p2.continuation_token.clone(),
            max_keys: Some(2),
        })
        .await
        .unwrap();
    assert!(p3.objects.is_empty());
    assert_eq!(p3.common_prefixes, vec!["empty/"]);
    assert!(!p3.is_truncated);

    // Non-'/' delimiter fails as InvalidArgument("delimiter") before walking.
    assert!(matches!(
        delim
            .list_delimited(DelimiterListRequest {
                prefix: None,
                delimiter: "-".into(),
                continuation_token: None,
                max_keys: None,
            })
            .await,
        Err(StorageError::InvalidArgument {
            ref argument,
            ..
        }) if argument == "delimiter"
    ));

    // max_keys > 1000 fails as InvalidArgument("max_keys").
    assert!(matches!(
        provider
            .list(ListOptions {
                max_keys: Some(1001),
                ..ListOptions::default()
            })
            .await,
        Err(StorageError::InvalidArgument {
            ref argument,
            ..
        }) if argument == "max_keys"
    ));

    // Cross-root and cross-scope token rejection.
    let other_fx = TempFixture::new();
    let other_provider = LocalProvider::new(other_fx.path()).unwrap();
    assert!(matches!(
        other_provider
            .list(ListOptions {
                continuation_token: p1.continuation_token.clone(),
                max_keys: Some(2),
                ..ListOptions::default()
            })
            .await,
        Err(StorageError::InvalidArgument {
            ref argument,
            ..
        }) if argument == "continuation_token"
    ));

    // Deep valid hierarchy (200 nested segments = 400 bytes) succeeds without hitting EMFILE.
    let deep_fx = TempFixture::new();
    let deep_provider = LocalProvider::new(deep_fx.path()).unwrap();
    let deep_key = format!("{}/leaf", vec!["d"; 199].join("/"));
    assert!(deep_key.len() < 1024);
    deep_provider
        .put(
            &deep_key,
            body_bytes(b"deep".to_vec()),
            PutOptions::default(),
        )
        .await
        .unwrap();
    let deep_list = deep_provider.list(ListOptions::default()).await.unwrap();
    assert_eq!(deep_list.objects.len(), 1);
    assert_eq!(deep_list.objects[0].path, deep_key);
}

#[tokio::test]
async fn deterministic_random_trees_match_sorted_utf8_oracle_across_page_sizes() {
    struct XorShift64(u64);
    impl XorShift64 {
        fn next_u64(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }
        fn below(&mut self, bound: usize) -> usize {
            (self.next_u64() as usize) % bound
        }
    }

    fn populate_tree(
        rng: &mut XorShift64,
        dir: &Path,
        rel: &str,
        depth: usize,
        expected: &mut Vec<String>,
    ) {
        let mut names = ["a", "a-b", "a.txt", "a0", "b", "z", "é"];
        for i in (1..names.len()).rev() {
            let j = rng.below(i + 1);
            names.swap(i, j);
        }
        let count = rng.below(names.len() + 1);
        for &name in &names[..count] {
            let child_path = dir.join(name);
            let child_key = if rel.is_empty() {
                name.to_string()
            } else {
                format!("{rel}/{name}")
            };
            if depth > 0 && rng.below(10) < 4 {
                std::fs::create_dir(&child_path).unwrap();
                populate_tree(rng, &child_path, &child_key, depth - 1, expected);
            } else {
                std::fs::write(&child_path, b"x").unwrap();
                expected.push(child_key);
            }
        }
    }

    let mut rng = XorShift64(0x3020_2026_1107_cafe);
    for _case in 0..40 {
        let fx = TempFixture::new();
        let mut expected_all = Vec::new();
        populate_tree(&mut rng, fx.path(), "", 3, &mut expected_all);
        expected_all.sort();

        let provider = LocalProvider::new(fx.path()).unwrap();
        for prefix in ["", "a", "a/", "a/b", "missing", "z/"] {
            let expected: Vec<String> = expected_all
                .iter()
                .filter(|k| k.starts_with(prefix))
                .cloned()
                .collect();
            for k in [1_u32, 2, 3, 1000] {
                let mut actual = Vec::new();
                let mut token: Option<String> = None;
                let opt_prefix = if prefix.is_empty() {
                    None
                } else {
                    Some(prefix.to_string())
                };
                loop {
                    let res = provider
                        .list(ListOptions {
                            prefix: opt_prefix.clone(),
                            continuation_token: token.clone(),
                            max_keys: Some(k),
                        })
                        .await
                        .unwrap();
                    actual.extend(res.objects.into_iter().map(|o| o.path));
                    if !res.is_truncated {
                        break;
                    }
                    token = res.continuation_token;
                    assert!(token.is_some());
                }
                assert_eq!(actual, expected, "mismatch for prefix={prefix:?}, k={k}");
            }
        }
    }
}

#[tokio::test]
async fn case_mismatched_list_prefixes_never_fabricate_keys() {
    let fx = TempFixture::new();
    std::fs::create_dir_all(fx.path().join("Reports")).unwrap();
    std::fs::write(fx.path().join("Reports/q1.txt"), "q1").unwrap();
    let provider = LocalProvider::new(fx.path()).unwrap();

    // Whether the volume is case-sensitive or case-insensitive (APFS / NTFS), literal prefix
    // "reports/" must never fabricate or return "Reports/q1.txt".
    let mismatched = provider
        .list(ListOptions {
            prefix: Some("reports/".into()),
            ..ListOptions::default()
        })
        .await
        .unwrap();
    assert!(mismatched.objects.is_empty());

    let delim = require_delimiter_listing(&provider).unwrap();
    let mismatched_delim = delim
        .list_delimited(DelimiterListRequest {
            prefix: Some("reports/".into()),
            delimiter: "/".into(),
            continuation_token: None,
            max_keys: None,
        })
        .await
        .unwrap();
    assert!(mismatched_delim.objects.is_empty());
    assert!(mismatched_delim.common_prefixes.is_empty());

    // Exact-case prefix "Reports/" returns "Reports/q1.txt".
    let exact = provider
        .list(ListOptions {
            prefix: Some("Reports/".into()),
            ..ListOptions::default()
        })
        .await
        .unwrap();
    assert_eq!(exact.objects.len(), 1);
    assert_eq!(exact.objects[0].path, "Reports/q1.txt");
}

#[cfg(unix)]
#[tokio::test]
async fn unix_symlinks_are_refused_across_list_delete_and_put() {
    use std::os::unix::fs::symlink;

    let fx = TempFixture::new();
    std::fs::create_dir_all(fx.path().join("Reports")).unwrap();
    std::fs::write(fx.path().join("Reports/q1.txt"), "q1").unwrap();
    let provider = LocalProvider::new(fx.path()).unwrap();

    // Planting a symlink inside a scanned directory fails listing closed with StorageError::Other.
    symlink("Reports/q1.txt", fx.path().join("planted-link")).unwrap();
    let scan_err = provider.list(ListOptions::default()).await.unwrap_err();
    assert!(matches!(
        scan_err,
        StorageError::Other {
            provider: Some(ProviderKind::Local),
            operation: Some(StorageOperation::List),
            ..
        }
    ));
    assert_error_chain_redacts_root(&scan_err, fx.path());

    // Delete and overwrite put also refuse symlink leaves without following or unlinking them.
    let del_err = provider.delete("planted-link").await.unwrap_err();
    assert!(matches!(del_err, StorageError::Io { .. }));
    assert_error_chain_redacts_root(&del_err, fx.path());
    assert!(fx.path().join("planted-link").symlink_metadata().is_ok());

    let put_err = provider
        .put(
            "planted-link",
            body_bytes(b"overwrite".to_vec()),
            PutOptions::default(),
        )
        .await
        .unwrap_err();
    assert!(matches!(put_err, StorageError::Io { .. }));
    assert_eq!(
        std::fs::read_to_string(fx.path().join("Reports/q1.txt")).unwrap(),
        "q1"
    );
}
