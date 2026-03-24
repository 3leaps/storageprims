#![cfg(all(feature = "integration", unix))]

use std::collections::BTreeMap;
use std::ffi::{CStr, CString};
use std::io::{Read, Write};
use std::os::fd::FromRawFd;
use std::time::{SystemTime, UNIX_EPOCH};

use aws_credential_types::Credentials;
use aws_sdk_s3::Client;
use serde::Deserialize;
use storageprims_core::{
    CopyRequest, CredentialSource, CredentialSourceKind, ListOptions, ProbeResult, ProviderConfig,
    ProviderKind, PutOptions, TargetConfig,
};
use storageprims_ffi::{
    storageprims_clear_error, storageprims_copy, storageprims_count_lines, storageprims_delete,
    storageprims_free_string, storageprims_get, storageprims_get_finalize, storageprims_get_range,
    storageprims_head, storageprims_head_lines, storageprims_init, storageprims_last_error,
    storageprims_list, storageprims_mid_lines, storageprims_probe, storageprims_provider_create,
    storageprims_provider_destroy, storageprims_put_begin, storageprims_put_finalize,
    storageprims_shutdown, storageprims_tail_lines, StorageprimsErrorCode,
};
use storageprims_ops::{LineOptions, LineResult};

const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:4566";
const TEST_REGION: &str = "us-east-1";

#[derive(Debug, Deserialize)]
struct StreamDescriptor {
    stream_id: Option<u64>,
    fd: i32,
    mode: String,
}

#[derive(Debug, Deserialize)]
struct DeleteResult {
    deleted: bool,
}

#[derive(Debug, Deserialize)]
struct PutResult {
    path: String,
}

#[derive(Debug, Deserialize)]
struct ObjectMetadata {
    path: String,
}

#[derive(Debug, Deserialize)]
struct ListResult {
    objects: Vec<ObjectSummary>,
}

#[derive(Debug, Deserialize)]
struct ObjectSummary {
    path: String,
}

#[derive(Debug, Deserialize)]
struct CountLinesResult {
    count: u64,
}

fn endpoint() -> String {
    std::env::var("STORAGEPRIMS_S3_TEST_ENDPOINT").unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string())
}

#[test]
fn ffi_round_trips_control_and_data_plane_against_localstack() {
    let endpoint = endpoint();
    let bucket = format!("storageprims-ffi-it-{}", unique_suffix());
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should build");
    runtime.block_on(create_bucket(&endpoint, &bucket));

    let handle = storageprims_init();
    assert!(handle > 0);

    let provider_id = create_provider(handle, &bucket, &endpoint);
    put_object(handle, provider_id, "fixtures/data.txt", b"hello from ffi");

    let head: ObjectMetadata = call_json_out(|out| {
        let key = CString::new("fixtures/data.txt").unwrap();
        unsafe { storageprims_head(handle, provider_id, key.as_ptr(), out) }
    });
    assert_eq!(head.path, "fixtures/data.txt");

    let listed: ListResult = call_json_out(|out| {
        let request = serde_json::to_string(&ListOptions {
            prefix: Some("fixtures/".to_string()),
            continuation_token: None,
            max_keys: Some(10),
        })
        .unwrap();
        let request = CString::new(request).unwrap();
        unsafe { storageprims_list(handle, provider_id, request.as_ptr(), out) }
    });
    assert!(listed
        .objects
        .iter()
        .any(|object| object.path == "fixtures/data.txt"));

    let body = read_object(handle, provider_id, "fixtures/data.txt");
    assert_eq!(body, b"hello from ffi");
    let range = read_object_range(handle, provider_id, "fixtures/data.txt", 6, 4);
    assert_eq!(range, b"from");

    let copied: serde_json::Value = call_json_out(|out| {
        let request = serde_json::to_string(&CopyRequest {
            source: "fixtures/data.txt".to_string(),
            destination: "fixtures/copy.txt".to_string(),
        })
        .unwrap();
        let request = CString::new(request).unwrap();
        unsafe { storageprims_copy(handle, provider_id, request.as_ptr(), out) }
    });
    assert_eq!(copied["destination"], "fixtures/copy.txt");

    let deleted: DeleteResult = call_json_out(|out| {
        let key = CString::new("fixtures/copy.txt").unwrap();
        unsafe { storageprims_delete(handle, provider_id, key.as_ptr(), out) }
    });
    assert!(deleted.deleted);

    assert_eq!(
        storageprims_provider_destroy(handle, provider_id),
        StorageprimsErrorCode::Ok
    );
    assert_eq!(storageprims_shutdown(handle), StorageprimsErrorCode::Ok);
}

