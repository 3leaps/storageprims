use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::ptr;

use serde::Serialize;
use storageprims_core::{StorageError, StorageOperation};

pub(crate) fn parse_cstr(value: *const c_char, field: &str) -> storageprims_core::Result<String> {
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

pub(crate) fn parse_json<T: serde::de::DeserializeOwned>(
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

pub(crate) fn write_json<T: Serialize>(
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

pub(crate) fn initialize_out_json(
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

pub(crate) fn initialize_out_u64(
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
