use std::ffi::{CStr, CString};
use std::os::fd::{FromRawFd, IntoRawFd};
use std::os::raw::c_char;
use std::ptr;

use os_pipe::pipe;
use serde::Serialize;
use storageprims_core::{GetRangeRequest, PutOptions, StorageError, StorageOperation};
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;

use crate::error::{with_error_boundary, StorageprimsErrorCode};
use crate::runtime::{get_runtime, StreamState};

#[derive(Debug, Serialize)]
struct StreamDescriptor {
    stream_id: Option<u64>,
    fd: i32,
    mode: &'static str,
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

async fn copy_stream_to_pipe(
    mut provider_stream: storageprims_core::BoxedByteStream,
    writer: os_pipe::PipeWriter,
    operation: StorageOperation,
) -> storageprims_core::Result<()> {
    let writer = unsafe { std::fs::File::from_raw_fd(writer.into_raw_fd()) };
    let mut writer = tokio::fs::File::from_std(writer);
    let mut buffer = [0_u8; 8192];

    loop {
        let read = provider_stream
            .read(&mut buffer)
            .await
            .map_err(|error| StorageError::Io {
                operation: Some(operation),
                source: error,
            })?;

        if read == 0 {
            writer.flush().await.map_err(|error| StorageError::Io {
                operation: Some(operation),
                source: error,
            })?;
            return Ok(());
        }

        writer
            .write_all(&buffer[..read])
            .await
            .map_err(|error| StorageError::Io {
                operation: Some(operation),
                source: error,
            })?;
    }
}

/// Open a readable stream descriptor for an object.
///
/// The returned JSON descriptor includes both an OS file descriptor and a
/// `stream_id`. Callers must drain or close the descriptor and then call
/// `storageprims_get_finalize` with that `stream_id` to release the pending
/// read state and surface any late read failure. Skipping finalization leaks
/// the tracked read stream and treats late worker failure as unobserved.
///
/// # Safety
///
/// `key` must be a valid, NUL-terminated UTF-8 C string.
/// `out_stream_json` must be non-null and writable for a `char*` returned by
/// `storageprims_free_string`.
#[no_mangle]
pub unsafe extern "C" fn storageprims_get(
    handle: u64,
    provider_id: u64,
    key: *const c_char,
    out_stream_json: *mut *mut c_char,
) -> StorageprimsErrorCode {
    match with_error_boundary(|| {
        initialize_out_json(out_stream_json, "out_stream_json", StorageOperation::Get)?;
        let runtime = get_runtime(handle)?;
        let provider = runtime.provider(provider_id)?;
        let key = parse_cstr(key, "key")?;
        let (reader, writer) = pipe().map_err(|error| StorageError::Io {
            operation: Some(StorageOperation::Get),
            source: error,
        })?;

        let provider_stream = runtime.block_on(provider.get(&key))?;
        let (tx, rx) = tokio::sync::oneshot::channel();
        runtime.spawn(async move {
            let result = copy_stream_to_pipe(provider_stream, writer, StorageOperation::Get).await;
            let _ = tx.send(result);
        });
        let stream_id = runtime.insert_stream(StreamState::PendingRead {
            receiver: rx,
            operation: StorageOperation::Get,
        });

        write_json(
            out_stream_json,
            &StreamDescriptor {
                stream_id: Some(stream_id),
                fd: reader.into_raw_fd(),
                mode: "read",
            },
        )
    }) {
        Ok(()) => StorageprimsErrorCode::Ok,
        Err(code) => code,
    }
}

/// Open a readable stream descriptor for a byte range.
///
/// The returned JSON descriptor includes both an OS file descriptor and a
/// `stream_id`. Callers must drain or close the descriptor and then call
/// `storageprims_get_finalize` with that `stream_id` to release the pending
/// read state and surface any late read failure. Skipping finalization leaks
/// the tracked read stream and treats late worker failure as unobserved.
///
/// # Safety
///
/// `key` must be a valid, NUL-terminated UTF-8 C string.
/// `out_stream_json` must be non-null and writable for a `char*` returned by
/// `storageprims_free_string`.
#[no_mangle]
pub unsafe extern "C" fn storageprims_get_range(
    handle: u64,
    provider_id: u64,
    key: *const c_char,
    offset: u64,
    length: u64,
    out_stream_json: *mut *mut c_char,
) -> StorageprimsErrorCode {
    match with_error_boundary(|| {
        initialize_out_json(
            out_stream_json,
            "out_stream_json",
            StorageOperation::GetRange,
        )?;
        let runtime = get_runtime(handle)?;
        let provider = runtime.provider(provider_id)?;
        let key = parse_cstr(key, "key")?;
        let (reader, writer) = pipe().map_err(|error| StorageError::Io {
            operation: Some(StorageOperation::GetRange),
            source: error,
        })?;

        let provider_stream = runtime.block_on(provider.get_range(GetRangeRequest {
            key,
            offset,
            length,
        }))?;
        let (tx, rx) = tokio::sync::oneshot::channel();
        runtime.spawn(async move {
            let result =
                copy_stream_to_pipe(provider_stream, writer, StorageOperation::GetRange).await;
            let _ = tx.send(result);
        });
        let stream_id = runtime.insert_stream(StreamState::PendingRead {
            receiver: rx,
            operation: StorageOperation::GetRange,
        });

        write_json(
            out_stream_json,
            &StreamDescriptor {
                stream_id: Some(stream_id),
                fd: reader.into_raw_fd(),
                mode: "read",
            },
        )
    }) {
        Ok(()) => StorageprimsErrorCode::Ok,
        Err(code) => code,
    }
}

/// Begin a streamed upload and return a writable descriptor.
///
/// # Safety
///
/// `key` must be a valid, NUL-terminated UTF-8 C string.
/// If non-null, `metadata_json` must be a valid, NUL-terminated UTF-8 JSON string.
/// `out_stream_json` must be non-null and writable for a `char*` returned by
/// `storageprims_free_string`.
#[no_mangle]
pub unsafe extern "C" fn storageprims_put_begin(
    handle: u64,
    provider_id: u64,
    key: *const c_char,
    metadata_json: *const c_char,
    out_stream_json: *mut *mut c_char,
) -> StorageprimsErrorCode {
    match with_error_boundary(|| {
        initialize_out_json(out_stream_json, "out_stream_json", StorageOperation::Put)?;
        let runtime = get_runtime(handle)?;
        let provider = runtime.provider(provider_id)?;
        let key = parse_cstr(key, "key")?;
        let options: PutOptions = if metadata_json.is_null() {
            PutOptions::default()
        } else {
            parse_json(metadata_json, "metadata_json")?
        };

        let (reader, writer) = pipe().map_err(|error| StorageError::Io {
            operation: Some(StorageOperation::Put),
            source: error,
        })?;
        let (tx, rx) = tokio::sync::oneshot::channel();

        runtime.spawn(async move {
            let reader = unsafe { std::fs::File::from_raw_fd(reader.into_raw_fd()) };
            let body = Box::new(tokio::fs::File::from_std(reader));
            let result = provider.put(&key, body, options).await;
            let _ = tx.send(result);
        });

        let stream_id = runtime.insert_stream(StreamState::PendingPut(rx));
        write_json(
            out_stream_json,
            &StreamDescriptor {
                stream_id: Some(stream_id),
                fd: writer.into_raw_fd(),
                mode: "write",
            },
        )
    }) {
        Ok(()) => StorageprimsErrorCode::Ok,
        Err(code) => code,
    }
}

/// Finalize a previously opened streamed upload and return the JSON put result.
///
/// # Safety
///
/// `out_result_json` must be non-null and writable for a `char*` returned by
/// `storageprims_free_string`.
#[no_mangle]
pub unsafe extern "C" fn storageprims_put_finalize(
    handle: u64,
    stream_id: u64,
    out_result_json: *mut *mut c_char,
) -> StorageprimsErrorCode {
    match with_error_boundary(|| {
        initialize_out_json(out_result_json, "out_result_json", StorageOperation::Put)?;
        let runtime = get_runtime(handle)?;
        match runtime.take_stream(stream_id)? {
            StreamState::PendingRead { operation, .. } => Err(StorageError::InvalidArgument {
                operation: Some(operation),
                argument: "stream_id".to_string(),
                reason: format!("stream id {stream_id} belongs to a read stream"),
            }),
            StreamState::PendingPut(receiver) => {
                let result = runtime.block_on(async {
                    receiver.await.map_err(|_| StorageError::Other {
                        provider: None,
                        operation: Some(StorageOperation::Put),
                        detail: "put worker dropped before finalization".to_string(),
                        source: None,
                    })?
                })?;
                write_json(out_result_json, &result)
            }
        }
    }) {
        Ok(()) => StorageprimsErrorCode::Ok,
        Err(code) => code,
    }
}

/// Finalize a previously opened read stream and surface any late read failure.
///
/// Call this exactly once after the consumer has drained or closed the file
/// descriptor returned by `storageprims_get` or `storageprims_get_range`.
/// Finalization releases the pending read entry and reports any failure that
/// occurred while copying provider bytes into the pipe.
#[no_mangle]
pub extern "C" fn storageprims_get_finalize(handle: u64, stream_id: u64) -> StorageprimsErrorCode {
    match with_error_boundary(|| {
        let runtime = get_runtime(handle)?;
        match runtime.take_stream(stream_id)? {
            StreamState::PendingRead {
                receiver,
                operation,
            } => runtime.block_on(async {
                receiver.await.map_err(|_| StorageError::Other {
                    provider: None,
                    operation: Some(operation),
                    detail: "read worker dropped before finalization".to_string(),
                    source: None,
                })?
            }),
            StreamState::PendingPut(_) => Err(StorageError::InvalidArgument {
                operation: Some(StorageOperation::Put),
                argument: "stream_id".to_string(),
                reason: format!("stream id {stream_id} belongs to a write stream"),
            }),
        }
    }) {
        Ok(()) => StorageprimsErrorCode::Ok,
        Err(code) => code,
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::os::fd::FromRawFd;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use super::*;

    struct FailingReader {
        bytes: Vec<u8>,
        cursor: usize,
        fail_after: usize,
    }

    impl tokio::io::AsyncRead for FailingReader {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            if self.cursor >= self.fail_after {
                return Poll::Ready(Err(io::Error::other("forced read failure")));
            }

            let remaining_before_failure = self.fail_after - self.cursor;
            let remaining_bytes = self.bytes.len().saturating_sub(self.cursor);
            let to_copy = remaining_before_failure
                .min(remaining_bytes)
                .min(buf.remaining());

            if to_copy == 0 {
                return Poll::Ready(Ok(()));
            }

            let end = self.cursor + to_copy;
            buf.put_slice(&self.bytes[self.cursor..end]);
            self.cursor = end;
            Poll::Ready(Ok(()))
        }
    }

    #[test]
    fn copy_stream_to_pipe_surfaces_mid_stream_read_failure() {
        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should build");
        let (reader, writer) = pipe().expect("pipe should build");
        let stream: storageprims_core::BoxedByteStream = Box::new(FailingReader {
            bytes: b"abcdef".to_vec(),
            cursor: 0,
            fail_after: 3,
        });

        let result = runtime.block_on(copy_stream_to_pipe(stream, writer, StorageOperation::Get));
        let mut output = Vec::new();
        let mut reader = unsafe { std::fs::File::from_raw_fd(reader.into_raw_fd()) };
        std::io::Read::read_to_end(&mut reader, &mut output).expect("read pipe output");

        match result {
            Err(StorageError::Io {
                operation: Some(StorageOperation::Get),
                ..
            }) => {}
            other => panic!("expected get I/O error, got {other:?}"),
        }
        assert_eq!(output, b"abc");
    }
}
