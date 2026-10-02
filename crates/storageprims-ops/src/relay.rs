//! Explicit bounded one-object relay contracts. No recursive copy or retry policy.

use crate::relay_reader::{Progress, ValidatingReader};
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use storageprims_core::{
    Cancellation, Evidence, ExtensionErrorKind, FutureResult, OperationControl, PutOptions,
    SourceSelector, StorageProvider,
};
use storageprims_core::{
    CommitKnowledge, CopyStrategy, ExtensionError, PutPrecondition, PutResult, ReadEnforcement,
    ReadRequest, SourceReceipt, StorageError, WriteMode,
};
use tokio::io::AsyncReadExt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum MetadataPolicy {
    None,
    ContentTypeOnly,
    ContentTypeAndUserMetadata,
}

/// Finite helper payload limits, not a bound on provider metadata or SDK heap.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct TransferBudget {
    max_payload_bytes: u64,
    buffer_bytes: usize,
    timeout: Duration,
}

impl TransferBudget {
    pub fn new(
        max_payload_bytes: u64,
        buffer_bytes: usize,
        timeout: Duration,
    ) -> Result<Self, StorageError> {
        if max_payload_bytes == u64::MAX
            || !(1..=1048576).contains(&buffer_bytes)
            || timeout.is_zero()
            || std::time::Instant::now().checked_add(timeout).is_none()
        {
            return Err(StorageError::InvalidArgument {
                operation: Some(storageprims_core::StorageOperation::Copy),
                argument: "transfer_budget".into(),
                reason: "requires representable payload plus lookahead, buffer 1..=1048576, and finite positive timeout".into(),
            });
        }
        Ok(Self {
            max_payload_bytes,
            buffer_bytes,
            timeout,
        })
    }
    pub fn max_payload_bytes(&self) -> u64 {
        self.max_payload_bytes
    }
    pub fn buffer_bytes(&self) -> usize {
        self.buffer_bytes
    }
    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

#[non_exhaustive]
pub struct RelayRequest {
    source: ReadRequest,
    destination: String,
    precondition: PutPrecondition,
    metadata: MetadataPolicy,
    budget: TransferBudget,
    mode: WriteMode,
}

impl RelayRequest {
    pub fn new(
        source: ReadRequest,
        destination: String,
        precondition: PutPrecondition,
        metadata: MetadataPolicy,
        budget: TransferBudget,
    ) -> Self {
        Self {
            source,
            destination,
            precondition,
            metadata,
            budget,
            mode: WriteMode::SinglePart,
        }
    }
    pub fn source(&self) -> &ReadRequest {
        &self.source
    }
    pub fn destination(&self) -> &str {
        &self.destination
    }
    pub fn precondition(&self) -> &PutPrecondition {
        &self.precondition
    }
    pub fn metadata(&self) -> MetadataPolicy {
        self.metadata
    }
    pub fn budget(&self) -> &TransferBudget {
        &self.budget
    }
    pub fn write_mode(&self) -> WriteMode {
        self.mode
    }
    pub fn with_write_mode(mut self, mode: WriteMode) -> Self {
        self.mode = mode;
        self
    }
}
impl fmt::Debug for RelayRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RelayRequest")
            .field("metadata", &self.metadata)
            .field("budget", &self.budget)
            .field("mode", &self.mode)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TransferFailureKind {
    InvalidRequest,
    SourceExtensionUnavailable,
    DestinationExtensionUnavailable,
    Admission,
    SourceOpen,
    SourceBody,
    LengthMismatch,
    Destination,
    Cancelled,
    Deadline,
}

#[non_exhaustive]
pub struct TransferReceipt {
    source: SourceReceipt,
    enforcement: ReadEnforcement,
    destination: PutResult,
    consumed: u64,
    delivered: u64,
    metadata: MetadataPolicy,
    mode: WriteMode,
}
impl TransferReceipt {
    pub fn source(&self) -> &SourceReceipt {
        &self.source
    }
    pub fn enforcement(&self) -> ReadEnforcement {
        self.enforcement
    }
    pub fn destination(&self) -> &PutResult {
        &self.destination
    }
    pub fn source_bytes_consumed(&self) -> u64 {
        self.consumed
    }
    pub fn destination_bytes_handed_off(&self) -> u64 {
        self.delivered
    }
    pub fn metadata(&self) -> MetadataPolicy {
        self.metadata
    }
    pub fn write_mode(&self) -> WriteMode {
        self.mode
    }
    pub fn strategy(&self) -> CopyStrategy {
        CopyStrategy::Relay
    }
}
impl fmt::Debug for TransferReceipt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TransferReceipt")
            .field("enforcement", &self.enforcement)
            .field("consumed", &self.consumed)
            .field("delivered", &self.delivered)
            .field("metadata", &self.metadata)
            .field("mode", &self.mode)
            .finish_non_exhaustive()
    }
}

