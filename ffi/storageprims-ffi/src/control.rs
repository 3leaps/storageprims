use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::ptr;

use serde::Serialize;
use storageprims_core::{
    CopyRequest, ListOptions, ObjectMetadata, ProviderConfig, StorageError, StorageOperation,
};

use crate::error::{with_error_boundary, StorageprimsErrorCode};
use crate::runtime::{build_provider, get_runtime};

#[derive(Serialize)]
struct DeleteResult {
    deleted: bool,
}

fn parse_cstr(value: *const c_char, field: &str) -> storageprims_core::Result<String> {
    if value.is_null() {
        return Err(StorageError::InvalidArgument {
            operation: None,
            argument: field.to_string(),
            reason: "value must not be null".to_string(),
        });
    }

    let text = unsafe { CStr::from_ptr(value) }.to_str().map_err(|error| {
        StorageError::InvalidArgument {
            operation: None,
            argument: field.to_string(),
            reason: format!("value must be valid UTF-8: {error}"),
        }
    })?;

    Ok(text.to_string())
}

fn parse_json<T: serde::de::DeserializeOwned>(
    value: *const c_char,
    field: &str,
) -> storageprims_core::Result<T> {
    let text = parse_cstr(value, field)?;
    serde_json::from_str(&text).map_err(|error| StorageError::InvalidArgument {
        operation: None,
        argument: field.to_string(),
        reason: format!("invalid JSON payload: {error}"),
    })
}

fn write_json<T: Serialize>(
    out_json: *mut *mut c_char,
    value: &T,
) -> storageprims_core::Result<()> {
    if out_json.is_null() {
        return Err(StorageError::InvalidArgument {
            operation: None,
            argument: "out_json".to_string(),
            reason: "output pointer must not be null".to_string(),
        });
    }

    let json = serde_json::to_string(value).map_err(|error| StorageError::Other {
        provider: None,
        operation: None,
        detail: format!("failed to serialize JSON result: {error}"),
        source: None,
    })?;
    let c_json = CString::new(json.replace('\0', "?")).map_err(|error| StorageError::Other {
        provider: None,
        operation: None,
        detail: format!("failed to encode JSON result: {error}"),
        source: None,
    })?;

    unsafe {
        *out_json = c_json.into_raw();
    }
    Ok(())
}

fn initialize_out_json(
    out_json: *mut *mut c_char,
    field: &str,
    operation: StorageOperation,
) -> storageprims_core::Result<()> {
    if out_json.is_null() {
        return Err(StorageError::InvalidArgument {
            operation: Some(operation),
            argument: field.to_string(),
            reason: "output pointer must not be null".to_string(),
        });
    }

    unsafe {
        *out_json = ptr::null_mut();
    }
    Ok(())
}

fn initialize_out_u64(
    out_value: *mut u64,
    field: &str,
    operation: StorageOperation,
) -> storageprims_core::Result<()> {
    if out_value.is_null() {
        return Err(StorageError::InvalidArgument {
            operation: Some(operation),
            argument: field.to_string(),
            reason: "output pointer must not be null".to_string(),
        });
    }

    unsafe {
        *out_value = 0;
    }
    Ok(())
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
