use std::os::raw::c_char;

use serde::Serialize;
use storageprims_core::{
    CopyRequest, ListOptions, ObjectMetadata, ProviderConfig, StorageError, StorageOperation,
};

use crate::error::{with_error_boundary, StorageprimsErrorCode};
use crate::ffi_support::{
    initialize_out_json, initialize_out_u64, parse_cstr, parse_json, write_json,
};
use crate::runtime::{build_provider, get_runtime};

#[derive(Serialize)]
struct DeleteResult {
    deleted: bool,
}

/// Create a provider instance from JSON configuration and return an opaque provider id.
///
/// # Safety
///
/// `config_json` must be a valid, NUL-terminated UTF-8 C string.
/// `out_provider_id` must be non-null and writable for a `u64`.
#[no_mangle]
pub unsafe extern "C" fn storageprims_provider_create(
    handle: u64,
    config_json: *const c_char,
    out_provider_id: *mut u64,
) -> StorageprimsErrorCode {
    match with_error_boundary(|| {
        initialize_out_u64(
            out_provider_id,
            "out_provider_id",
            StorageOperation::ConfigureProvider,
        )?;

        let runtime = get_runtime(handle)?;
        let config: ProviderConfig = parse_json(config_json, "config_json")?;
        let provider = runtime.block_on(build_provider(config))?;
        let provider_id = runtime.insert_provider(provider);

        unsafe {
            *out_provider_id = provider_id;
        }
        Ok(())
    }) {
        Ok(()) => StorageprimsErrorCode::Ok,
        Err(code) => code,
    }
}

#[no_mangle]
pub extern "C" fn storageprims_provider_destroy(
    handle: u64,
    provider_id: u64,
) -> StorageprimsErrorCode {
    match with_error_boundary(|| {
        let runtime = get_runtime(handle)?;
        if runtime.remove_provider(provider_id) {
            Ok(())
        } else {
            Err(StorageError::InvalidArgument {
                operation: Some(StorageOperation::ConfigureProvider),
                argument: "provider_id".to_string(),
                reason: format!("unknown provider id {provider_id}"),
            })
        }
    }) {
        Ok(()) => StorageprimsErrorCode::Ok,
        Err(code) => code,
    }
}

/// List objects via the JSON control plane.
///
/// # Safety
///
/// `request_json` must be a valid, NUL-terminated UTF-8 C string.
/// `out_result_json` must be non-null and writable for a `char*` returned by
/// `storageprims_free_string`.
#[no_mangle]
pub unsafe extern "C" fn storageprims_list(
    handle: u64,
    provider_id: u64,
    request_json: *const c_char,
    out_result_json: *mut *mut c_char,
) -> StorageprimsErrorCode {
    match with_error_boundary(|| {
        initialize_out_json(out_result_json, "out_result_json", StorageOperation::List)?;
        let runtime = get_runtime(handle)?;
        let provider = runtime.provider(provider_id)?;
        let request: ListOptions = parse_json(request_json, "request_json")?;
        let result = runtime.block_on(provider.list(request))?;
        write_json(out_result_json, &result)
    }) {
        Ok(()) => StorageprimsErrorCode::Ok,
        Err(code) => code,
    }
}

/// Fetch object metadata via the JSON control plane.
///
/// # Safety
///
/// `key` must be a valid, NUL-terminated UTF-8 C string.
/// `out_metadata_json` must be non-null and writable for a `char*` returned by
/// `storageprims_free_string`.
#[no_mangle]
pub unsafe extern "C" fn storageprims_head(
    handle: u64,
    provider_id: u64,
    key: *const c_char,
    out_metadata_json: *mut *mut c_char,
) -> StorageprimsErrorCode {
    match with_error_boundary(|| {
        initialize_out_json(
            out_metadata_json,
            "out_metadata_json",
            StorageOperation::Head,
        )?;
        let runtime = get_runtime(handle)?;
        let provider = runtime.provider(provider_id)?;
        let key = parse_cstr(key, "key")?;
        let metadata: ObjectMetadata = runtime.block_on(provider.head(&key))?;
        write_json(out_metadata_json, &metadata)
    }) {
        Ok(()) => StorageprimsErrorCode::Ok,
        Err(code) => code,
    }
}

/// Delete an object via the JSON control plane.
///
/// # Safety
///
/// `key` must be a valid, NUL-terminated UTF-8 C string.
/// `out_result_json` must be non-null and writable for a `char*` returned by
/// `storageprims_free_string`.
#[no_mangle]
pub unsafe extern "C" fn storageprims_delete(
    handle: u64,
    provider_id: u64,
    key: *const c_char,
    out_result_json: *mut *mut c_char,
) -> StorageprimsErrorCode {
    match with_error_boundary(|| {
        initialize_out_json(out_result_json, "out_result_json", StorageOperation::Delete)?;
        let runtime = get_runtime(handle)?;
        let provider = runtime.provider(provider_id)?;
        let key = parse_cstr(key, "key")?;
        runtime.block_on(provider.delete(&key))?;
        write_json(out_result_json, &DeleteResult { deleted: true })
    }) {
        Ok(()) => StorageprimsErrorCode::Ok,
        Err(code) => code,
    }
}

/// Copy an object via the JSON control plane.
///
/// # Safety
///
/// `request_json` must be a valid, NUL-terminated UTF-8 C string.
/// `out_result_json` must be non-null and writable for a `char*` returned by
/// `storageprims_free_string`.
#[no_mangle]
pub unsafe extern "C" fn storageprims_copy(
    handle: u64,
    provider_id: u64,
    request_json: *const c_char,
    out_result_json: *mut *mut c_char,
) -> StorageprimsErrorCode {
    match with_error_boundary(|| {
        initialize_out_json(out_result_json, "out_result_json", StorageOperation::Copy)?;
        let runtime = get_runtime(handle)?;
        let provider = runtime.provider(provider_id)?;
        let request: CopyRequest = parse_json(request_json, "request_json")?;
        let result = runtime.block_on(provider.copy(request))?;
        write_json(out_result_json, &result)
    }) {
        Ok(()) => StorageprimsErrorCode::Ok,
        Err(code) => code,
    }
}
