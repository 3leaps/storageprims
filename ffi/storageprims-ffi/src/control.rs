use std::os::raw::c_char;

use serde::Serialize;
use storageprims_core::{
    Capability, CopyRequest, ListOptions, ObjectMetadata, ProbeResult, ProviderConfig,
    StorageError, StorageOperation,
};
use storageprims_ops::{
    count_lines, head_lines_with_options, mid_lines_with_options, tail_lines_with_options,
    LineOptions,
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

#[derive(Serialize)]
struct CountLinesResult {
    count: u64,
}

fn ffi_capabilities(capabilities: &[Capability]) -> Vec<Capability> {
    capabilities
        .iter()
        .copied()
        // These operations are callable only from Rust extensions in this cut.
        .filter(|capability| {
            !matches!(
                capability,
                Capability::GuardedRead | Capability::DelimiterListing
            )
        })
        .collect()
}

fn ffi_probe_result(mut result: ProbeResult) -> ProbeResult {
    result.capabilities = ffi_capabilities(&result.capabilities);
    result
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

/// Return the provider's callable capabilities as a JSON array.
///
/// This query reads only the in-process provider registry and does not perform
/// credential resolution, provider probes, or network I/O.
///
/// # Safety
///
/// `out_capabilities_json` must be non-null and writable for a `char*` returned
/// by `storageprims_free_string`.
#[no_mangle]
pub unsafe extern "C" fn storageprims_provider_capabilities(
    handle: u64,
    provider_id: u64,
    out_capabilities_json: *mut *mut c_char,
) -> StorageprimsErrorCode {
    match with_error_boundary(|| {
        initialize_out_json(
            out_capabilities_json,
            "out_capabilities_json",
            StorageOperation::ConfigureProvider,
        )?;
        let runtime = get_runtime(handle)?;
        let provider = runtime.provider(provider_id)?;
        write_json(
            out_capabilities_json,
            &ffi_capabilities(&provider.capabilities()),
        )
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

/// Validate provider credentials and connectivity via the JSON control plane.
///
/// # Safety
///
/// `out_result_json` must be non-null and writable for a `char*` returned by
/// `storageprims_free_string`.
#[no_mangle]
pub unsafe extern "C" fn storageprims_probe(
    handle: u64,
    provider_id: u64,
    out_result_json: *mut *mut c_char,
) -> StorageprimsErrorCode {
    match with_error_boundary(|| {
        initialize_out_json(out_result_json, "out_result_json", StorageOperation::Probe)?;
        let runtime = get_runtime(handle)?;
        let provider = runtime.provider(provider_id)?;
        let result: ProbeResult = runtime.block_on(provider.probe())?;
        write_json(out_result_json, &ffi_probe_result(result))
    }) {
        Ok(()) => StorageprimsErrorCode::Ok,
        Err(code) => code,
    }
}

/// Read the first `n` logical lines via the JSON control plane.
///
/// # Safety
///
/// `key` must be a valid, NUL-terminated UTF-8 C string.
/// If non-null, `opts_json` must be a valid, NUL-terminated UTF-8 JSON string.
/// `out_result_json` must be non-null and writable for a `char*` returned by
/// `storageprims_free_string`.
#[no_mangle]
pub unsafe extern "C" fn storageprims_head_lines(
    handle: u64,
    provider_id: u64,
    key: *const c_char,
    n: u64,
    opts_json: *const c_char,
    out_result_json: *mut *mut c_char,
) -> StorageprimsErrorCode {
    match with_error_boundary(|| {
        initialize_out_json(
            out_result_json,
            "out_result_json",
            StorageOperation::HeadLines,
        )?;
        let runtime = get_runtime(handle)?;
        let provider = runtime.provider(provider_id)?;
        let key = parse_cstr(key, "key")?;
        let options: LineOptions = if opts_json.is_null() {
            LineOptions::default()
        } else {
            parse_json(opts_json, "opts_json")?
        };
        let n = usize::try_from(n).map_err(|_| StorageError::InvalidArgument {
            operation: Some(StorageOperation::HeadLines),
            argument: "n".to_string(),
            reason: "line count exceeds supported size".to_string(),
        })?;
        let result =
            runtime.block_on(head_lines_with_options(provider.as_ref(), &key, n, options))?;
        write_json(out_result_json, &result)
    }) {
        Ok(()) => StorageprimsErrorCode::Ok,
        Err(code) => code,
    }
}

/// Read the last `n` logical lines via the JSON control plane.
///
/// # Safety
///
/// `key` must be a valid, NUL-terminated UTF-8 C string.
/// If non-null, `opts_json` must be a valid, NUL-terminated UTF-8 JSON string.
/// `out_result_json` must be non-null and writable for a `char*` returned by
/// `storageprims_free_string`.
#[no_mangle]
pub unsafe extern "C" fn storageprims_tail_lines(
    handle: u64,
    provider_id: u64,
    key: *const c_char,
    n: u64,
    opts_json: *const c_char,
    out_result_json: *mut *mut c_char,
) -> StorageprimsErrorCode {
    match with_error_boundary(|| {
        initialize_out_json(
            out_result_json,
            "out_result_json",
            StorageOperation::TailLines,
        )?;
        let runtime = get_runtime(handle)?;
        let provider = runtime.provider(provider_id)?;
        let key = parse_cstr(key, "key")?;
        let options: LineOptions = if opts_json.is_null() {
            LineOptions::default()
        } else {
            parse_json(opts_json, "opts_json")?
        };
        let n = usize::try_from(n).map_err(|_| StorageError::InvalidArgument {
            operation: Some(StorageOperation::TailLines),
            argument: "n".to_string(),
            reason: "line count exceeds supported size".to_string(),
        })?;
        let result =
            runtime.block_on(tail_lines_with_options(provider.as_ref(), &key, n, options))?;
        write_json(out_result_json, &result)
    }) {
        Ok(()) => StorageprimsErrorCode::Ok,
        Err(code) => code,
    }
}

/// Read the first `n` logical lines after midpoint alignment via the JSON control plane.
///
/// # Safety
///
/// `key` must be a valid, NUL-terminated UTF-8 C string.
/// If non-null, `opts_json` must be a valid, NUL-terminated UTF-8 JSON string.
/// `out_result_json` must be non-null and writable for a `char*` returned by
/// `storageprims_free_string`.
#[no_mangle]
pub unsafe extern "C" fn storageprims_mid_lines(
    handle: u64,
    provider_id: u64,
    key: *const c_char,
    n: u64,
    opts_json: *const c_char,
    out_result_json: *mut *mut c_char,
) -> StorageprimsErrorCode {
    match with_error_boundary(|| {
        initialize_out_json(
            out_result_json,
            "out_result_json",
            StorageOperation::MidLines,
        )?;
        let runtime = get_runtime(handle)?;
        let provider = runtime.provider(provider_id)?;
        let key = parse_cstr(key, "key")?;
        let options: LineOptions = if opts_json.is_null() {
            LineOptions::default()
        } else {
            parse_json(opts_json, "opts_json")?
        };
        let n = usize::try_from(n).map_err(|_| StorageError::InvalidArgument {
            operation: Some(StorageOperation::MidLines),
            argument: "n".to_string(),
            reason: "line count exceeds supported size".to_string(),
        })?;
        let result =
            runtime.block_on(mid_lines_with_options(provider.as_ref(), &key, n, options))?;
        write_json(out_result_json, &result)
    }) {
        Ok(()) => StorageprimsErrorCode::Ok,
        Err(code) => code,
    }
}

/// Count logical lines via the JSON control plane.
///
/// # Safety
///
/// `key` must be a valid, NUL-terminated UTF-8 C string.
/// `out_result_json` must be non-null and writable for a `char*` returned by
/// `storageprims_free_string`.
#[no_mangle]
pub unsafe extern "C" fn storageprims_count_lines(
    handle: u64,
    provider_id: u64,
    key: *const c_char,
    out_result_json: *mut *mut c_char,
) -> StorageprimsErrorCode {
    match with_error_boundary(|| {
        initialize_out_json(
            out_result_json,
            "out_result_json",
            StorageOperation::CountLines,
        )?;
        let runtime = get_runtime(handle)?;
        let provider = runtime.provider(provider_id)?;
        let key = parse_cstr(key, "key")?;
        let count = runtime.block_on(count_lines(provider.as_ref(), &key))?;
        write_json(out_result_json, &CountLinesResult { count })
    }) {
        Ok(()) => StorageprimsErrorCode::Ok,
        Err(code) => code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use storageprims_core::{CredentialSourceKind, ProbeScope, ProviderKind};

    #[test]
    fn ffi_capability_projection_omits_rust_only_extensions() {
        let projected = ffi_capabilities(&[
            Capability::CredentialProbe,
            Capability::GuardedRead,
            Capability::DelimiterListing,
            Capability::ConditionalPut,
        ]);
        assert_eq!(
            projected,
            vec![Capability::CredentialProbe, Capability::ConditionalPut]
        );
    }

    #[test]
    fn ffi_probe_projection_omits_delimiter_listing() {
        let projected = ffi_probe_result(ProbeResult {
            provider: ProviderKind::S3,
            endpoint: None,
            credential_source: CredentialSourceKind::DefaultChain,
            scope: ProbeScope::ConfiguredContainer,
            probe_method: "test".to_string(),
            latency_ms: 0,
            capabilities: vec![Capability::DelimiterListing, Capability::CredentialProbe],
        });
        assert_eq!(projected.capabilities, vec![Capability::CredentialProbe]);
    }
}
