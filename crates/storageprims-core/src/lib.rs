//! storageprims-core — Core traits, error types, and URI parsing for cloud storage primitives.
//!
//! This crate defines the `StorageProvider` trait that all provider implementations
//! (S3, GCS, Azure, local) must satisfy, along with the canonical `StorageError` type
//! and `StorageUri` parser.

pub mod error;
pub mod uri;

pub use error::StorageError;
pub use uri::{ProviderType, StorageUri};
