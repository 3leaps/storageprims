//! Store, read, list, and delete objects under a local directory root.
//!
//! Run with `cargo run -p storageprims-local --example local_roundtrip`.
//! The example creates and removes its own temporary directory.

use std::io::Cursor;

use storageprims_core::{
    require_delimiter_listing, ConflictKind, DelimiterListRequest, ListOptions, PutOptions,
    PutPrecondition, StorageError, StorageProvider,
};
use storageprims_local::LocalProvider;
use tokio::io::AsyncReadExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::temp_dir().join(format!("storageprims-example-{}", std::process::id()));
    std::fs::create_dir(&root)?;
    let result = run(&root).await;
    std::fs::remove_dir_all(&root)?;
    result
}

async fn run(root: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    // The root directory must already exist. Every key resolves inside it.
    let store = LocalProvider::new(root)?;

    // Plain put: creates the object or atomically replaces it.
    let body = b"hello, storageprims\n".to_vec();
    store
        .put(
            "reports/2026/summary.txt",
            Box::new(Cursor::new(body.clone())),
            PutOptions {
                content_length: Some(body.len() as u64),
                ..PutOptions::default()
            },
        )
        .await?;

    // Create-only put: refused when the key already exists.
    let second = b"second writer".to_vec();
    let attempt = store
        .put(
            "reports/2026/summary.txt",
            Box::new(Cursor::new(second.clone())),
            PutOptions {
                content_length: Some(second.len() as u64),
                precondition: PutPrecondition::MustNotExist,
                ..PutOptions::default()
            },
        )
        .await;
    match attempt {
        Err(StorageError::Conflict {
            kind: ConflictKind::AlreadyExists,
            ..
        }) => println!("create-only put refused: the object already exists"),
        other => return Err(format!("unexpected create-only result: {other:?}").into()),
    }

    // Read the object back.
    let mut text = String::new();
    store
        .get("reports/2026/summary.txt")
        .await?
        .read_to_string(&mut text)
        .await?;
    let meta = store.head("reports/2026/summary.txt").await?;
    println!("read {} bytes: {}", meta.size, text.trim_end());

    // Flat listing returns one page (up to 1000 objects) under a prefix, in key
    // order. Loop on `continuation_token` while `is_truncated` for more.
    let listing = store
        .list(ListOptions {
            prefix: Some("reports/".into()),
            ..ListOptions::default()
        })
        .await?;
    for object in &listing.objects {
        println!("object: {} ({} bytes)", object.path, object.size);
    }

    // Delimiter listing groups the next path segment into common prefixes.
    let delimited = require_delimiter_listing(&store)?
        .list_delimited(DelimiterListRequest {
            prefix: Some("reports/".into()),
            delimiter: "/".into(),
            continuation_token: None,
            max_keys: None,
        })
        .await?;
    for prefix in &delimited.common_prefixes {
        println!("common prefix: {prefix}");
    }

    store.delete("reports/2026/summary.txt").await?;
    Ok(())
}