#[test]
fn ffi_line_ops_round_trip_against_localstack() {
    let endpoint = endpoint();
    let bucket = format!("storageprims-ffi-it-{}", unique_suffix());
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should build");
    runtime.block_on(create_bucket(&endpoint, &bucket));

    let handle = storageprims_init();
    assert!(handle > 0);

    let provider_id = create_provider(handle, &bucket, &endpoint);
    put_object(
        handle,
        provider_id,
        "fixtures/lines.txt",
        b"aa\nbb\ncc\ndd\nee\n",
    );

    let head: LineResult = call_json_out(|out| {
        let key = CString::new("fixtures/lines.txt").unwrap();
        unsafe {
            storageprims_head_lines(handle, provider_id, key.as_ptr(), 2, std::ptr::null(), out)
        }
    });
    assert_eq!(head.lines, vec!["aa", "bb"]);
    assert_eq!(head.byte_offset_start, 0);
    assert_eq!(head.byte_offset_end, 6);

    let options = CString::new(serde_json::to_string(&LineOptions::default()).unwrap()).unwrap();

    let tail: LineResult = call_json_out(|out| {
        let key = CString::new("fixtures/lines.txt").unwrap();
        unsafe {
            storageprims_tail_lines(handle, provider_id, key.as_ptr(), 2, options.as_ptr(), out)
        }
    });
    assert_eq!(tail.lines, vec!["dd", "ee"]);

    let mid: LineResult = call_json_out(|out| {
        let key = CString::new("fixtures/lines.txt").unwrap();
        unsafe {
            storageprims_mid_lines(handle, provider_id, key.as_ptr(), 2, options.as_ptr(), out)
        }
    });
    assert_eq!(mid.lines, vec!["dd", "ee"]);

    let counted: CountLinesResult = call_json_out(|out| {
        let key = CString::new("fixtures/lines.txt").unwrap();
        unsafe { storageprims_count_lines(handle, provider_id, key.as_ptr(), out) }
    });
    assert_eq!(counted.count, 5);

    assert_eq!(
        storageprims_provider_destroy(handle, provider_id),
        StorageprimsErrorCode::Ok
    );
    assert_eq!(storageprims_shutdown(handle), StorageprimsErrorCode::Ok);
}

#[test]
fn ffi_probe_uses_head_bucket_fallback_against_localstack() {
    let endpoint = endpoint();
    let bucket = format!("storageprims-ffi-it-{}", unique_suffix());
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should build");
    runtime.block_on(create_bucket(&endpoint, &bucket));

    let handle = storageprims_init();
    assert!(handle > 0);

    let provider_id = create_provider(handle, &bucket, &endpoint);

    let result: ProbeResult =
        call_json_out(|out| unsafe { storageprims_probe(handle, provider_id, out) });
    assert_eq!(result.provider, ProviderKind::S3);
    assert_eq!(result.probe_method, "s3:HeadBucket");
    assert_eq!(result.credential_source, CredentialSourceKind::InlineStatic);
    assert!(result.latency_ms < 30_000);
    assert!(result
        .capabilities
        .contains(&storageprims_core::Capability::CredentialProbe));
    assert_eq!(result.endpoint.as_deref(), Some(endpoint.as_str()));

    assert_eq!(
        storageprims_provider_destroy(handle, provider_id),
        StorageprimsErrorCode::Ok
    );
    assert_eq!(storageprims_shutdown(handle), StorageprimsErrorCode::Ok);
}

#[test]
fn ffi_provider_create_reports_json_error_for_invalid_config() {
    storageprims_clear_error();
    let handle = storageprims_init();
    assert!(handle > 0);

    let invalid = CString::new("{\"provider\":\"s3\"").unwrap();
    let mut provider_id = 41_u64;
    let code = unsafe { storageprims_provider_create(handle, invalid.as_ptr(), &mut provider_id) };
    assert_eq!(code, StorageprimsErrorCode::InvalidArgument);
    assert_eq!(provider_id, 0);

    let detail_ptr = storageprims_last_error();
    let detail = unsafe { CStr::from_ptr(detail_ptr) }
        .to_str()
        .expect("last error should be valid utf-8")
        .to_string();
    assert!(detail.contains("\"code\":\"invalidargument\""));
    unsafe { storageprims_free_string(detail_ptr) };

    assert_eq!(storageprims_shutdown(handle), StorageprimsErrorCode::Ok);
}

