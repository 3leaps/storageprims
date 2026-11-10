//! The test pattern shown in docs/guides/testing-with-the-local-provider.md.
//!
//! Application code accepts `&dyn StorageProvider`; tests hand it a
//! `LocalProvider` rooted in a fresh temporary directory.

use std::{
    io::Cursor,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use storageprims_core::{
    ConflictKind, ListOptions, PutOptions, PutPrecondition, StorageError, StorageProvider,
};
use storageprims_local::LocalProvider;
use tokio::io::AsyncReadExt;

// --- Application code under test: it only knows the trait. ---

/// Record a job result once; a duplicate run must not overwrite it.
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
    match store
        .put(&key, Box::new(Cursor::new(result.to_vec())), options)
        .await
    {
        Ok(_) => Ok(true),
        Err(StorageError::Conflict {
            kind: ConflictKind::AlreadyExists,
            ..
        }) => Ok(false),
        Err(error) => Err(error),
    }
}

// --- Test support: a temporary root removed on drop. ---

static NEXT: AtomicU64 = AtomicU64::new(0);

struct TempRoot(PathBuf);

impl TempRoot {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "myapp-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// --- Tests ---

#[tokio::test]
async fn records_a_result_once() {
    let root = TempRoot::new();
    let store = LocalProvider::new(root.path()).unwrap();

    assert!(record_result(&store, "job-1", b"{\"ok\":true}")
        .await
        .unwrap());
    assert!(!record_result(&store, "job-1", b"{\"ok\":false}")
        .await
        .unwrap());

    // Assert through the same API the application uses...
    let mut body = String::new();
    store
        .get("results/job-1.json")
        .await
        .unwrap()
        .read_to_string(&mut body)
        .await
        .unwrap();
    assert_eq!(body, "{\"ok\":true}");

    // ...or directly on disk, since the layout is just files under the root.
    let on_disk = std::fs::read(root.path().join("results").join("job-1.json")).unwrap();
    assert_eq!(on_disk, b"{\"ok\":true}");

    let listing = store
        .list(ListOptions {
            prefix: Some("results/".into()),
            ..ListOptions::default()
        })
        .await
        .unwrap();
    assert_eq!(listing.objects.len(), 1);
}

#[tokio::test]
async fn independent_roots_do_not_share_state() {
    let first = TempRoot::new();
    let second = TempRoot::new();
    let a = LocalProvider::new(first.path()).unwrap();
    let b = LocalProvider::new(second.path()).unwrap();

    assert!(record_result(&a, "job-2", b"{}").await.unwrap());
    assert!(record_result(&b, "job-2", b"{}").await.unwrap());
}