#[non_exhaustive]
pub struct TransferFailure {
    kind: TransferFailureKind,
    commit: CommitKnowledge,
    source: Option<SourceReceipt>,
    consumed: u64,
    delivered: u64,
    cause: Option<ExtensionError>,
}
impl TransferFailure {
    pub fn kind(&self) -> TransferFailureKind {
        self.kind
    }
    pub fn commit(&self) -> CommitKnowledge {
        self.commit
    }
    pub fn source(&self) -> Option<&SourceReceipt> {
        self.source.as_ref()
    }
    pub fn source_bytes_consumed(&self) -> u64 {
        self.consumed
    }
    pub fn destination_bytes_handed_off(&self) -> u64 {
        self.delivered
    }
    pub fn cause(&self) -> Option<&ExtensionError> {
        self.cause.as_ref()
    }
}
impl fmt::Debug for TransferFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TransferFailure")
            .field("kind", &self.kind)
            .field("commit", &self.commit)
            .field("consumed", &self.consumed)
            .field("delivered", &self.delivered)
            .finish_non_exhaustive()
    }
}
impl fmt::Display for TransferFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "relay {:?} ({:?})", self.kind, self.commit)
    }
}
impl std::error::Error for TransferFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause
            .as_ref()
            .map(|cause| cause as &(dyn std::error::Error + 'static))
    }
}

fn failure(
    kind: TransferFailureKind,
    commit: CommitKnowledge,
    source: Option<SourceReceipt>,
    progress: &Arc<Mutex<Progress>>,
    cause: Option<ExtensionError>,
) -> TransferFailure {
    let state = progress.lock().unwrap();
    TransferFailure {
        kind,
        commit,
        source,
        consumed: state.consumed,
        delivered: state.delivered,
        cause,
    }
}

pub(crate) fn stopped(control: &OperationControl) -> Option<TransferFailureKind> {
    if control.cancellation().is_cancelled() {
        Some(TransferFailureKind::Cancelled)
    } else if std::time::Instant::now() >= control.deadline() {
        Some(TransferFailureKind::Deadline)
    } else {
        None
    }
}