#[test]
fn ffi_failure_paths_clear_stale_output_values() {
    let endpoint = endpoint();
    let bucket = format!("storageprims-ffi-it-{}", unique_suffix());
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should build");
    runtime.block_on(create_bucket(&endpoint, &bucket));

    let handle = storageprims_init();
    assert!(handle > 0);

    let provider_id = create_provider(handle, &bucket, &endpoint);
    put_object(handle, provider_id, "fixtures/data.txt", b"hello from ffi");

    let invalid_list = CString::new("{").unwrap();
    let mut list_json = std::ptr::dangling_mut::<std::os::raw::c_char>();
    let code =
        unsafe { storageprims_list(handle, provider_id, invalid_list.as_ptr(), &mut list_json) };
    assert_eq!(code, StorageprimsErrorCode::InvalidArgument);
    assert!(list_json.is_null());

    let missing_key = CString::new("fixtures/missing.txt").unwrap();
    let mut get_json = std::ptr::dangling_mut::<std::os::raw::c_char>();
    let code =
        unsafe { storageprims_get(handle, provider_id, missing_key.as_ptr(), &mut get_json) };
    assert_eq!(code, StorageprimsErrorCode::NotFound);
    assert!(get_json.is_null());

    let key = CString::new("fixtures/data.txt").unwrap();
    let invalid_metadata = CString::new("{").unwrap();
    let mut put_json = std::ptr::dangling_mut::<std::os::raw::c_char>();
    let code = unsafe {
        storageprims_put_begin(
            handle,
            provider_id,
            key.as_ptr(),
            invalid_metadata.as_ptr(),
            &mut put_json,
        )
    };
    assert_eq!(code, StorageprimsErrorCode::InvalidArgument);
    assert!(put_json.is_null());

    assert_eq!(
        storageprims_provider_destroy(handle, provider_id),
        StorageprimsErrorCode::Ok
    );
    assert_eq!(storageprims_shutdown(handle), StorageprimsErrorCode::Ok);
}

#[test]
fn ffi_delete_rejects_null_output_pointer_before_side_effects() {
    let endpoint = endpoint();
    let bucket = format!("storageprims-ffi-it-{}", unique_suffix());
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should build");
    runtime.block_on(create_bucket(&endpoint, &bucket));

    let handle = storageprims_init();
    assert!(handle > 0);

    let provider_id = create_provider(handle, &bucket, &endpoint);
    put_object(
        handle,
        provider_id,
        "fixtures/delete-guard.txt",
        b"still here",
    );

    let key = CString::new("fixtures/delete-guard.txt").unwrap();
    let code =
        unsafe { storageprims_delete(handle, provider_id, key.as_ptr(), std::ptr::null_mut()) };
    assert_eq!(code, StorageprimsErrorCode::InvalidArgument);

    let head: ObjectMetadata =
        call_json_out(|out| unsafe { storageprims_head(handle, provider_id, key.as_ptr(), out) });
    assert_eq!(head.path, "fixtures/delete-guard.txt");

    assert_eq!(
        storageprims_provider_destroy(handle, provider_id),
        StorageprimsErrorCode::Ok
    );
    assert_eq!(storageprims_shutdown(handle), StorageprimsErrorCode::Ok);
}

fn create_provider(handle: u64, bucket: &str, endpoint: &str) -> u64 {
    let config = ProviderConfig {
        provider: ProviderKind::S3,
        target: TargetConfig {
            container: Some(bucket.to_string()),
            region: Some(TEST_REGION.to_string()),
            endpoint: Some(endpoint.to_string()),
            force_path_style: Some(true),
            ..TargetConfig::default()
        },
        credentials: CredentialSource::InlineStatic {
            values: credentials_map(),
        },
    };

    let mut provider_id = 0_u64;
    let config = CString::new(serde_json::to_string(&config).unwrap()).unwrap();
    let code = unsafe { storageprims_provider_create(handle, config.as_ptr(), &mut provider_id) };
    assert_eq!(code, StorageprimsErrorCode::Ok);
    assert!(provider_id > 0);
    provider_id
}

