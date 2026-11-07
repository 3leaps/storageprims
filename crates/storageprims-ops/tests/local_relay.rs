//! Two-handle bounded relay integration tests over real `LocalProvider` instances.

use std::{
    io::Cursor,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use storageprims_core::{
    Cancellation, CommitKnowledge, ConflictKind, CopyStrategy, ExtensionErrorKind, PutOptions,
    PutPrecondition, ReadEnforcement, ReadRequest, StorageError, StorageProvider,
};
use storageprims_local::LocalProvider;
use storageprims_ops::{
    relay_copy, MetadataPolicy, RelayRequest, TransferBudget, TransferFailureKind,
};
use tokio::io::AsyncReadExt;

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "storageprims-ops-local-relay-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn relay_copy_between_independent_local_roots_handles_multi_chunk_zero_byte_and_conflicts() {
    let src_dir = TempDir::new();
    let dst_dir = TempDir::new();
    let source = LocalProvider::new(src_dir.path()).unwrap();
    let destination = LocalProvider::new(dst_dir.path()).unwrap();

    // Multi-chunk 200 KiB payload relayed with 8 KiB bounded buffer.
    let payload: Vec<u8> = (0..204_800).map(|idx| (idx % 251) as u8).collect();
    source
        .put(
            "artifacts/bundle.bin",
            Box::new(Cursor::new(payload.clone())),
            PutOptions {
                content_length: Some(payload.len() as u64),
                ..PutOptions::default()
            },
        )
        .await
        .unwrap();

    let budget = TransferBudget::new(512 * 1024, 8 * 1024, Duration::from_secs(10)).unwrap();
    let receipt = relay_copy(
        &source,
        &destination,
        RelayRequest::new(
            ReadRequest::Current("artifacts/bundle.bin".into()),
            "mirror/bundle.bin".into(),
            PutPrecondition::MustNotExist,
            MetadataPolicy::None,
            budget,
        ),
        Cancellation::new(),
    )
    .await
    .unwrap();

    assert_eq!(receipt.strategy(), CopyStrategy::Relay);
    assert_eq!(receipt.enforcement(), ReadEnforcement::Unguarded);
    assert_eq!(receipt.source_bytes_consumed(), payload.len() as u64);
    assert_eq!(receipt.destination_bytes_handed_off(), payload.len() as u64);
    assert_eq!(receipt.destination().path, "mirror/bundle.bin");
    assert_eq!(receipt.destination().size, Some(payload.len() as u64));

    let mut mirrored = Vec::new();
    destination
        .get("mirror/bundle.bin")
        .await
        .unwrap()
        .read_to_end(&mut mirrored)
        .await
        .unwrap();
    assert_eq!(mirrored, payload);

    // Repeating create-only relay fails with Destination + NotCommitted + Conflict::AlreadyExists.
    let conflict = relay_copy(
        &source,
        &destination,
        RelayRequest::new(
            ReadRequest::Current("artifacts/bundle.bin".into()),
            "mirror/bundle.bin".into(),
            PutPrecondition::MustNotExist,
            MetadataPolicy::None,
            budget,
        ),
        Cancellation::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(conflict.kind(), TransferFailureKind::Destination);
    assert_eq!(conflict.commit(), CommitKnowledge::NotCommitted);
    assert!(matches!(
        conflict.cause().and_then(|e| e.cause()),
        Some(StorageError::Conflict {
            kind: ConflictKind::AlreadyExists,
            ..
        })
    ));

    // Zero-byte object relay succeeds cleanly.
    source
        .put(
            "empty.txt",
            Box::new(Cursor::new(Vec::new())),
            PutOptions {
                content_length: Some(0),
                ..PutOptions::default()
            },
        )
        .await
        .unwrap();
    let zero_receipt = relay_copy(
        &source,
        &destination,
        RelayRequest::new(
            ReadRequest::Current("empty.txt".into()),
            "mirror/empty.txt".into(),
            PutPrecondition::None,
            MetadataPolicy::None,
            budget,
        ),
        Cancellation::new(),
    )
    .await
    .unwrap();
    assert_eq!(zero_receipt.source_bytes_consumed(), 0);
    assert_eq!(zero_receipt.destination_bytes_handed_off(), 0);

    // MetadataPolicy::ContentTypeOnly refuses before starting any destination write.
    let meta_refusal = relay_copy(
        &source,
        &destination,
        RelayRequest::new(
            ReadRequest::Current("empty.txt".into()),
            "mirror/never-written.txt".into(),
            PutPrecondition::None,
            MetadataPolicy::ContentTypeOnly,
            budget,
        ),
        Cancellation::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(meta_refusal.kind(), TransferFailureKind::Admission);
    assert_eq!(meta_refusal.commit(), CommitKnowledge::NotStarted);
    assert_eq!(
        meta_refusal.cause().map(|e| e.kind()),
        Some(ExtensionErrorKind::UnsupportedMetadata)
    );
    assert!(!dst_dir.path().join("mirror/never-written.txt").exists());

    // Payload exceeding TransferBudget.max_payload_bytes refuses at admission before writing.
    let small_budget = TransferBudget::new(1024, 512, Duration::from_secs(5)).unwrap();
    let ceiling_refusal = relay_copy(
        &source,
        &destination,
        RelayRequest::new(
            ReadRequest::Current("artifacts/bundle.bin".into()),
            "mirror/too-big.bin".into(),
            PutPrecondition::None,
            MetadataPolicy::None,
            small_budget,
        ),
        Cancellation::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(ceiling_refusal.kind(), TransferFailureKind::Admission);
    assert_eq!(ceiling_refusal.commit(), CommitKnowledge::NotStarted);
    assert_eq!(
        ceiling_refusal.cause().map(|e| e.kind()),
        Some(ExtensionErrorKind::LengthLimit)
    );
    assert!(!dst_dir.path().join("mirror/too-big.bin").exists());
}
