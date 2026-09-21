use std::cell::RefCell;
use std::ffi::CString;
use std::os::raw::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};

use serde::Serialize;
use storageprims_core::{
    ConflictKind, InspectionKind, InspectionLimitDimension, StorageError, StorageErrorCode,
};

#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StorageprimsErrorCode {
    #[default]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<ConflictKind>,
    /// Additive bounded-inspection detail. Numeric `Other = 99` is unchanged.
    #[serde(skip_serializing_if = "Option::is_none")]
    inspection_version: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    inspection_kind: Option<InspectionKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    limit_dimension: Option<InspectionLimitDimension>,
    #[serde(skip_serializing_if = "Option::is_none")]
    configured_limit: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    consumed: Option<u64>,
}

#[derive(Default)]
struct ErrorState {
    code: StorageprimsErrorCode,
    detail_json: Option<String>,
}

thread_local! {
    static LAST_ERROR: RefCell<ErrorState> = RefCell::new(ErrorState::default());
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
        kind: match error {
            StorageError::Conflict { kind, .. } => Some(*kind),
            _ => None,
        },
        inspection_version: match error {
            StorageError::Inspection { .. } => Some(1),
            _ => None,
        },
        inspection_kind: match error {
            StorageError::Inspection { kind, .. } => Some(*kind),
            _ => None,
        },
        limit_dimension: match error {
            StorageError::Inspection {
                limit_dimension, ..
            } => *limit_dimension,
            _ => None,
        },
        configured_limit: match error {
            StorageError::Inspection {
                configured_limit, ..
            } => Some(*configured_limit),
            _ => None,
        },
        consumed: match error {
            StorageError::Inspection { consumed, .. } => Some(*consumed),
            _ => None,
        },
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

fn set_other_error(message: String) -> StorageprimsErrorCode {
    let payload = ErrorPayload {
        code: "other".to_string(),
        message,
        kind: None,
        inspection_version: None,
        inspection_kind: None,
        limit_dimension: None,
        configured_limit: None,
        consumed: None,
    };
    let detail_json = serde_json::to_string(&payload).unwrap_or_else(|_| {
        "{\"code\":\"other\",\"message\":\"failed to serialize error\"}".to_string()
    });

    LAST_ERROR.with(|state| {
        let mut state = state.borrow_mut();
        state.code = StorageprimsErrorCode::Other;
        state.detail_json = Some(detail_json);
    });

    StorageprimsErrorCode::Other
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    match payload.downcast::<String>() {
        Ok(message) => *message,
        Err(payload) => match payload.downcast::<&'static str>() {
            Ok(message) => (*message).to_string(),
            Err(_) => "panic payload was not a string".to_string(),
        },
    }
}

pub(crate) fn with_error_boundary<T>(
    operation: impl FnOnce() -> storageprims_core::Result<T>,
) -> Result<T, StorageprimsErrorCode> {
    clear_error_state();
    match catch_unwind(AssertUnwindSafe(operation)) {
        Ok(result) => result.map_err(|error| set_error(&error)),
        Err(payload) => Err(set_other_error(format!(
            "panic caught at storageprims FFI boundary: {}",
            panic_message(payload)
        ))),
    }
}

pub(crate) fn with_init_boundary(
    operation: impl FnOnce() -> storageprims_core::Result<u64>,
) -> u64 {
    with_error_boundary(operation).unwrap_or_default()
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

    use storageprims_core::{
        InspectionKind, InspectionLimitDimension, ProviderKind, StorageOperation,
    };

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

    #[test]
    fn with_error_boundary_catches_panics() {
        let result = with_error_boundary::<()>(|| panic!("ffi panic test"));

        assert_eq!(result, Err(StorageprimsErrorCode::Other));
        assert_eq!(storageprims_last_error_code(), StorageprimsErrorCode::Other);

        let detail = storageprims_last_error();
        let value = unsafe { CStr::from_ptr(detail).to_str().expect("valid utf-8") };
        assert!(value.contains("panic caught at storageprims FFI boundary"));
        assert!(value.contains("ffi panic test"));
        unsafe { crate::storageprims_free_string(detail) };
    }

    #[test]
    fn conflict_error_includes_structured_kind() {
        let error = StorageError::Conflict {
            provider: ProviderKind::S3,
            operation: StorageOperation::Put,
            target: Some("key".to_string()),
            kind: ConflictKind::TokenMismatch,
            detail: "precondition failed".to_string(),
        };
        set_error(&error);

        let detail = storageprims_last_error();
        let value = unsafe { CStr::from_ptr(detail).to_str().expect("valid utf-8") };
        let payload: serde_json::Value = serde_json::from_str(value).expect("valid JSON");
        assert_eq!(payload["code"], "conflict");
        assert_eq!(payload["kind"], "token_mismatch");
        unsafe { crate::storageprims_free_string(detail) };
    }

    #[test]
    fn inspection_error_uses_bounded_versioned_detail_without_selector_data() {
        let error = StorageError::Inspection {
            provider: Some(ProviderKind::S3),
            operation: StorageOperation::PreviewBytes,
            kind: InspectionKind::LimitExceeded,
            limit_dimension: Some(InspectionLimitDimension::PayloadBytes),
            configured_limit: 1024,
            consumed: 1025,
        };
        assert_eq!(set_error(&error), StorageprimsErrorCode::Other);
        let detail = storageprims_last_error();
        let value = unsafe { CStr::from_ptr(detail).to_str().expect("valid utf-8") };
        let payload: serde_json::Value = serde_json::from_str(value).expect("valid JSON");
        assert_eq!(payload["inspection_version"], 1);
        assert_eq!(payload["inspection_kind"], "limit_exceeded");
        assert_eq!(payload["limit_dimension"], "payload_bytes");
        assert_eq!(payload["configured_limit"], 1024);
        assert_eq!(payload["consumed"], 1025);
        assert!(payload.get("selector").is_none());
        assert!(payload.get("body").is_none());
        unsafe { crate::storageprims_free_string(detail) };
    }
}