fn put_object(handle: u64, provider_id: u64, key: &str, body: &[u8]) {
    let mut out_json = std::ptr::null_mut();
    let key = CString::new(key).unwrap();
    let options = CString::new(
        serde_json::to_string(&PutOptions {
            content_length: Some(body.len() as u64),
            ..PutOptions::default()
        })
        .unwrap(),
    )
    .unwrap();

    let code = unsafe {
        storageprims_put_begin(
            handle,
            provider_id,
            key.as_ptr(),
            options.as_ptr(),
            &mut out_json,
        )
    };
    assert_eq!(code, StorageprimsErrorCode::Ok);

    let descriptor: StreamDescriptor = take_json_string(out_json);
    assert_eq!(descriptor.mode, "write");
    let stream_id = descriptor.stream_id.expect("put should return a stream id");

    let mut file = unsafe { std::fs::File::from_raw_fd(descriptor.fd) };
    file.write_all(body).expect("ffi write should succeed");
    drop(file);

    let result: PutResult =
        call_json_out(|out| unsafe { storageprims_put_finalize(handle, stream_id, out) });
    assert_eq!(result.path, key.to_str().unwrap());
}

fn read_object(handle: u64, provider_id: u64, key: &str) -> Vec<u8> {
    let key = CString::new(key).unwrap();
    let descriptor: StreamDescriptor =
        call_json_out(|out| unsafe { storageprims_get(handle, provider_id, key.as_ptr(), out) });
    assert_eq!(descriptor.mode, "read");
    let stream_id = descriptor
        .stream_id
        .expect("get should return a stream id for finalization");

    let mut file = unsafe { std::fs::File::from_raw_fd(descriptor.fd) };
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)
        .expect("ffi read should succeed");
    drop(file);
    assert_finalize_ok(handle, stream_id);
    buffer
}

fn read_object_range(
    handle: u64,
    provider_id: u64,
    key: &str,
    offset: u64,
    length: u64,
) -> Vec<u8> {
    let key = CString::new(key).unwrap();
    let descriptor: StreamDescriptor = call_json_out(|out| unsafe {
        storageprims_get_range(handle, provider_id, key.as_ptr(), offset, length, out)
    });
    assert_eq!(descriptor.mode, "read");
    let stream_id = descriptor
        .stream_id
        .expect("get_range should return a stream id for finalization");

    let mut file = unsafe { std::fs::File::from_raw_fd(descriptor.fd) };
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)
        .expect("ffi range read should succeed");
    drop(file);
    assert_finalize_ok(handle, stream_id);
    buffer
}

fn assert_finalize_ok(handle: u64, stream_id: u64) {
    let code = storageprims_get_finalize(handle, stream_id);
    if code != StorageprimsErrorCode::Ok {
        panic!("get finalize failed with {code:?}: {}", last_error_detail());
    }
}

fn last_error_detail() -> String {
    let detail_ptr = storageprims_last_error();
    if detail_ptr.is_null() {
        return "<null last error>".to_string();
    }

    let detail = unsafe { CStr::from_ptr(detail_ptr) }
        .to_str()
        .expect("last error should be valid utf-8")
        .to_string();
    unsafe { storageprims_free_string(detail_ptr) };
    detail
}

fn call_json_out<T: for<'de> Deserialize<'de>>(
    f: impl FnOnce(*mut *mut std::os::raw::c_char) -> StorageprimsErrorCode,
) -> T {
    let mut out_json = std::ptr::null_mut();
    let code = f(&mut out_json);
    assert_eq!(code, StorageprimsErrorCode::Ok);
    take_json_string(out_json)
}

fn take_json_string<T: for<'de> Deserialize<'de>>(ptr: *mut std::os::raw::c_char) -> T {
    assert!(!ptr.is_null());
    let json = unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .expect("ffi output should be valid utf-8")
        .to_string();
    unsafe { storageprims_free_string(ptr) };
    serde_json::from_str(&json).expect("ffi output should deserialize")
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

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be monotonic enough for tests")
        .as_nanos()
}
