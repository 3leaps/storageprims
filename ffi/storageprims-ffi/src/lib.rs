//! storageprims-ffi: C-ABI exports for storageprims.

use std::ffi::CString;
use std::os::raw::c_char;

mod control;
mod error;
mod ffi_support;
mod runtime;
mod stream;

pub use control::{
    storageprims_copy, storageprims_count_lines, storageprims_delete, storageprims_head,
    storageprims_head_lines, storageprims_list, storageprims_mid_lines,
    storageprims_provider_create, storageprims_provider_destroy, storageprims_tail_lines,
};
pub use error::{
    storageprims_clear_error, storageprims_last_error, storageprims_last_error_code,
    StorageprimsErrorCode,
};
pub use stream::{
    storageprims_get, storageprims_get_finalize, storageprims_get_range, storageprims_put_begin,
    storageprims_put_finalize,
};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const ABI_VERSION: u32 = 1;

#[no_mangle]
pub extern "C" fn storageprims_version() -> *const c_char {
    static VERSION_CSTR: std::sync::OnceLock<CString> = std::sync::OnceLock::new();
    VERSION_CSTR
        .get_or_init(|| CString::new(VERSION).expect("VERSION should not contain null bytes"))
        .as_ptr()
}

#[no_mangle]
pub extern "C" fn storageprims_abi_version() -> u32 {
    ABI_VERSION
}

/// Initialize a storageprims FFI runtime and return an opaque handle.
///
/// Returns `0` on failure and records the failure detail in the thread-local
/// last-error slot available through `storageprims_last_error_code` and
/// `storageprims_last_error`.
#[no_mangle]
pub extern "C" fn storageprims_init() -> u64 {
    error::with_init_boundary(runtime::init_runtime)
}

#[no_mangle]
pub extern "C" fn storageprims_shutdown(handle: u64) -> StorageprimsErrorCode {
    match error::with_error_boundary(|| runtime::shutdown_runtime(handle)) {
        Ok(()) => StorageprimsErrorCode::Ok,
        Err(code) => code,
    }
}

/// Free a string returned by a storageprims FFI function.
///
/// # Safety
///
/// `s` must either be null or a pointer previously returned by a storageprims
/// FFI function that transfers string ownership to the caller.
#[no_mangle]
pub unsafe extern "C" fn storageprims_free_string(s: *mut c_char) {
    if s.is_null() {
        return;
    }
    let _ = CString::from_raw(s);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_and_shutdown_round_trip() {
        let handle = storageprims_init();
        assert!(handle > 0);
        assert_eq!(storageprims_shutdown(handle), StorageprimsErrorCode::Ok);
    }
}
