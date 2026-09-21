use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd};
use std::os::raw::c_char;

use os_pipe::pipe;
use serde::Serialize;
use storageprims_core::{
    require_guarded_reads, Capability, GuardedRangeRequest, PutOptions, SourceSelector,
    StorageError, StorageOperation,
};
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;

use crate::error::{with_error_boundary, StorageprimsErrorCode};
use crate::ffi_support::{initialize_out_json, parse_cstr, parse_json, write_json};
use crate::runtime::{get_runtime, StreamState};

#[derive(Debug, Serialize)]
struct StreamDescriptor {
    stream_id: Option<u64>,
    fd: i32,
    mode: &'static str,
}

async fn copy_stream_to_pipe(
    mut provider_stream: storageprims_core::BoxedByteStream,
    writer: os_pipe::PipeWriter,
    operation: StorageOperation,
    expected_bytes: Option<u64>,
) -> storageprims_core::Result<()> {
    let writer = unsafe { std::fs::File::from_raw_fd(writer.into_raw_fd()) };
    let mut writer = tokio::fs::File::from_std(writer);
    let mut buffer = [0_u8; 8192];
    let mut remaining = expected_bytes;

    loop {
        let read_limit = remaining
            .map(|value| usize::try_from(value.min(buffer.len() as u64)).unwrap_or(buffer.len()))
            .unwrap_or(buffer.len());

        if read_limit == 0 {
            return Ok(());
        }

        let read = provider_stream
            .read(&mut buffer[..read_limit])
            .await
            .map_err(|error| StorageError::Io {
                operation: Some(operation),
                source: error,
            })?;

        if read == 0 {
            if let Some(remaining) = remaining {
                return Err(StorageError::Io {
                    operation: Some(operation),
                    source: std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        format!("stream ended with {remaining} bytes remaining"),
                    ),
                });
            }
            return Ok(());
        }

        writer
            .write_all(&buffer[..read])
            .await
            .map_err(|error| StorageError::Io {
                operation: Some(operation),
                source: error,
            })?;

        if let Some(remaining_bytes) = remaining.as_mut() {
            *remaining_bytes = remaining_bytes.saturating_sub(read as u64);
            if *remaining_bytes == 0 {
                return Ok(());
            }
        }
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
/// `storageprims_free_string`. The returned fd is backed by a bounded OS pipe,
/// so callers should keep draining it; otherwise the provider worker may block
/// under backpressure until the fd is drained or closed.
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
        let stream_id = runtime.insert_stream(StreamState::PendingRead {
            receiver: rx,
            operation: StorageOperation::Get,
        });
        let descriptor = StreamDescriptor {
            stream_id: Some(stream_id),
            fd: reader.as_raw_fd(),
            mode: "read",
        };

        if let Err(error) = write_json(out_stream_json, &descriptor) {
            let _ = runtime.remove_stream(stream_id);
            return Err(error);
        }

        let _ = reader.into_raw_fd();
        runtime.spawn(async move {
            let result =
                copy_stream_to_pipe(provider_stream, writer, StorageOperation::Get, None).await;
            let _ = tx.send(result);
        });
        Ok(())
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
/// `storageprims_free_string`. The returned fd is backed by a bounded OS pipe,
/// so callers should keep draining it; otherwise the provider worker may block
/// under backpressure until the fd is drained or closed.
///
/// The range uses an internal source-guarded observation and selected range
/// request. A range proven clipped at object EOF finalizes successfully with
/// its returned length; an ignored range, wrong window, or transport truncation
/// remains an error. Providers that cannot enforce a guarded selection return
/// `UnsupportedCapability` rather than exposing unvalidated current bytes.
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
        if length == 0 || offset.checked_add(length - 1).is_none() {
            return Err(StorageError::InvalidArgument {
                operation: Some(StorageOperation::GetRange),
                argument: "offset/length".to_string(),
                reason: "range must be non-empty and must not overflow".to_string(),
            });
        }
        let (reader, writer) = pipe().map_err(|error| StorageError::Io {
            operation: Some(StorageOperation::GetRange),
            source: error,
        })?;

        // The Unix data-plane ABI does not expose selectors, but its range
        // finalization must still distinguish a proven EOF clip from a
        // truncated or ignored range. Bind and consume one guarded selection
        // internally rather than accepting arbitrary short current-object data.
        let guarded = require_guarded_reads(provider.as_ref(), StorageOperation::GetRange)?;
        let observation = runtime.block_on(guarded.observe_source(&key))?;
        let selector = if let Some(token) = observation.receipt.native_version {
            SourceSelector::NativeVersion { token }
        } else if let Some(token) = observation.receipt.validator {
            SourceSelector::ValidatorMatch { token }
        } else {
            return Err(StorageError::UnsupportedCapability {
                provider: provider.provider_kind(),
                operation: StorageOperation::GetRange,
                capability: Capability::GuardedRead,
            });
        };
        let selection = guarded.bind_guarded_read(&key, selector)?;
        let response = runtime.block_on(guarded.guarded_get_range(GuardedRangeRequest {
            selection,
            offset,
            length,
        }))?;
        let expected_bytes = response
            .receipt
            .returned_window
            .and_then(|window| window.checked_len())
            .ok_or_else(|| StorageError::Io {
                operation: Some(StorageOperation::GetRange),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "guarded range response is missing a valid returned window",
                ),
            })?;
        let provider_stream = response.reader;
        let (tx, rx) = tokio::sync::oneshot::channel();
        let stream_id = runtime.insert_stream(StreamState::PendingRead {
            receiver: rx,
            operation: StorageOperation::GetRange,
        });
        let descriptor = StreamDescriptor {
            stream_id: Some(stream_id),
            fd: reader.as_raw_fd(),
            mode: "read",
        };

        if let Err(error) = write_json(out_stream_json, &descriptor) {
            let _ = runtime.remove_stream(stream_id);
            return Err(error);
        }

        let _ = reader.into_raw_fd();
        runtime.spawn(async move {
            let result = copy_stream_to_pipe(
                provider_stream,
                writer,
                StorageOperation::GetRange,
                Some(expected_bytes),
            )
            .await;
            let _ = tx.send(result);
        });
        Ok(())
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
/// `storageprims_free_string`. The returned fd is the write end of a bounded
/// OS pipe, so producer writes may block until the provider worker drains the
/// pipe or the fd is closed.
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
        let stream_id = runtime.insert_stream(StreamState::PendingPut(rx));
        let descriptor = StreamDescriptor {
            stream_id: Some(stream_id),
            fd: writer.as_raw_fd(),
            mode: "write",
        };

        if let Err(error) = write_json(out_stream_json, &descriptor) {
            let _ = runtime.remove_stream(stream_id);
            return Err(error);
        }

        let _ = writer.into_raw_fd();
        runtime.spawn(async move {
            let reader = unsafe { std::fs::File::from_raw_fd(reader.into_raw_fd()) };
            let body = Box::new(tokio::fs::File::from_std(reader));
            let result = provider.put(&key, body, options).await;
            let _ = tx.send(result);
        });
        Ok(())
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

        let result = runtime.block_on(copy_stream_to_pipe(
            stream,
            writer,
            StorageOperation::Get,
            None,
        ));
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

    #[test]
    fn copy_stream_to_pipe_stops_after_expected_range_bytes() {
        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should build");
        let (reader, writer) = pipe().expect("pipe should build");
        let stream: storageprims_core::BoxedByteStream = Box::new(FailingReader {
            bytes: b"abcdef".to_vec(),
            cursor: 0,
            fail_after: 3,
        });

        let result = runtime.block_on(copy_stream_to_pipe(
            stream,
            writer,
            StorageOperation::GetRange,
            Some(3),
        ));
        let mut output = Vec::new();
        let mut reader = unsafe { std::fs::File::from_raw_fd(reader.into_raw_fd()) };
        std::io::Read::read_to_end(&mut reader, &mut output).expect("read pipe output");

        assert!(
            result.is_ok(),
            "expected range copy to stop cleanly: {result:?}"
        );
        assert_eq!(output, b"abc");
    }
}