/// Relay one whole object through supplied provider handles with one absolute
/// deadline, no retry/HEAD/delete, explicit metadata policy and exact EOF proof.
/// Conforming providers redact every canonical error source before returning it.
pub fn relay_copy<'a>(
    source: &'a dyn StorageProvider,
    destination: &'a dyn StorageProvider,
    request: RelayRequest,
    cancellation: Cancellation,
) -> FutureResult<'a, TransferReceipt, TransferFailure> {
    Box::pin(async move {
        let progress = Arc::new(Mutex::new(Progress::default()));
        let no_write = CommitKnowledge::NotStarted;
        let Some(deadline) = std::time::Instant::now().checked_add(request.budget.timeout) else {
            return Err(failure(
                TransferFailureKind::InvalidRequest,
                no_write,
                None,
                &progress,
                None,
            ));
        };
        let control = OperationControl::new(deadline, cancellation);
        if let Some(kind) = stopped(&control) {
            return Err(failure(kind, no_write, None, &progress, None));
        }
        if request.destination.is_empty() {
            return Err(failure(
                TransferFailureKind::InvalidRequest,
                no_write,
                None,
                &progress,
                None,
            ));
        }
        let reader = source.read_responses().ok_or_else(|| {
            failure(
                TransferFailureKind::SourceExtensionUnavailable,
                no_write,
                None,
                &progress,
                None,
            )
        })?;
        let writer = destination.admitted_puts().ok_or_else(|| {
            failure(
                TransferFailureKind::DestinationExtensionUnavailable,
                no_write,
                None,
                &progress,
                None,
            )
        })?;
        let read_support = reader.read_support();
        let readable = match &request.source {
            ReadRequest::Current(key) => !key.is_empty() && read_support.current(),
            ReadRequest::Guarded(selection) => match selection.selector() {
                SourceSelector::NativeVersion { .. } => read_support.native_version(),
                SourceSelector::ValidatorMatch { .. } => read_support.validator_match(),
            },
            _ => false,
        };
        let support = writer.put_support();
        let predicate = match &request.precondition {
            PutPrecondition::None => support.unconditional(),
            PutPrecondition::MustNotExist => support.create_only(),
            PutPrecondition::Match { .. } => support.validator_match(),
        };
        let policy = match request.metadata {
            MetadataPolicy::None => true,
            MetadataPolicy::ContentTypeOnly => support.content_type(),
            MetadataPolicy::ContentTypeAndUserMetadata => {
                support.content_type() && support.user_metadata()
            }
        };
        let refusal = if !readable {
            Some(ExtensionErrorKind::UnsupportedReadSelection)
        } else if support.mode() != request.mode {
            Some(ExtensionErrorKind::UnsupportedWriteMode)
        } else if !predicate {
            Some(ExtensionErrorKind::UnsupportedPredicate)
        } else if !policy {
            Some(ExtensionErrorKind::UnsupportedMetadata)
        } else {
            None
        };
        if let Some(kind) = refusal {
            return Err(failure(
                TransferFailureKind::Admission,
                no_write,
                None,
                &progress,
                Some(ExtensionError::new(kind, None)),
            ));
        }
        if let Some(kind) = stopped(&control) {
            return Err(failure(kind, no_write, None, &progress, None));
        }
        let response = tokio::select! {
            biased;
            _ = control.cancellation().cancelled() => return Err(failure(TransferFailureKind::Cancelled, no_write, None, &progress, None)),
            _ = tokio::time::sleep_until(deadline.into()) => return Err(failure(TransferFailureKind::Deadline, no_write, None, &progress, None)),
            result = reader.read_response(request.source, control.clone()) => result,
        };
        let response = response.map_err(|error| {
            failure(
                stopped(&control).unwrap_or(TransferFailureKind::SourceOpen),
                no_write,
                None,
                &progress,
                Some(error),
            )
        })?;
        let (body, receipt, enforcement, metadata) = response.into_parts();
        if let Some(kind) = stopped(&control) {
            return Err(failure(kind, no_write, Some(receipt), &progress, None));
        }
        let length = receipt.total_size.ok_or_else(|| {
            failure(
                TransferFailureKind::Admission,
                no_write,
                Some(receipt.clone()),
                &progress,
                Some(ExtensionError::new(
                    ExtensionErrorKind::LengthRequired,
                    None,
                )),
            )
        })?;
        if length > request.budget.max_payload_bytes || length > support.max_content_length() {
            return Err(failure(
                TransferFailureKind::Admission,
                no_write,
                Some(receipt),
                &progress,
                Some(ExtensionError::new(ExtensionErrorKind::LengthLimit, None)),
            ));
        }
        let mut options = PutOptions {
            content_length: Some(length),
            precondition: request.precondition,
            ..PutOptions::default()
        };
        if request.metadata != MetadataPolicy::None {
            match metadata.content_type() {
                Evidence::Available(value) => options.content_type = value.clone(),
                _ => {
                    return Err(failure(
                        TransferFailureKind::Admission,
                        no_write,
                        Some(receipt),
                        &progress,
                        Some(ExtensionError::new(
                            ExtensionErrorKind::UnsupportedMetadata,
                            None,
                        )),
                    ))
                }
            }
        }
        if request.metadata == MetadataPolicy::ContentTypeAndUserMetadata {
            match metadata.user_metadata() {
                Evidence::Available(value) => options.metadata = value.clone(),
                _ => {
                    return Err(failure(
                        TransferFailureKind::Admission,
                        no_write,
                        Some(receipt),
                        &progress,
                        Some(ExtensionError::new(
                            ExtensionErrorKind::UnsupportedMetadata,
                            None,
                        )),
                    ))
                }
            }
        }
        if let Some(kind) = stopped(&control) {
            return Err(failure(kind, no_write, Some(receipt), &progress, None));
        }
        let prepared = writer
            .admit_put(&request.destination, options, request.mode)
            .map_err(|error| {
                failure(
                    stopped(&control).unwrap_or(TransferFailureKind::Admission),
                    no_write,
                    Some(receipt.clone()),
                    &progress,
                    Some(error),
                )
            })?;
        if let Some(kind) = stopped(&control) {
            return Err(failure(kind, no_write, Some(receipt), &progress, None));
        }
        let mut validating = ValidatingReader::new(
            body,
            length,
            request.budget.buffer_bytes,
            progress.clone(),
            control.clone(),
        );
        if length == 0 {
            let mut byte = [0u8; 1];
            let result = tokio::select! {
                biased;
                _ = control.cancellation().cancelled() => return Err(failure(TransferFailureKind::Cancelled, no_write, Some(receipt), &progress, None)),
                _ = tokio::time::sleep_until(deadline.into()) => return Err(failure(TransferFailureKind::Deadline, no_write, Some(receipt), &progress, None)),
                result = validating.read(&mut byte) => result,
            };
            if result.is_err() {
                let kind = progress
                    .lock()
                    .unwrap()
                    .failure
                    .unwrap_or(TransferFailureKind::SourceBody);
                return Err(failure(kind, no_write, Some(receipt), &progress, None));
            }
        }
        if let Some(kind) = stopped(&control) {
            return Err(failure(kind, no_write, Some(receipt), &progress, None));
        }
        let write = prepared.execute(Box::new(validating), control.clone());
        let result = tokio::select! {
            biased;
            _ = control.cancellation().cancelled() => return Err(failure(TransferFailureKind::Cancelled, CommitKnowledge::Unknown, Some(receipt), &progress, None)),
            _ = tokio::time::sleep_until(deadline.into()) => return Err(failure(TransferFailureKind::Deadline, CommitKnowledge::Unknown, Some(receipt), &progress, None)),
            result = write => result,
        };
        match result {
            Ok(destination) => {
                if let Some(kind) = stopped(&control) {
                    return Err(failure(
                        kind,
                        CommitKnowledge::Committed,
                        Some(receipt),
                        &progress,
                        None,
                    ));
                }
                let (valid, consumed, delivered, kind) = {
                    let state = progress.lock().unwrap();
                    (
                        state.validated && state.delivered == length,
                        state.consumed,
                        state.delivered,
                        state.failure,
                    )
                };
                if !valid || kind.is_some() {
                    return Err(failure(
                        kind.unwrap_or(TransferFailureKind::LengthMismatch),
                        CommitKnowledge::Committed,
                        Some(receipt),
                        &progress,
                        None,
                    ));
                }
                Ok(TransferReceipt {
                    source: receipt,
                    enforcement,
                    destination,
                    consumed,
                    delivered,
                    metadata: request.metadata,
                    mode: request.mode,
                })
            }
            Err(error) => {
                let reader_kind = progress.lock().unwrap().failure;
                let kind = stopped(&control)
                    .or(reader_kind)
                    .unwrap_or(TransferFailureKind::Destination);
                let (commit, cause) = error.into_parts();
                Err(failure(kind, commit, Some(receipt), &progress, Some(cause)))
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finite_budget_boundaries() {
        assert!(TransferBudget::new(0, 1, Duration::from_nanos(1)).is_ok());
        assert!(TransferBudget::new(u64::MAX - 1, 1048576, Duration::from_secs(1)).is_ok());
        for (payload, buffer, time) in [
            (u64::MAX, 1, Duration::from_secs(1)),
            (1, 0, Duration::from_secs(1)),
            (1, 1048577, Duration::from_secs(1)),
            (1, 1, Duration::ZERO),
            (1, 1, Duration::MAX),
        ] {
            assert!(TransferBudget::new(payload, buffer, time).is_err());
        }
    }
}
