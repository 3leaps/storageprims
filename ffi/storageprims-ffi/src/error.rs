use std::cell::RefCell;
use std::ffi::CString;
use std::os::raw::c_char;

use serde::Serialize;
use storageprims_core::{StorageError, StorageErrorCode};

#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageprimsErrorCode {
    Ok = 0,
    InvalidUri = 1,
    InvalidArgument = 2,
    NotFound = 3,
    ContainerNotFound = 4,
    AccessDenied = 5,
    InvalidCredentials = 6,
    Throttled = 7,
    ProviderUnavailable = 8,
    UnsupportedCapability = 9,
    Conflict = 10,
    Io = 11,
    Other = 99,
}

#[derive(Debug, Serialize)]
struct ErrorPayload {
    code: String,
    message: String,
}

#[derive(Default)]
struct ErrorState {
    code: StorageprimsErrorCode,
    detail_json: Option<String>,
}

thread_local! {
    static LAST_ERROR: RefCell<ErrorState> = RefCell::new(ErrorState::default());
}

impl Default for StorageprimsErrorCode {
    fn default() -> Self {
        Self::Ok
    }
}

impl From<StorageErrorCode> for StorageprimsErrorCode {
    fn from(value: StorageErrorCode) -> Self {
        match value {
            StorageErrorCode::InvalidUri => Self::InvalidUri,
            StorageErrorCode::InvalidArgument => Self::InvalidArgument,
            StorageErrorCode::NotFound => Self::NotFound,
            StorageErrorCode::ContainerNotFound => Self::ContainerNotFound,
            StorageErrorCode::AccessDenied => Self::AccessDenied,
            StorageErrorCode::InvalidCredentials => Self::InvalidCredentials,
            StorageErrorCode::Throttled => Self::Throttled,
            StorageErrorCode::ProviderUnavailable => Self::ProviderUnavailable,
            StorageErrorCode::UnsupportedCapability => Self::UnsupportedCapability,
            StorageErrorCode::Conflict => Self::Conflict,
            StorageErrorCode::Io => Self::Io,
            StorageErrorCode::Other => Self::Other,
        }
    }
}

pub(crate) fn clear_error_state() {
    LAST_ERROR.with(|state| {
        *state.borrow_mut() = ErrorState::default();
    });
}

pub(crate) fn set_error(error: &StorageError) -> StorageprimsErrorCode {
    let code = StorageprimsErrorCode::from(error.code());
    let payload = ErrorPayload {
        code: format!("{:?}", error.code()).to_ascii_lowercase(),
        message: error.to_string(),
    };
    let detail_json = serde_json::to_string(&payload).unwrap_or_else(|_| {
        "{\"code\":\"other\",\"message\":\"failed to serialize error\"}".to_string()
    });

    LAST_ERROR.with(|state| {
        let mut state = state.borrow_mut();
        state.code = code;
        state.detail_json = Some(detail_json);
    });

    code
}

pub(crate) fn with_error_boundary<T>(
    operation: impl FnOnce() -> storageprims_core::Result<T>,
) -> Result<T, StorageprimsErrorCode> {
    clear_error_state();
    operation().map_err(|error| set_error(&error))
}

#[no_mangle]
pub extern "C" fn storageprims_last_error_code() -> StorageprimsErrorCode {
    LAST_ERROR.with(|state| state.borrow().code)
}

#[no_mangle]
pub extern "C" fn storageprims_last_error() -> *mut c_char {
    LAST_ERROR.with(|state| {
        let state = state.borrow();
        let detail = state.detail_json.as_deref().unwrap_or("{}");
        CString::new(detail.replace('\0', "?"))
            .map(CString::into_raw)
            .unwrap_or(std::ptr::null_mut())
    })
}

#[no_mangle]
pub extern "C" fn storageprims_clear_error() {
    clear_error_state();
}

#[cfg(test)]
mod tests {
    use std::ffi::CStr;

    use storageprims_core::{ProviderKind, StorageOperation};

    use super::*;

    #[test]
    fn error_state_defaults_to_ok() {
        clear_error_state();
        assert_eq!(storageprims_last_error_code(), StorageprimsErrorCode::Ok);
    }

    #[test]
    fn last_error_returns_json() {
        let error = StorageError::NotFound {
            provider: ProviderKind::S3,
            operation: StorageOperation::Head,
            path: "missing.txt".to_string(),
        };
        set_error(&error);

        let detail = storageprims_last_error();
        assert!(!detail.is_null());

        let value = unsafe { CStr::from_ptr(detail).to_str().expect("valid utf-8") };
        assert!(value.contains("\"code\":\"notfound\""));

        unsafe { crate::storageprims_free_string(detail) };
    }
}
