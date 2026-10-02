//! Internal bounded reader; never releases the last fragment before EOF proof.

use crate::relay::TransferFailureKind;
use std::{
    io,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};
use storageprims_core::{BoxedByteStream, OperationControl};
use tokio::io::{AsyncRead, ReadBuf};

#[derive(Default)]
pub(crate) struct Progress {
    pub consumed: u64,
    pub delivered: u64,
    pub validated: bool,
    pub failure: Option<TransferFailureKind>,
}

pub(crate) struct ValidatingReader {
    source: BoxedByteStream,
    expected: u64,
    buffer: Vec<u8>,
    start: usize,
    end: usize,
    progress: Arc<Mutex<Progress>>,
    control: OperationControl,
}

impl ValidatingReader {
    pub fn new(
        source: BoxedByteStream,
        expected: u64,
        buffer: usize,
        progress: Arc<Mutex<Progress>>,
        control: OperationControl,
    ) -> Self {
        Self {
            source,
            expected,
            buffer: vec![0; buffer],
            start: 0,
            end: 0,
            progress,
            control,
        }
    }
    fn fail(&self, kind: TransferFailureKind) -> Poll<io::Result<()>> {
        let mut state = self.progress.lock().unwrap();
        state.failure.get_or_insert(kind);
        // Canonical source messages may contain paths. Record only a typed class.
        Poll::Ready(Err(io::Error::other("relay source validation failed")))
    }
}

impl AsyncRead for ValidatingReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if out.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        if let Some(kind) = crate::relay::stopped(&this.control) {
            return this.fail(kind);
        }
        let (consumed, validated, failed) = {
            let state = this.progress.lock().unwrap();
            (state.consumed, state.validated, state.failure.is_some())
        };
        if failed {
            return this.fail(TransferFailureKind::SourceBody);
        }
        if this.start == this.end && consumed < this.expected {
            let count = (this.expected - consumed).min(this.buffer.len() as u64) as usize;
            let mut read = ReadBuf::new(&mut this.buffer[..count]);
            match Pin::new(&mut this.source).poll_read(cx, &mut read) {
                Poll::Pending => {
                    return match crate::relay::stopped(&this.control) {
                        Some(kind) => this.fail(kind),
                        None => Poll::Pending,
                    }
                }
                Poll::Ready(Err(_)) => {
                    return this.fail(
                        crate::relay::stopped(&this.control)
                            .unwrap_or(TransferFailureKind::SourceBody),
                    )
                }
                Poll::Ready(Ok(())) => {
                    let count = read.filled().len();
                    if count == 0 {
                        return this.fail(
                            crate::relay::stopped(&this.control)
                                .unwrap_or(TransferFailureKind::LengthMismatch),
                        );
                    }
                    this.start = 0;
                    this.end = count;
                    this.progress.lock().unwrap().consumed = consumed
                        .checked_add(count as u64)
                        .expect("read capped at representable declared length");
                }
            }
            if let Some(kind) = crate::relay::stopped(&this.control) {
                return this.fail(kind);
            }
        }
        let consumed = this.progress.lock().unwrap().consumed;
        if consumed == this.expected && !validated {
            let mut byte = [0u8; 1];
            let mut probe = ReadBuf::new(&mut byte);
            match Pin::new(&mut this.source).poll_read(cx, &mut probe) {
                Poll::Pending => {
                    return match crate::relay::stopped(&this.control) {
                        Some(kind) => this.fail(kind),
                        None => Poll::Pending,
                    }
                }
                Poll::Ready(Err(_)) => {
                    return this.fail(
                        crate::relay::stopped(&this.control)
                            .unwrap_or(TransferFailureKind::SourceBody),
                    )
                }
                Poll::Ready(Ok(())) if !probe.filled().is_empty() => {
                    this.progress.lock().unwrap().consumed = consumed
                        .checked_add(1)
                        .expect("budget reserves one representable lookahead byte");
                    if let Some(kind) = crate::relay::stopped(&this.control) {
                        return this.fail(kind);
                    }
                    return this.fail(TransferFailureKind::LengthMismatch);
                }
                Poll::Ready(Ok(())) => this.progress.lock().unwrap().validated = true,
            }
        }
        if let Some(kind) = crate::relay::stopped(&this.control) {
            return this.fail(kind);
        }
        let count = out.remaining().min(this.end - this.start);
        out.put_slice(&this.buffer[this.start..this.start + count]);
        this.start += count;
        let mut state = this.progress.lock().unwrap();
        state.delivered = state
            .delivered
            .checked_add(count as u64)
            .expect("delivery capped at representable declared length");
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use storageprims_core::Cancellation;
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn helper_payload_storage_never_grows_while_withholding_or_partially_delivering() {
        for buffer in [1, 2, 7, 32, 1048576] {
            let progress = Arc::new(Mutex::new(Progress::default()));
            let control = OperationControl::new(
                std::time::Instant::now() + std::time::Duration::from_secs(5),
                Cancellation::new(),
            );
            let mut reader = ValidatingReader::new(
                Box::new(std::io::Cursor::new(vec![9; 100])),
                100,
                buffer,
                progress.clone(),
                control,
            );
            assert_eq!(reader.buffer.len(), buffer);
            assert_eq!(reader.buffer.capacity(), buffer);
            let allocation = reader.buffer.as_ptr();
            let mut byte = [0; 1];
            for _ in 0..100 {
                reader.read_exact(&mut byte).await.unwrap();
                assert_eq!(byte, [9]);
                assert_eq!(reader.buffer.as_ptr(), allocation);
                assert_eq!(reader.buffer.len(), buffer);
                assert_eq!(reader.buffer.capacity(), buffer);
                assert!(reader.end <= buffer && reader.start <= reader.end);
            }
            assert_eq!(reader.read(&mut byte).await.unwrap(), 0);
            let state = progress.lock().unwrap();
            assert!(state.validated);
            assert_eq!((state.consumed, state.delivered), (100, 100));
        }
        // The source owns its own synthetic payload. This assertion covers only
        // the reader's sole payload Vec, not source/SDK heap or allocator overhead.
    }
}
