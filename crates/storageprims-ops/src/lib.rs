//! Higher-level provider-agnostic operations composed from storageprims-core.

use std::collections::{BTreeSet, VecDeque};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use storageprims_core::{
    require_guarded_reads, Capability, GuardedReadProvider, GuardedReadSelection, InspectionKind,
    InspectionLimitDimension, ProviderKind, SourceReceipt, SourceSelector, StorageError,
    StorageOperation, StorageProvider,
};
use tokio::io::AsyncReadExt;
use tokio::time::{timeout_at, Instant};

const MAX_BUDGET_BYTES: u64 = 64 * 1024 * 1024;
const MAX_OUTPUT_BYTES: u64 = 8 * 1024 * 1024;
const MAX_PENDING_LINE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_HELPER_REQUESTS: u32 = 64;
const MAX_LINE_COUNT: usize = 10_000;
const MAX_CHUNK_SIZE: u64 = 1024 * 1024;
const MAX_PROBE_SIZE: u64 = 1024 * 1024;

mod duration_millis {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u64(value.as_millis().min(u128::from(u64::MAX)) as u64)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        u64::deserialize(deserializer).map(Duration::from_millis)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadStrategy {
    RangeBackward,
    RangeMidpoint,
    ForwardStream,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineEnding {
    Lf,
    CrLf,
    Cr,
    Mixed,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineResult {
    pub lines: Vec<String>,
    pub byte_offset_start: u64,
    pub byte_offset_end: u64,
    pub total_size: u64,
    pub strategy: ReadStrategy,
    pub line_ending: LineEnding,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineOptions {
    pub chunk_size: u64,
    pub probe_size: usize,
    pub force_stream: bool,
}

/// Explicit finite limits for bounded inspection operations.
///
/// Existing `LineOptions` remains source-compatible; callers that need to vary
/// total operation cost use this additive budget type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectionBudget {
    pub payload_bytes: u64,
    pub helper_requests: u32,
    pub output_bytes: u64,
    pub pending_line_bytes: u64,
    pub line_count: usize,
    pub chunk_size: u64,
    pub probe_size: u64,
    #[serde(with = "duration_millis")]
    pub wall_time: Duration,
}

impl Default for InspectionBudget {
    fn default() -> Self {
        Self {
            payload_bytes: 8 * 1024 * 1024,
            helper_requests: 16,
            output_bytes: 1024 * 1024,
            pending_line_bytes: 1024 * 1024,
            line_count: 256,
            chunk_size: 256 * 1024,
            probe_size: 32 * 1024,
            wall_time: Duration::from_secs(30),
        }
    }
}

/// Result of a selected first-N byte preview.
#[derive(Debug)]
pub struct BytePreview {
    pub bytes: Vec<u8>,
    /// Same-response source evidence for the returned prefix.
    pub receipt: SourceReceipt,
    pub consumed_payload_bytes: u64,
    pub helper_requests: u32,
}

struct InspectionLedger {
    provider: ProviderKind,
    operation: StorageOperation,
    budget: InspectionBudget,
    deadline: Instant,
    helper_requests: u32,
    payload_bytes: u64,
    output_bytes: u64,
}

impl InspectionLedger {
    fn new(
        provider: ProviderKind,
        operation: StorageOperation,
        budget: InspectionBudget,
    ) -> storageprims_core::Result<Self> {
        validate_budget(operation, &budget)?;
        let deadline = Instant::now()
            .checked_add(budget.wall_time)
            .ok_or_else(|| StorageError::InvalidArgument {
                operation: Some(operation),
                argument: "wall_time".to_string(),
                reason: "duration cannot be represented by the monotonic clock".to_string(),
            })?;
        Ok(Self {
            provider,
            operation,
            deadline,
            budget,
            helper_requests: 0,
            payload_bytes: 0,
            output_bytes: 0,
        })
    }

    fn deadline(&self) -> Instant {
        self.deadline
    }

    fn request(&mut self) -> storageprims_core::Result<()> {
        self.check_deadline()?;
        let next = self.helper_requests.checked_add(1).ok_or_else(|| {
            self.limit(
                InspectionLimitDimension::Requests,
                self.budget.helper_requests as u64,
                u64::MAX,
            )
        })?;
        if next > self.budget.helper_requests {
            return Err(self.limit(
                InspectionLimitDimension::Requests,
                self.budget.helper_requests as u64,
                next as u64,
            ));
        }
        self.helper_requests = next;
        Ok(())
    }

    fn admit_payload(&mut self, bytes: u64) -> storageprims_core::Result<()> {
        self.check_deadline()?;
        let next = self.payload_bytes.checked_add(bytes).ok_or_else(|| {
            self.limit(
                InspectionLimitDimension::PayloadBytes,
                self.budget.payload_bytes,
                u64::MAX,
            )
        })?;
        if next > self.budget.payload_bytes {
            return Err(self.limit(
                InspectionLimitDimension::PayloadBytes,
                self.budget.payload_bytes,
                next,
            ));
        }
        self.payload_bytes = next;
        Ok(())
    }

    fn release_payload(&mut self, bytes: u64) {
        self.payload_bytes = self.payload_bytes.saturating_sub(bytes);
    }

    fn admit_output(&mut self, bytes: u64) -> storageprims_core::Result<()> {
        let next = self.output_bytes.checked_add(bytes).ok_or_else(|| {
            self.limit(
                InspectionLimitDimension::OutputBytes,
                self.budget.output_bytes,
                u64::MAX,
            )
        })?;
        if next > self.budget.output_bytes {
            return Err(self.limit(
                InspectionLimitDimension::OutputBytes,
                self.budget.output_bytes,
                next,
            ));
        }
        self.output_bytes = next;
        Ok(())
    }

    fn check_deadline(&self) -> storageprims_core::Result<()> {
        if Instant::now() >= self.deadline {
            return Err(self.limit(
                InspectionLimitDimension::Deadline,
                self.budget.wall_time.as_millis().min(u128::from(u64::MAX)) as u64,
                self.budget.wall_time.as_millis().min(u128::from(u64::MAX)) as u64,
            ));
        }
        Ok(())
    }

    fn limit(
        &self,
        dimension: InspectionLimitDimension,
        configured_limit: u64,
        consumed: u64,
    ) -> StorageError {
        StorageError::Inspection {
            provider: Some(self.provider),
            operation: self.operation,
            kind: InspectionKind::LimitExceeded,
            limit_dimension: Some(dimension),
            configured_limit,
            consumed,
        }
    }
}

/// Read a selected prefix through the source-guarded range contract.
///
/// The returned receipt describes only the prefix. It does not claim a complete
/// object download when the requested prefix is fulfilled before EOF.
pub async fn preview_bytes(
    provider: &dyn StorageProvider,
    key: &str,
    length: u64,
    budget: InspectionBudget,
) -> storageprims_core::Result<BytePreview> {
    if length == 0 {
        return Err(StorageError::InvalidArgument {
            operation: Some(StorageOperation::PreviewBytes),
            argument: "length".to_string(),
            reason: "preview length must be greater than zero".to_string(),
        });
    }
    let mut ledger = InspectionLedger::new(
        provider.provider_kind(),
        StorageOperation::PreviewBytes,
        budget,
    )?;
    validate_preview_request(length, &ledger)?;
    let guarded = require_guarded_reads(provider, StorageOperation::PreviewBytes)?;
    let (selection, _) = observe_and_bind(guarded, key, &mut ledger).await?;
    // Current-object observation size cannot cap a preview of a selected
    // retained version. A selected HEAD establishes only selected-empty
    // success; non-empty reads request the caller's full bounded prefix.
    ledger.request()?;
    let selected_receipt = timeout_at(ledger.deadline(), guarded.guarded_head(selection.clone()))
        .await
        .map_err(|_| {
            ledger.limit(
                InspectionLimitDimension::Deadline,
                ledger
                    .budget
                    .wall_time
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64,
                ledger
                    .budget
                    .wall_time
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64,
            )
        })??;
    if selected_receipt.total_size == Some(0) {
        return Ok(BytePreview {
            bytes: Vec::new(),
            receipt: selected_receipt,
            consumed_payload_bytes: 0,
            helper_requests: ledger.helper_requests,
        });
    }
    preview_bytes_selected_with_ledger(guarded, selection, length, &mut ledger).await
}

async fn observe_and_bind(
    provider: &dyn GuardedReadProvider,
    key: &str,
    ledger: &mut InspectionLedger,
) -> storageprims_core::Result<(GuardedReadSelection, SourceReceipt)> {
    ledger.request()?;
    let observation = timeout_at(ledger.deadline(), provider.observe_source(key))
        .await
        .map_err(|_| {
            ledger.limit(
                InspectionLimitDimension::Deadline,
                ledger
                    .budget
                    .wall_time
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64,
                ledger
                    .budget
                    .wall_time
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64,
            )
        })??;
    let selector = if let Some(token) = observation.receipt.native_version.clone() {
        SourceSelector::NativeVersion { token }
    } else if let Some(token) = observation.receipt.validator.clone() {
        SourceSelector::ValidatorMatch { token }
    } else {
        return Err(StorageError::UnsupportedCapability {
            provider: ledger.provider,
            operation: ledger.operation,
            capability: Capability::GuardedRead,
        });
    };
    let selection = provider.bind_guarded_read(key, selector)?;
    Ok((selection, observation.receipt))
}

/// Read a prefix from an already-bound guarded selection without re-observing
/// or rebinding it. This is the composable selected-source seam for callers
/// coordinating multiple inspection windows.
pub async fn preview_bytes_selected(
    provider: &dyn GuardedReadProvider,
    provider_kind: ProviderKind,
    selection: GuardedReadSelection,
    length: u64,
    budget: InspectionBudget,
) -> storageprims_core::Result<BytePreview> {
    if length == 0 {
        return Err(StorageError::InvalidArgument {
            operation: Some(StorageOperation::PreviewBytes),
            argument: "length".to_string(),
            reason: "preview length must be greater than zero".to_string(),
        });
    }
    let mut ledger = InspectionLedger::new(provider_kind, StorageOperation::PreviewBytes, budget)?;
    validate_preview_request(length, &ledger)?;
    ledger.request()?;
    let receipt = timeout_at(ledger.deadline(), provider.guarded_head(selection.clone()))
        .await
        .map_err(|_| {
            ledger.limit(
                InspectionLimitDimension::Deadline,
                ledger
                    .budget
                    .wall_time
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64,
                ledger
                    .budget
                    .wall_time
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64,
            )
        })??;
    if receipt.total_size == Some(0) {
        return Ok(BytePreview {
            bytes: Vec::new(),
            receipt,
            consumed_payload_bytes: 0,
            helper_requests: ledger.helper_requests,
        });
    }
    preview_bytes_selected_with_ledger(provider, selection, length, &mut ledger).await
}

async fn preview_bytes_selected_with_ledger(
    provider: &dyn GuardedReadProvider,
    selection: GuardedReadSelection,
    length: u64,
    ledger: &mut InspectionLedger,
) -> storageprims_core::Result<BytePreview> {
    let (bytes, receipt) = guarded_range_bytes(provider, selection, 0, length, ledger).await?;
    ledger.admit_output(bytes.len() as u64)?;
    Ok(BytePreview {
        bytes,
        consumed_payload_bytes: receipt
            .returned_window
            .and_then(|window| window.checked_len())
            .expect("validated guarded range receipt has a returned length"),
        receipt,
        helper_requests: ledger.helper_requests,
    })
}

async fn guarded_range_bytes(
    provider: &dyn GuardedReadProvider,
    selection: GuardedReadSelection,
    offset: u64,
    length: u64,
    ledger: &mut InspectionLedger,
) -> storageprims_core::Result<(Vec<u8>, SourceReceipt)> {
    if length == 0 {
        return Ok((
            Vec::new(),
            SourceReceipt {
                path: selection.key().to_string(),
                native_version: None,
                validator: None,
                total_size: None,
                requested_window: None,
                returned_window: None,
            },
        ));
    }
    let reserved = length
        .checked_add(1)
        .ok_or_else(|| StorageError::InvalidArgument {
            operation: Some(ledger.operation),
            argument: "length".to_string(),
            reason: "range length cannot reserve an EOF proof byte".to_string(),
        })?;
    ledger.admit_payload(reserved)?;
    ledger.request()?;
    let response = timeout_at(
        ledger.deadline(),
        provider.guarded_get_range(storageprims_core::GuardedRangeRequest {
            selection,
            offset,
            length,
        }),
    )
    .await
    .map_err(|_| {
        ledger.limit(
            InspectionLimitDimension::Deadline,
            ledger
                .budget
                .wall_time
                .as_millis()
                .min(u128::from(u64::MAX)) as u64,
            ledger
                .budget
                .wall_time
                .as_millis()
                .min(u128::from(u64::MAX)) as u64,
        )
    })??;
    let returned_length =
        validate_guarded_range_receipt(&response.receipt, offset, length, ledger.operation)?;
    let capacity = usize::try_from(returned_length).map_err(|_| {
        ledger.limit(
            InspectionLimitDimension::PayloadBytes,
            ledger.budget.payload_bytes,
            returned_length,
        )
    })?;
    let mut reader = response.reader;
    let scratch_len = usize::try_from(ledger.budget.chunk_size.min(returned_length))
        .unwrap_or(0)
        .max(1);
    let mut scratch = vec![0_u8; scratch_len];
    let mut bytes = Vec::with_capacity(capacity);
    while bytes.len() < capacity {
        ledger.check_deadline()?;
        let allowed = (capacity - bytes.len()).min(scratch.len());
        let read = timeout_at(ledger.deadline(), reader.read(&mut scratch[..allowed]))
            .await
            .map_err(|_| {
                ledger.limit(
                    InspectionLimitDimension::Deadline,
                    ledger
                        .budget
                        .wall_time
                        .as_millis()
                        .min(u128::from(u64::MAX)) as u64,
                    ledger
                        .budget
                        .wall_time
                        .as_millis()
                        .min(u128::from(u64::MAX)) as u64,
                )
            })?
            .map_err(|source| StorageError::Io {
                operation: Some(ledger.operation),
                source,
            })?;
        if read == 0 {
            return Err(StorageError::Io {
                operation: Some(ledger.operation),
                source: std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "guarded range body ended before its validated window",
                ),
            });
        }
        bytes.extend_from_slice(&scratch[..read]);
    }
    let mut eof_probe = [0_u8; 1];
    let extra = timeout_at(ledger.deadline(), reader.read(&mut eof_probe))
        .await
        .map_err(|_| {
            ledger.limit(
                InspectionLimitDimension::Deadline,
                ledger
                    .budget
                    .wall_time
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64,
                ledger
                    .budget
                    .wall_time
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64,
            )
        })?
        .map_err(|source| StorageError::Io {
            operation: Some(ledger.operation),
            source,
        })?;
    if extra != 0 {
        return Err(invalid_guarded_range_response(
            ledger.operation,
            "guarded range body exceeds its returned window",
        ));
    }
    ledger.release_payload(reserved.saturating_sub(returned_length));
    Ok((bytes, response.receipt))
}

fn invalid_guarded_range_response(operation: StorageOperation, detail: &str) -> StorageError {
    StorageError::Io {
        operation: Some(operation),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, detail),
    }
}

fn validate_guarded_range_receipt(
    receipt: &SourceReceipt,
    offset: u64,
    length: u64,
    operation: StorageOperation,
) -> storageprims_core::Result<u64> {
    let requested_end = offset.checked_add(length - 1).ok_or_else(|| {
        invalid_guarded_range_response(operation, "guarded range request overflows")
    })?;
    let requested = receipt.requested_window.ok_or_else(|| {
        invalid_guarded_range_response(operation, "guarded range is missing its requested window")
    })?;
    if requested.start != offset || requested.end != requested_end {
        return Err(invalid_guarded_range_response(
            operation,
            "guarded range receipt does not match its request",
        ));
    }
    let returned = receipt.returned_window.ok_or_else(|| {
        invalid_guarded_range_response(operation, "guarded range is missing its returned window")
    })?;
    let expected_end = match receipt.total_size {
        Some(0) => {
            return Err(invalid_guarded_range_response(
                operation,
                "non-empty range has an empty total",
            ))
        }
        Some(total) if offset >= total => {
            return Err(invalid_guarded_range_response(
                operation,
                "range starts at or beyond total size",
            ));
        }
        Some(total) => requested_end.min(total - 1),
        None => requested_end,
    };
    if returned.start != offset || returned.end != expected_end {
        return Err(invalid_guarded_range_response(
            operation,
            "guarded range returned window does not prove the requested bytes",
        ));
    }
    returned.checked_len().ok_or_else(|| {
        invalid_guarded_range_response(operation, "guarded range returned window overflows")
    })
}

fn apply_line_output_budget(
    ledger: &mut InspectionLedger,
    lines: &[ParsedLine],
) -> storageprims_core::Result<()> {
    let output_bytes = lines
        .iter()
        .try_fold(0_u64, |total, line| {
            total.checked_add(line.text.len() as u64).ok_or(())
        })
        .map_err(|_| {
            ledger.limit(
                InspectionLimitDimension::OutputBytes,
                ledger.budget.output_bytes,
                u64::MAX,
            )
        })?;
    ledger.admit_output(output_bytes)
}

fn line_budget(
    operation: StorageOperation,
    n: usize,
    options: &LineOptions,
) -> storageprims_core::Result<InspectionBudget> {
    let budget = InspectionBudget {
        line_count: n,
        chunk_size: options.chunk_size,
        probe_size: options.probe_size as u64,
        ..InspectionBudget::default()
    };
    validate_budget(operation, &budget)?;
    Ok(budget)
}

fn validate_preview_request(
    length: u64,
    ledger: &InspectionLedger,
) -> storageprims_core::Result<()> {
    if length > ledger.budget.payload_bytes {
        return Err(ledger.limit(
            InspectionLimitDimension::PayloadBytes,
            ledger.budget.payload_bytes,
            length,
        ));
    }
    if length > ledger.budget.output_bytes {
        return Err(ledger.limit(
            InspectionLimitDimension::OutputBytes,
            ledger.budget.output_bytes,
            length,
        ));
    }
    Ok(())
}

async fn selected_size(
    provider: &dyn GuardedReadProvider,
    selection: GuardedReadSelection,
    observation: SourceReceipt,
    ledger: &mut InspectionLedger,
) -> storageprims_core::Result<u64> {
    let _ = observation;
    // Current-object observation does not establish the size of a bound
    // version. Every line window is sized from this selected response instead.
    ledger.request()?;
    let receipt = timeout_at(ledger.deadline(), provider.guarded_head(selection))
        .await
        .map_err(|_| {
            ledger.limit(
                InspectionLimitDimension::Deadline,
                ledger
                    .budget
                    .wall_time
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64,
                ledger
                    .budget
                    .wall_time
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64,
            )
        })??;
    receipt.total_size.ok_or_else(|| StorageError::Io {
        operation: Some(ledger.operation),
        source: std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "guarded source response is missing total size",
        ),
    })
}

async fn line_probe(
    provider: &dyn GuardedReadProvider,
    selection: GuardedReadSelection,
    size: u64,
    ledger: &mut InspectionLedger,
) -> storageprims_core::Result<(Vec<u8>, SourceReceipt)> {
    let length = size.min(ledger.budget.probe_size);
    let (probe, receipt) = guarded_range_bytes(provider, selection, 0, length, ledger).await?;
    reject_binary_prefix(&probe, ledger.provider, ledger.operation)?;
    Ok((probe, receipt))
}

fn reject_binary_prefix(
    bytes: &[u8],
    provider: ProviderKind,
    operation: StorageOperation,
) -> storageprims_core::Result<()> {
    if starts_with_utf16_bom(bytes) || bytes.contains(&0) {
        return Err(line_ops_error(
            provider,
            operation,
            LineOpsError::BinaryContent,
        ));
    }
    let bytes = strip_utf8_bom(bytes);
    if let Err(error) = std::str::from_utf8(bytes) {
        // A split code point at a bounded range edge is framing carry-over, not
        // evidence of binary content. Complete lines still undergo strict UTF-8
        // validation in the line parser.
        if error.error_len().is_some() {
            return Err(line_ops_error(
                provider,
                operation,
                LineOpsError::BinaryContent,
            ));
        }
    }
    Ok(())
}

impl Default for LineOptions {
    fn default() -> Self {
        Self {
            chunk_size: 256 * 1024,
            probe_size: 32 * 1024,
            force_stream: false,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LineOpsError {
    #[error("compressed object detected; use force_stream to allow expensive forward scan")]
    CompressedObject,
    #[error("no line terminator found in probe window of {probe_size} bytes")]
    NoLineTerminatorInProbe { probe_size: usize },
    #[error("binary content detected; line-oriented operations require UTF-8 text")]
    BinaryContent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedLine {
    text: String,
    start: u64,
    end: u64,
    ending: Option<LineEnding>,
}

pub async fn head_lines(
    provider: &dyn StorageProvider,
    key: &str,
    n: usize,
) -> storageprims_core::Result<LineResult> {
    head_lines_with_options(provider, key, n, LineOptions::default()).await
}

pub async fn head_lines_with_options(
    provider: &dyn StorageProvider,
    key: &str,
    n: usize,
    options: LineOptions,
) -> storageprims_core::Result<LineResult> {
    let budget = line_budget(StorageOperation::HeadLines, n, &options)?;
    let mut ledger = InspectionLedger::new(
        provider.provider_kind(),
        StorageOperation::HeadLines,
        budget,
    )?;
    let guarded = require_guarded_reads(provider, StorageOperation::HeadLines)?;
    let (selection, observation) = observe_and_bind(guarded, key, &mut ledger).await?;
    let size = selected_size(guarded, selection.clone(), observation, &mut ledger).await?;
    if size == 0 {
        return Ok(empty_result(ReadStrategy::ForwardStream, 0));
    }
    let (probe, probe_receipt) = line_probe(guarded, selection.clone(), size, &mut ledger).await?;
    let probe_eof = probe_receipt.total_size
        == probe_receipt
            .returned_window
            .map(|window| window.end.saturating_add(1));
    let probe_lines = parse_lines_up_to(
        &probe,
        0,
        probe_eof,
        n,
        provider.provider_kind(),
        StorageOperation::HeadLines,
    )?;
    if probe_lines.len() == n || probe_eof {
        apply_line_output_budget(&mut ledger, &probe_lines)?;
        return Ok(build_line_result(
            probe_lines,
            size,
            ReadStrategy::ForwardStream,
        ));
    }
    let length = size
        .min(ledger.budget.pending_line_bytes)
        .min(ledger.budget.payload_bytes);
    let (bytes, receipt) = guarded_range_bytes(guarded, selection, 0, length, &mut ledger).await?;
    reject_binary_prefix(
        &bytes,
        provider.provider_kind(),
        StorageOperation::HeadLines,
    )?;
    let eof = receipt.total_size
        == receipt
            .returned_window
            .map(|window| window.end.saturating_add(1));
    let selected = parse_lines_up_to(
        &bytes,
        0,
        eof,
        n,
        provider.provider_kind(),
        StorageOperation::HeadLines,
    )?;
    if selected.len() < n && !eof {
        return Err(ledger.limit(
            InspectionLimitDimension::PendingLineBytes,
            ledger.budget.pending_line_bytes,
            length,
        ));
    }
    apply_line_output_budget(&mut ledger, &selected)?;
    Ok(build_line_result(
        selected,
        size,
        ReadStrategy::ForwardStream,
    ))
}

pub async fn tail_lines(
    provider: &dyn StorageProvider,
    key: &str,
    n: usize,
) -> storageprims_core::Result<LineResult> {
    tail_lines_with_options(provider, key, n, LineOptions::default()).await
}

pub async fn tail_lines_with_options(
    provider: &dyn StorageProvider,
    key: &str,
    n: usize,
    options: LineOptions,
) -> storageprims_core::Result<LineResult> {
    if is_compressed_key(key) && !options.force_stream {
        return Err(line_ops_error(
            provider.provider_kind(),
            StorageOperation::TailLines,
            LineOpsError::CompressedObject,
        ));
    }

    let budget = line_budget(StorageOperation::TailLines, n, &options)?;
    let mut ledger = InspectionLedger::new(
        provider.provider_kind(),
        StorageOperation::TailLines,
        budget,
    )?;
    let guarded = require_guarded_reads(provider, StorageOperation::TailLines)?;
    let (selection, observation) = observe_and_bind(guarded, key, &mut ledger).await?;
    let size = selected_size(guarded, selection.clone(), observation, &mut ledger).await?;
    if size == 0 {
        return Ok(empty_result(ReadStrategy::RangeBackward, 0));
    }
    let _ = line_probe(guarded, selection.clone(), size, &mut ledger).await?;
    let length = size
        .min(ledger.budget.pending_line_bytes)
        .min(ledger.budget.payload_bytes);
    let start = if options.force_stream {
        0
    } else {
        size - length
    };
    let previous_byte = if start == 0 {
        None
    } else {
        let (previous, _) =
            guarded_range_bytes(guarded, selection.clone(), start - 1, 1, &mut ledger).await?;
        previous.first().copied()
    };
    let (bytes, receipt) =
        guarded_range_bytes(guarded, selection, start, length, &mut ledger).await?;
    let aligned = if start == 0 {
        0
    } else {
        first_full_line_start(
            &bytes,
            slice_starts_on_line_boundary(previous_byte, bytes.first().copied()),
        )
        .unwrap_or(bytes.len())
    };
    reject_binary_prefix(
        &bytes[aligned..],
        provider.provider_kind(),
        StorageOperation::TailLines,
    )?;
    let eof = receipt.total_size
        == receipt
            .returned_window
            .map(|window| window.end.saturating_add(1));
    if options.force_stream && !eof {
        return Err(ledger.limit(
            InspectionLimitDimension::PendingLineBytes,
            ledger.budget.pending_line_bytes,
            length,
        ));
    }
    let selected = parse_tail_lines_bounded(
        &bytes[aligned..],
        start + aligned as u64,
        eof,
        n,
        provider.provider_kind(),
        StorageOperation::TailLines,
    )?;
    if selected.len() < n && start != 0 {
        return Err(ledger.limit(
            InspectionLimitDimension::PendingLineBytes,
            ledger.budget.pending_line_bytes,
            length,
        ));
    }
    apply_line_output_budget(&mut ledger, &selected)?;
    Ok(build_line_result(
        selected,
        size,
        if options.force_stream {
            ReadStrategy::ForwardStream
        } else {
            ReadStrategy::RangeBackward
        },
    ))
}

pub async fn mid_lines(
    provider: &dyn StorageProvider,
    key: &str,
    n: usize,
) -> storageprims_core::Result<LineResult> {
    mid_lines_with_options(provider, key, n, LineOptions::default()).await
}

pub async fn mid_lines_with_options(
    provider: &dyn StorageProvider,
    key: &str,
    n: usize,
    options: LineOptions,
) -> storageprims_core::Result<LineResult> {
    if is_compressed_key(key) && !options.force_stream {
        return Err(line_ops_error(
            provider.provider_kind(),
            StorageOperation::MidLines,
            LineOpsError::CompressedObject,
        ));
    }

    let budget = line_budget(StorageOperation::MidLines, n, &options)?;
    let mut ledger =
        InspectionLedger::new(provider.provider_kind(), StorageOperation::MidLines, budget)?;
    let guarded = require_guarded_reads(provider, StorageOperation::MidLines)?;
    let (selection, observation) = observe_and_bind(guarded, key, &mut ledger).await?;
    let size = selected_size(guarded, selection.clone(), observation, &mut ledger).await?;
    if size == 0 {
        return Ok(empty_result(ReadStrategy::RangeMidpoint, 0));
    }
    let _ = line_probe(guarded, selection.clone(), size, &mut ledger).await?;
    let length = size
        .min(ledger.budget.pending_line_bytes)
        .min(ledger.budget.payload_bytes);
    let midpoint = size / 2;
    let start = if options.force_stream {
        0
    } else {
        midpoint.saturating_sub(length / 2)
    };
    let requested = if options.force_stream {
        length
    } else {
        length.min(size - start)
    };
    let (bytes, receipt) =
        guarded_range_bytes(guarded, selection, start, requested, &mut ledger).await?;
    let eof = receipt.total_size
        == receipt
            .returned_window
            .map(|window| window.end.saturating_add(1));
    if options.force_stream && !eof {
        return Err(ledger.limit(
            InspectionLimitDimension::PendingLineBytes,
            ledger.budget.pending_line_bytes,
            requested,
        ));
    }
    let aligned = if options.force_stream {
        midpoint_alignment(&bytes, midpoint).unwrap_or({
            if eof && start == 0 {
                0
            } else {
                bytes.len()
            }
        })
    } else {
        midpoint_alignment(&bytes, midpoint - start).unwrap_or({
            if eof && start == 0 {
                0
            } else {
                bytes.len()
            }
        })
    };
    reject_binary_prefix(
        &bytes[aligned..],
        provider.provider_kind(),
        StorageOperation::MidLines,
    )?;
    let selected = parse_lines_up_to(
        &bytes[aligned..],
        start + aligned as u64,
        eof,
        n,
        provider.provider_kind(),
        StorageOperation::MidLines,
    )?;
    if selected.len() < n && !eof {
        return Err(ledger.limit(
            InspectionLimitDimension::PendingLineBytes,
            ledger.budget.pending_line_bytes,
            requested,
        ));
    }
    apply_line_output_budget(&mut ledger, &selected)?;
    Ok(build_line_result(
        selected,
        size,
        if options.force_stream {
            ReadStrategy::ForwardStream
        } else {
            ReadStrategy::RangeMidpoint
        },
    ))
}

pub async fn count_lines(
    provider: &dyn StorageProvider,
    key: &str,
) -> storageprims_core::Result<u64> {
    let budget = InspectionBudget::default();
    let mut ledger = InspectionLedger::new(
        provider.provider_kind(),
        StorageOperation::CountLines,
        budget,
    )?;
    let guarded = require_guarded_reads(provider, StorageOperation::CountLines)?;
    let (selection, observation) = observe_and_bind(guarded, key, &mut ledger).await?;
    let size = selected_size(guarded, selection.clone(), observation, &mut ledger).await?;
    if size == 0 {
        return Ok(0);
    }
    if size > ledger.budget.payload_bytes {
        return Err(ledger.limit(
            InspectionLimitDimension::PayloadBytes,
            ledger.budget.payload_bytes,
            ledger.payload_bytes,
        ));
    }
    if size > ledger.budget.pending_line_bytes {
        return Err(ledger.limit(
            InspectionLimitDimension::PendingLineBytes,
            ledger.budget.pending_line_bytes,
            ledger.payload_bytes,
        ));
    }
    let _ = line_probe(guarded, selection.clone(), size, &mut ledger).await?;
    let (bytes, receipt) = guarded_range_bytes(guarded, selection, 0, size, &mut ledger).await?;
    if receipt.total_size != Some(size)
        || receipt
            .returned_window
            .map(|window| window.end.saturating_add(1))
            != Some(size)
    {
        return Err(ledger.limit(
            InspectionLimitDimension::PayloadBytes,
            ledger.budget.payload_bytes,
            size,
        ));
    }
    reject_binary_prefix(
        &bytes,
        provider.provider_kind(),
        StorageOperation::CountLines,
    )?;
    count_lines_bounded(
        &bytes,
        provider.provider_kind(),
        StorageOperation::CountLines,
    )
}

fn validate_budget(
    operation: StorageOperation,
    budget: &InspectionBudget,
) -> storageprims_core::Result<()> {
    let invalid = |argument: &str, reason: &str| StorageError::InvalidArgument {
        operation: Some(operation),
        argument: argument.to_string(),
        reason: reason.to_string(),
    };
    if budget.payload_bytes == 0 || budget.payload_bytes > MAX_BUDGET_BYTES {
        return Err(invalid(
            "payload_bytes",
            "must be within the finite inspection cap",
        ));
    }
    if budget.helper_requests == 0 || budget.helper_requests > MAX_HELPER_REQUESTS {
        return Err(invalid(
            "helper_requests",
            "must be within the finite inspection cap",
        ));
    }
    if budget.output_bytes == 0 || budget.output_bytes > MAX_OUTPUT_BYTES {
        return Err(invalid(
            "output_bytes",
            "must be within the finite inspection cap",
        ));
    }
    if budget.pending_line_bytes == 0 || budget.pending_line_bytes > MAX_PENDING_LINE_BYTES {
        return Err(invalid(
            "pending_line_bytes",
            "must be within the finite inspection cap",
        ));
    }
    if budget.line_count == 0 || budget.line_count > MAX_LINE_COUNT {
        return Err(invalid(
            "line_count",
            "must be within the finite inspection cap",
        ));
    }
    if budget.chunk_size == 0 || budget.chunk_size > MAX_CHUNK_SIZE {
        return Err(invalid(
            "chunk_size",
            "must be within the finite inspection cap",
        ));
    }
    if budget.probe_size == 0 || budget.probe_size > MAX_PROBE_SIZE {
        return Err(invalid(
            "probe_size",
            "must be within the finite inspection cap",
        ));
    }
    if budget.wall_time.is_zero() {
        return Err(invalid("wall_time", "must be a positive finite duration"));
    }
    Ok(())
}

fn empty_result(strategy: ReadStrategy, total_size: u64) -> LineResult {
    LineResult {
        lines: Vec::new(),
        byte_offset_start: 0,
        byte_offset_end: 0,
        total_size,
        strategy,
        line_ending: LineEnding::Unknown,
    }
}

fn line_ops_error(
    provider: ProviderKind,
    operation: StorageOperation,
    error: LineOpsError,
) -> StorageError {
    match error {
        LineOpsError::NoLineTerminatorInProbe { probe_size } => StorageError::Inspection {
            provider: Some(provider),
            operation,
            kind: InspectionKind::LimitExceeded,
            limit_dimension: Some(InspectionLimitDimension::ProbeSize),
            configured_limit: probe_size as u64,
            consumed: probe_size as u64,
        },
        LineOpsError::CompressedObject | LineOpsError::BinaryContent => StorageError::Inspection {
            provider: Some(provider),
            operation,
            kind: InspectionKind::EncodingRejected,
            limit_dimension: None,
            configured_limit: 0,
            consumed: 0,
        },
    }
}

fn build_line_result(
    lines: Vec<ParsedLine>,
    total_size: u64,
    strategy: ReadStrategy,
) -> LineResult {
    if lines.is_empty() {
        return empty_result(strategy, total_size);
    }

    let byte_offset_start = lines.first().expect("not empty").start;
    let byte_offset_end = lines.last().expect("not empty").end;
    let line_ending = aggregate_line_endings(lines.iter().filter_map(|line| line.ending));
    let lines = lines.into_iter().map(|line| line.text).collect();

    LineResult {
        lines,
        byte_offset_start,
        byte_offset_end,
        total_size,
        strategy,
        line_ending,
    }
}

fn aggregate_line_endings(endings: impl IntoIterator<Item = LineEnding>) -> LineEnding {
    let endings = endings.into_iter().collect::<BTreeSet<_>>();
    if endings.is_empty() {
        LineEnding::Unknown
    } else if endings.len() == 1 {
        *endings.iter().next().expect("set is not empty")
    } else {
        LineEnding::Mixed
    }
}

fn first_full_line_start(bytes: &[u8], boundary_before_start: bool) -> Option<usize> {
    if boundary_before_start {
        return Some(0);
    }

    next_line_boundary(bytes, 0)
}

fn slice_starts_on_line_boundary(previous_byte: Option<u8>, first_byte: Option<u8>) -> bool {
    match previous_byte {
        None | Some(b'\n') => true,
        Some(b'\r') if first_byte != Some(b'\n') => true,
        _ => false,
    }
}

fn midpoint_alignment(bytes: &[u8], midpoint_in_slice: u64) -> Option<usize> {
    let midpoint = usize::try_from(midpoint_in_slice).ok()?;
    if midpoint >= bytes.len() {
        return None;
    }

    match bytes[midpoint] {
        b'\n' => Some(midpoint + 1),
        b'\r' => {
            if bytes.get(midpoint + 1) == Some(&b'\n') {
                Some(midpoint + 2)
            } else {
                Some(midpoint + 1)
            }
        }
        _ => next_line_boundary(bytes, midpoint),
    }
}

fn next_line_boundary(bytes: &[u8], start: usize) -> Option<usize> {
    let mut index = start;
    while index < bytes.len() {
        match bytes[index] {
            b'\n' => return Some(index + 1),
            b'\r' => {
                if bytes.get(index + 1) == Some(&b'\n') {
                    return Some(index + 2);
                }
                return Some(index + 1);
            }
            _ => index += 1,
        }
    }
    None
}

#[cfg(test)]
fn parse_lines(
    bytes: &[u8],
    base_offset: u64,
    include_trailing: bool,
    provider: ProviderKind,
    operation: StorageOperation,
) -> storageprims_core::Result<Vec<ParsedLine>> {
    let mut lines = Vec::new();
    let mut cursor = 0;

    while cursor < bytes.len() {
        if let Some((line, consumed)) = extract_first_complete_line(
            &bytes[cursor..],
            base_offset + cursor as u64,
            provider,
            operation,
            false,
        )? {
            cursor += consumed;
            lines.push(line);
        } else {
            break;
        }
    }

    if include_trailing && cursor < bytes.len() {
        if let Some((line, consumed)) = extract_first_complete_line(
            &bytes[cursor..],
            base_offset + cursor as u64,
            provider,
            operation,
            true,
        )? {
            cursor += consumed;
            lines.push(line);
        }
    }

    if include_trailing && cursor < bytes.len() {
        let text = std::str::from_utf8(&bytes[cursor..])
            .map_err(|_| line_ops_error(provider, operation, LineOpsError::BinaryContent))?
            .to_string();
        lines.push(ParsedLine {
            text,
            start: base_offset + cursor as u64,
            end: base_offset + bytes.len() as u64,
            ending: None,
        });
    }

    Ok(lines)
}

fn parse_lines_up_to(
    bytes: &[u8],
    base_offset: u64,
    include_trailing: bool,
    maximum: usize,
    provider: ProviderKind,
    operation: StorageOperation,
) -> storageprims_core::Result<Vec<ParsedLine>> {
    let mut lines = Vec::with_capacity(maximum.min(64));
    let mut cursor = 0;
    while cursor < bytes.len() && lines.len() < maximum {
        let Some((line, consumed)) = extract_first_complete_line(
            &bytes[cursor..],
            base_offset + cursor as u64,
            provider,
            operation,
            false,
        )?
        else {
            break;
        };
        cursor += consumed;
        lines.push(line);
    }
    if include_trailing && lines.len() < maximum && cursor < bytes.len() {
        if let Some((line, consumed)) = extract_first_complete_line(
            &bytes[cursor..],
            base_offset + cursor as u64,
            provider,
            operation,
            true,
        )? {
            cursor += consumed;
            lines.push(line);
        }
    }
    if include_trailing && lines.len() < maximum && cursor < bytes.len() {
        let text = std::str::from_utf8(&bytes[cursor..])
            .map_err(|_| line_ops_error(provider, operation, LineOpsError::BinaryContent))?
            .to_string();
        lines.push(ParsedLine {
            text,
            start: base_offset + cursor as u64,
            end: base_offset + bytes.len() as u64,
            ending: None,
        });
    }
    Ok(lines)
}

fn parse_tail_lines_bounded(
    bytes: &[u8],
    base_offset: u64,
    include_trailing: bool,
    maximum: usize,
    provider: ProviderKind,
    operation: StorageOperation,
) -> storageprims_core::Result<Vec<ParsedLine>> {
    let mut lines = VecDeque::with_capacity(maximum.min(64));
    let mut cursor = 0;
    while cursor < bytes.len() {
        let Some((line, consumed)) = extract_first_complete_line(
            &bytes[cursor..],
            base_offset + cursor as u64,
            provider,
            operation,
            false,
        )?
        else {
            break;
        };
        cursor += consumed;
        if lines.len() == maximum {
            lines.pop_front();
        }
        lines.push_back(line);
    }
    if include_trailing && cursor < bytes.len() {
        if let Some((line, consumed)) = extract_first_complete_line(
            &bytes[cursor..],
            base_offset + cursor as u64,
            provider,
            operation,
            true,
        )? {
            cursor += consumed;
            if lines.len() == maximum {
                lines.pop_front();
            }
            lines.push_back(line);
        }
    }
    if include_trailing && cursor < bytes.len() {
        let text = std::str::from_utf8(&bytes[cursor..])
            .map_err(|_| line_ops_error(provider, operation, LineOpsError::BinaryContent))?
            .to_string();
        if lines.len() == maximum {
            lines.pop_front();
        }
        lines.push_back(ParsedLine {
            text,
            start: base_offset + cursor as u64,
            end: base_offset + bytes.len() as u64,
            ending: None,
        });
    }
    Ok(lines.into_iter().collect())
}

fn count_lines_bounded(
    bytes: &[u8],
    provider: ProviderKind,
    operation: StorageOperation,
) -> storageprims_core::Result<u64> {
    let mut cursor = 0;
    let mut count = 0_u64;
    while cursor < bytes.len() {
        let Some(consumed) = complete_line_consumed(&bytes[cursor..], false, provider, operation)?
        else {
            break;
        };
        cursor += consumed;
        count = count
            .checked_add(1)
            .ok_or_else(|| StorageError::InvalidArgument {
                operation: Some(operation),
                argument: "line count".to_string(),
                reason: "line count overflows u64".to_string(),
            })?;
    }
    if cursor < bytes.len() {
        std::str::from_utf8(&bytes[cursor..])
            .map_err(|_| line_ops_error(provider, operation, LineOpsError::BinaryContent))?;
        count = count
            .checked_add(1)
            .ok_or_else(|| StorageError::InvalidArgument {
                operation: Some(operation),
                argument: "line count".to_string(),
                reason: "line count overflows u64".to_string(),
            })?;
    }
    Ok(count)
}

fn complete_line_consumed(
    bytes: &[u8],
    eof: bool,
    provider: ProviderKind,
    operation: StorageOperation,
) -> storageprims_core::Result<Option<usize>> {
    for (index, byte) in bytes.iter().enumerate() {
        let consumed = match byte {
            b'\n' => index + 1,
            b'\r' if index + 1 < bytes.len() && bytes[index + 1] == b'\n' => index + 2,
            b'\r' if index + 1 < bytes.len() || eof => index + 1,
            b'\r' => return Ok(None),
            _ => continue,
        };
        std::str::from_utf8(&bytes[..index])
            .map_err(|_| line_ops_error(provider, operation, LineOpsError::BinaryContent))?;
        return Ok(Some(consumed));
    }
    Ok(None)
}

fn extract_first_complete_line(
    bytes: &[u8],
    base_offset: u64,
    provider: ProviderKind,
    operation: StorageOperation,
    eof: bool,
) -> storageprims_core::Result<Option<(ParsedLine, usize)>> {
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\n' => {
                let text = std::str::from_utf8(&bytes[..index])
                    .map_err(|_| line_ops_error(provider, operation, LineOpsError::BinaryContent))?
                    .to_string();
                return Ok(Some((
                    ParsedLine {
                        text,
                        start: base_offset,
                        end: base_offset + index as u64 + 1,
                        ending: Some(LineEnding::Lf),
                    },
                    index + 1,
                )));
            }
            b'\r' => {
                if index + 1 == bytes.len() && !eof {
                    return Ok(None);
                }
                let (ending, consumed) = if bytes.get(index + 1) == Some(&b'\n') {
                    (LineEnding::CrLf, 2)
                } else {
                    (LineEnding::Cr, 1)
                };
                let text = std::str::from_utf8(&bytes[..index])
                    .map_err(|_| line_ops_error(provider, operation, LineOpsError::BinaryContent))?
                    .to_string();
                return Ok(Some((
                    ParsedLine {
                        text,
                        start: base_offset,
                        end: base_offset + index as u64 + consumed as u64,
                        ending: Some(ending),
                    },
                    index + consumed,
                )));
            }
            _ => index += 1,
        }
    }

    Ok(None)
}

fn starts_with_utf16_bom(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF])
}

fn strip_utf8_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes)
}

fn is_compressed_key(key: &str) -> bool {
    [".gz", ".zst", ".bz2", ".xz", ".lz4", ".br"]
        .iter()
        .any(|suffix| key.ends_with(suffix))
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, VecDeque};
    use std::sync::{Arc, Mutex};

    use bytes::Bytes;
    use storageprims_core::{
        BoxFuture, BoxedByteStream, ByteWindow, Capability, CopyRequest, CopyResult, CopyStrategy,
        CredentialSource, GetRangeRequest, GuardedRangeRequest, GuardedReadProvider,
        GuardedReadResponse, GuardedReadSelection, ListOptions, ListResult, ObjectMetadata,
        ProviderConfig, ProviderKind, PutOptions, PutResult, SourceObservation, SourceReceipt,
        StorageError, StorageProvider, TargetConfig,
    };
    use storageprims_s3::S3Provider;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio_util::io::StreamReader;

    use super::*;

    #[derive(Default, Clone)]
    struct MockProvider {
        key: String,
        bytes: Arc<Vec<u8>>,
        chunk_size: usize,
        reject_bound_range: bool,
        range_fault: Option<RangeFault>,
        head_calls: Arc<Mutex<u64>>,
        range_calls: Arc<Mutex<Vec<GetRangeRequest>>>,
        guarded_range_calls: Arc<Mutex<u64>>,
    }

    #[derive(Clone, Copy)]
    enum RangeFault {
        WrongOffset,
        OversizedWindow,
        IgnoredRange,
        OverlongBody,
    }

    impl MockProvider {
        fn new(key: &str, contents: &str) -> Self {
            Self {
                key: key.to_string(),
                bytes: Arc::new(contents.as_bytes().to_vec()),
                chunk_size: 4,
                ..Self::default()
            }
        }

        fn with_bytes(key: &str, bytes: Vec<u8>) -> Self {
            Self {
                key: key.to_string(),
                bytes: Arc::new(bytes),
                chunk_size: 4,
                ..Self::default()
            }
        }

        fn stream_from_bytes(bytes: &[u8], chunk_size: usize) -> BoxedByteStream {
            let chunks = bytes
                .chunks(chunk_size.max(1))
                .map(|chunk| Ok::<_, std::io::Error>(Bytes::copy_from_slice(chunk)))
                .collect::<VecDeque<_>>();
            Box::new(StreamReader::new(tokio_stream::iter(chunks)))
        }
    }

    async fn scripted_responses_server(
        responses: &'static [&'static str],
    ) -> (String, tokio::task::JoinHandle<Vec<Vec<u8>>>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("scripted server binds");
        let address = listener.local_addr().expect("scripted server has address");
        let task = tokio::spawn(async move {
            let mut requests = Vec::with_capacity(responses.len());
            for response in responses {
                let (mut socket, _) = listener.accept().await.expect("server accepts request");
                let mut request = Vec::new();
                let mut buffer = [0_u8; 2048];
                loop {
                    let read = socket.read(&mut buffer).await.expect("request reads");
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                socket
                    .write_all(response.as_bytes())
                    .await
                    .expect("response writes");
                requests.push(request);
            }
            requests
        });
        (format!("http://{address}"), task)
    }

    async fn scripted_s3_provider(endpoint: String) -> S3Provider {
        let mut credentials = BTreeMap::new();
        credentials.insert("access_key".to_string(), "test-access".to_string());
        credentials.insert("secret_key".to_string(), "test-secret".to_string());
        S3Provider::from_config(ProviderConfig {
            provider: ProviderKind::S3,
            target: TargetConfig {
                container: Some("bucket".to_string()),
                region: Some("us-east-1".to_string()),
                endpoint: Some(endpoint),
                force_path_style: Some(true),
                ..TargetConfig::default()
            },
            credentials: CredentialSource::InlineStatic {
                values: credentials,
            },
        })
        .await
        .expect("scripted provider config is valid")
    }

    impl StorageProvider for MockProvider {
        fn provider_kind(&self) -> ProviderKind {
            ProviderKind::Local
        }

        fn capabilities(&self) -> Vec<storageprims_core::Capability> {
            vec![Capability::GuardedRead]
        }

        fn guarded_reads(&self) -> Option<&dyn GuardedReadProvider> {
            Some(self)
        }

        fn list(&self, _options: ListOptions) -> BoxFuture<'_, ListResult> {
            Box::pin(async {
                Ok(ListResult {
                    objects: Vec::new(),
                    continuation_token: None,
                    is_truncated: false,
                })
            })
        }

        fn head(&self, key: &str) -> BoxFuture<'_, ObjectMetadata> {
            let key = key.to_string();
            let bytes = self.bytes.clone();
            let head_calls = self.head_calls.clone();
            Box::pin(async move {
                *head_calls.lock().expect("head mutex poisoned") += 1;
                Ok(ObjectMetadata {
                    path: key,
                    size: bytes.len() as u64,
                    etag: None,
                    content_type: None,
                    last_modified: None,
                    metadata: BTreeMap::new(),
                })
            })
        }

        fn get(&self, key: &str) -> BoxFuture<'_, BoxedByteStream> {
            let bytes = self.bytes.clone();
            let chunk_size = self.chunk_size;
            let expected = self.key.clone();
            let key = key.to_string();
            Box::pin(async move {
                if key != expected {
                    return Err(StorageError::NotFound {
                        provider: ProviderKind::Local,
                        operation: StorageOperation::Get,
                        path: key,
                    });
                }
                Ok(Self::stream_from_bytes(bytes.as_slice(), chunk_size))
            })
        }

        fn get_range(&self, request: GetRangeRequest) -> BoxFuture<'_, BoxedByteStream> {
            let bytes = self.bytes.clone();
            let expected = self.key.clone();
            let range_calls = self.range_calls.clone();
            Box::pin(async move {
                range_calls
                    .lock()
                    .expect("range mutex poisoned")
                    .push(request.clone());
                if request.key != expected {
                    return Err(StorageError::NotFound {
                        provider: ProviderKind::Local,
                        operation: StorageOperation::GetRange,
                        path: request.key,
                    });
                }
                let start = usize::try_from(request.offset).unwrap_or(usize::MAX);
                let end = usize::try_from(request.offset.saturating_add(request.length))
                    .unwrap_or(usize::MAX);
                if start > bytes.len() {
                    return Err(StorageError::InvalidArgument {
                        operation: Some(StorageOperation::GetRange),
                        argument: "offset".to_string(),
                        reason: "out of range".to_string(),
                    });
                }
                let slice_end = end.min(bytes.len());
                Ok(Self::stream_from_bytes(
                    &bytes[start..slice_end],
                    self.chunk_size,
                ))
            })
        }

        fn put(
            &self,
            key: &str,
            _body: BoxedByteStream,
            options: PutOptions,
        ) -> BoxFuture<'_, PutResult> {
            let key = key.to_string();
            Box::pin(async move {
                if !options.precondition.is_none() {
                    return Err(StorageError::UnsupportedCapability {
                        provider: ProviderKind::Local,
                        operation: StorageOperation::Put,
                        capability: storageprims_core::Capability::ConditionalPut,
                    });
                }
                Ok(PutResult {
                    path: key,
                    etag: None,
                    size: options.content_length,
                })
            })
        }

        fn delete(&self, _key: &str) -> BoxFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }

        fn copy(&self, request: CopyRequest) -> BoxFuture<'_, CopyResult> {
            Box::pin(async move {
                Ok(CopyResult {
                    source: request.source,
                    destination: request.destination,
                    strategy: CopyStrategy::Native,
                    bytes_copied: None,
                    destination_etag: None,
                })
            })
        }
    }

    impl GuardedReadProvider for MockProvider {
        fn guarded_read_target_identity(&self) -> String {
            "mock-target".to_string()
        }

        fn observe_source(&self, key: &str) -> BoxFuture<'_, SourceObservation> {
            let key = key.to_string();
            let expected = self.key.clone();
            let bytes = self.bytes.clone();
            Box::pin(async move {
                if key != expected {
                    return Err(StorageError::NotFound {
                        provider: ProviderKind::Local,
                        operation: StorageOperation::ObserveSource,
                        path: key,
                    });
                }
                Ok(SourceObservation {
                    receipt: SourceReceipt {
                        path: key,
                        native_version: Some("version-a".to_string()),
                        validator: Some("\"etag-a\"".to_string()),
                        total_size: Some(bytes.len() as u64),
                        requested_window: None,
                        returned_window: None,
                    },
                    content_type: None,
                    last_modified: None,
                    metadata: BTreeMap::new(),
                })
            })
        }

        fn guarded_head(&self, selection: GuardedReadSelection) -> BoxFuture<'_, SourceReceipt> {
            let bytes = self.bytes.clone();
            Box::pin(async move {
                selection.validate_for(
                    "mock-target",
                    selection.key(),
                    StorageOperation::GuardedHead,
                )?;
                Ok(SourceReceipt {
                    path: selection.key().to_string(),
                    native_version: Some("version-a".to_string()),
                    validator: Some("\"etag-a\"".to_string()),
                    total_size: Some(bytes.len() as u64),
                    requested_window: None,
                    returned_window: None,
                })
            })
        }

        fn guarded_get(
            &self,
            selection: GuardedReadSelection,
        ) -> BoxFuture<'_, GuardedReadResponse> {
            let bytes = self.bytes.clone();
            let chunk_size = self.chunk_size;
            Box::pin(async move {
                selection.validate_for(
                    "mock-target",
                    selection.key(),
                    StorageOperation::GuardedGet,
                )?;
                let window = (!bytes.is_empty()).then(|| ByteWindow {
                    start: 0,
                    end: bytes.len() as u64 - 1,
                });
                Ok(GuardedReadResponse {
                    receipt: SourceReceipt {
                        path: selection.key().to_string(),
                        native_version: Some("version-a".to_string()),
                        validator: Some("\"etag-a\"".to_string()),
                        total_size: Some(bytes.len() as u64),
                        requested_window: None,
                        returned_window: window,
                    },
                    reader: Self::stream_from_bytes(bytes.as_slice(), chunk_size),
                })
            })
        }

        fn guarded_get_range(
            &self,
            request: GuardedRangeRequest,
        ) -> BoxFuture<'_, GuardedReadResponse> {
            let bytes = self.bytes.clone();
            let chunk_size = self.chunk_size;
            let reject_bound_range = self.reject_bound_range;
            let range_fault = self.range_fault;
            let guarded_range_calls = self.guarded_range_calls.clone();
            Box::pin(async move {
                *guarded_range_calls
                    .lock()
                    .expect("guarded range mutex poisoned") += 1;
                request.selection.validate_for(
                    "mock-target",
                    request.selection.key(),
                    StorageOperation::GuardedGetRange,
                )?;
                if reject_bound_range {
                    return Err(StorageError::Conflict {
                        provider: ProviderKind::Local,
                        operation: StorageOperation::GuardedGetRange,
                        target: None,
                        kind: storageprims_core::ConflictKind::TokenMismatch,
                        detail: "selected source changed".to_string(),
                    });
                }
                let requested_end = request.offset.checked_add(request.length - 1).ok_or(
                    StorageError::InvalidArgument {
                        operation: Some(StorageOperation::GuardedGetRange),
                        argument: "length".to_string(),
                        reason: "range overflows".to_string(),
                    },
                )?;
                if request.length == 0 || request.offset >= bytes.len() as u64 {
                    return Err(StorageError::InvalidArgument {
                        operation: Some(StorageOperation::GuardedGetRange),
                        argument: "offset".to_string(),
                        reason: "range is outside the object".to_string(),
                    });
                }
                let returned_end = requested_end.min(bytes.len() as u64 - 1);
                let start = request.offset as usize;
                let mut receipt_start = request.offset;
                let mut receipt_end = returned_end;
                let mut end = returned_end as usize + 1;
                match range_fault {
                    Some(RangeFault::WrongOffset) => receipt_start = request.offset + 1,
                    Some(RangeFault::OversizedWindow) => {
                        receipt_end = returned_end.saturating_add(1)
                    }
                    Some(RangeFault::IgnoredRange) => {
                        receipt_start = 0;
                        receipt_end = bytes.len() as u64 - 1;
                        end = bytes.len();
                    }
                    Some(RangeFault::OverlongBody) => end = (end + 1).min(bytes.len()),
                    None => {}
                }
                Ok(GuardedReadResponse {
                    receipt: SourceReceipt {
                        path: request.selection.key().to_string(),
                        native_version: Some("version-a".to_string()),
                        validator: Some("\"etag-a\"".to_string()),
                        total_size: Some(bytes.len() as u64),
                        requested_window: Some(ByteWindow {
                            start: request.offset,
                            end: requested_end,
                        }),
                        returned_window: Some(ByteWindow {
                            start: receipt_start,
                            end: receipt_end,
                        }),
                    },
                    reader: Self::stream_from_bytes(&bytes[start..end], chunk_size),
                })
            })
        }
    }

    #[test]
    fn aggregate_line_endings_returns_unknown_for_empty_input() {
        assert_eq!(aggregate_line_endings([]), LineEnding::Unknown);
    }

    #[test]
    fn aggregate_line_endings_returns_mixed_for_multiple_styles() {
        assert_eq!(
            aggregate_line_endings([LineEnding::Lf, LineEnding::CrLf]),
            LineEnding::Mixed
        );
    }

    #[tokio::test]
    async fn head_lines_streams_first_n_lines() {
        let provider = MockProvider::new("fixtures/data.txt", "a\nb\nc\nd\n");
        let result = head_lines(&provider, "fixtures/data.txt", 2)
            .await
            .expect("head lines succeeds");

        assert_eq!(result.lines, vec!["a", "b"]);
        assert_eq!(result.strategy, ReadStrategy::ForwardStream);
        assert_eq!(result.byte_offset_start, 0);
        assert_eq!(result.byte_offset_end, 4);
    }

    #[tokio::test]
    async fn preview_bytes_returns_a_selected_prefix_without_full_download_claim() {
        let provider = MockProvider::new("notes.txt", "abcdef");
        let preview = preview_bytes(&provider, "notes.txt", 3, InspectionBudget::default())
            .await
            .expect("selected prefix succeeds");
        assert_eq!(preview.bytes, b"abc");
        assert_eq!(preview.receipt.total_size, Some(6));
        assert_eq!(
            preview.receipt.returned_window,
            Some(ByteWindow { start: 0, end: 2 })
        );
        assert_eq!(preview.helper_requests, 3);
    }

    #[tokio::test]
    async fn replacement_after_observation_fails_before_returning_preview_bytes() {
        let mut provider = MockProvider::new("notes.txt", "replacement");
        provider.reject_bound_range = true;
        assert!(matches!(
            preview_bytes(&provider, "notes.txt", 3, InspectionBudget::default()).await,
            Err(StorageError::Conflict {
                kind: storageprims_core::ConflictKind::TokenMismatch,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn preview_bytes_refuses_a_wire_level_replacement_after_observation() {
        let (endpoint, requests_task) = scripted_responses_server(&[
            "HTTP/1.1 200 OK\r\ncontent-length: 11\r\netag: \"etag-a\"\r\nconnection: close\r\n\r\n",
            "HTTP/1.1 200 OK\r\ncontent-length: 11\r\netag: \"etag-a\"\r\nconnection: close\r\n\r\n",
            "HTTP/1.1 412 Precondition Failed\r\ncontent-type: application/xml\r\ncontent-length: 72\r\nconnection: close\r\n\r\n<Error><Code>PreconditionFailed</Code><Message>changed</Message></Error>",
        ])
        .await;
        let provider = scripted_s3_provider(endpoint).await;
        let error = preview_bytes(&provider, "object-a", 3, InspectionBudget::default())
            .await
            .expect_err("replacement must not expose preview bytes");
        assert!(matches!(
            error,
            StorageError::Conflict {
                kind: storageprims_core::ConflictKind::TokenMismatch,
                ..
            }
        ));
        let requests = requests_task.await.expect("server task finishes");
        assert_eq!(requests.len(), 3);
        let requests = requests
            .into_iter()
            .map(|request| {
                String::from_utf8(request)
                    .expect("HTTP request text")
                    .to_ascii_lowercase()
            })
            .collect::<Vec<_>>();
        assert!(requests[0].starts_with("head "));
        assert!(requests[1].starts_with("head "));
        assert!(requests[2].starts_with("get "));
        assert!(requests[1].contains("if-match: \"etag-a\""));
        assert!(requests[2].contains("if-match: \"etag-a\""));
        assert!(requests[2].contains("range: bytes=0-2"));
    }

    #[tokio::test]
    async fn preview_bytes_requires_guarded_reads_and_rejects_budget_before_io() {
        struct Unguarded;
        impl StorageProvider for Unguarded {
            fn provider_kind(&self) -> ProviderKind {
                ProviderKind::Local
            }
            fn capabilities(&self) -> Vec<Capability> {
                Vec::new()
            }
            fn list(&self, _: ListOptions) -> BoxFuture<'_, ListResult> {
                Box::pin(async { unreachable!() })
            }
            fn head(&self, _: &str) -> BoxFuture<'_, ObjectMetadata> {
                Box::pin(async { unreachable!() })
            }
            fn get(&self, _: &str) -> BoxFuture<'_, BoxedByteStream> {
                Box::pin(async { unreachable!() })
            }
            fn get_range(&self, _: GetRangeRequest) -> BoxFuture<'_, BoxedByteStream> {
                Box::pin(async { unreachable!() })
            }
            fn put(&self, _: &str, _: BoxedByteStream, _: PutOptions) -> BoxFuture<'_, PutResult> {
                Box::pin(async { unreachable!() })
            }
            fn delete(&self, _: &str) -> BoxFuture<'_, ()> {
                Box::pin(async { unreachable!() })
            }
            fn copy(&self, _: CopyRequest) -> BoxFuture<'_, CopyResult> {
                Box::pin(async { unreachable!() })
            }
        }

        assert!(matches!(
            preview_bytes(&Unguarded, "notes.txt", 1, InspectionBudget::default()).await,
            Err(StorageError::UnsupportedCapability { .. })
        ));
        let provider = MockProvider::new("notes.txt", "abcdef");
        let budget = InspectionBudget {
            payload_bytes: 2,
            ..InspectionBudget::default()
        };
        assert!(matches!(
            preview_bytes(&provider, "notes.txt", 3, budget).await,
            Err(StorageError::Inspection {
                kind: InspectionKind::LimitExceeded,
                limit_dimension: Some(InspectionLimitDimension::PayloadBytes),
                ..
            })
        ));
    }

    #[tokio::test]
    async fn preview_rejects_malicious_range_receipts_and_overlong_bodies_before_success() {
        for fault in [
            RangeFault::WrongOffset,
            RangeFault::OversizedWindow,
            RangeFault::IgnoredRange,
            RangeFault::OverlongBody,
        ] {
            let mut provider = MockProvider::new("notes.txt", "abcdef");
            provider.range_fault = Some(fault);
            assert!(matches!(
                preview_bytes(&provider, "notes.txt", 3, InspectionBudget::default()).await,
                Err(StorageError::Io { .. })
            ));
        }
    }

    #[tokio::test]
    async fn wall_time_overflow_is_rejected_before_provider_io() {
        let provider = MockProvider::new("notes.txt", "abc");
        let budget = InspectionBudget {
            wall_time: Duration::MAX,
            ..InspectionBudget::default()
        };
        assert!(matches!(
            preview_bytes(&provider, "notes.txt", 1, budget).await,
            Err(StorageError::InvalidArgument { .. })
        ));
    }

    #[tokio::test]
    async fn tail_and_mid_discard_a_leading_utf8_carry_before_text_validation() {
        let pending = InspectionBudget::default().pending_line_bytes as usize;
        let mut tail_bytes = vec![b'x'; pending + 10];
        let tail_start = tail_bytes.len() - pending;
        tail_bytes[tail_start - 1] = 0xC3;
        tail_bytes[tail_start] = 0xA9;
        tail_bytes[tail_start + 1] = b'\n';
        let tail_end = tail_bytes.len();
        tail_bytes[tail_end - 6..].copy_from_slice(b"\ntail\n");
        let tail = MockProvider::with_bytes("tail.txt", tail_bytes);
        assert_eq!(
            tail_lines(&tail, "tail.txt", 1)
                .await
                .expect("tail succeeds")
                .lines,
            vec!["tail"]
        );

        let mut mid_bytes = vec![b'x'; pending * 2 + 20];
        let midpoint = mid_bytes.len() / 2;
        let mid_start = midpoint - pending / 2;
        mid_bytes[mid_start - 1] = 0xC3;
        mid_bytes[mid_start] = 0xA9;
        mid_bytes[mid_start + 1..mid_start + 6].copy_from_slice(b"\nmid\n");
        mid_bytes[midpoint..midpoint + 5].copy_from_slice(b"\nmid\n");
        let mid = MockProvider::with_bytes("mid.txt", mid_bytes);
        assert_eq!(
            mid_lines(&mid, "mid.txt", 1)
                .await
                .expect("mid succeeds")
                .lines,
            vec!["mid"]
        );
    }

    #[tokio::test]
    async fn tail_keeps_a_final_unterminated_line_after_a_proven_suffix_boundary() {
        let pending = InspectionBudget::default().pending_line_bytes as usize;
        for prefix in [
            b"head\n".as_slice(),
            b"head\r".as_slice(),
            b"head\r\n".as_slice(),
        ] {
            let mut bytes = prefix.to_vec();
            bytes.extend(std::iter::repeat_n(b'x', pending));
            let provider = MockProvider::with_bytes("tail.txt", bytes);
            let result = tail_lines(&provider, "tail.txt", 1)
                .await
                .expect("selected suffix starts at a complete line boundary");
            assert_eq!(result.lines.len(), 1);
            assert_eq!(result.lines[0].len(), pending);
        }
    }

    #[tokio::test]
    async fn force_stream_refuses_unproven_prefixes_for_tail_and_mid() {
        let pending = InspectionBudget::default().pending_line_bytes as usize;
        let mut bytes = b"first\nsecond\n".to_vec();
        bytes.extend(std::iter::repeat_n(b'x', pending));
        let provider = MockProvider::with_bytes("large.txt", bytes);
        let options = LineOptions {
            force_stream: true,
            ..LineOptions::default()
        };
        for result in [
            tail_lines_with_options(&provider, "large.txt", 1, options.clone()).await,
            mid_lines_with_options(&provider, "large.txt", 1, options).await,
        ] {
            assert!(matches!(
                result,
                Err(StorageError::Inspection {
                    kind: InspectionKind::LimitExceeded,
                    limit_dimension: Some(InspectionLimitDimension::PendingLineBytes),
                    ..
                })
            ));
        }
    }

    #[tokio::test]
    async fn line_options_reject_unbounded_inputs_before_provider_selection() {
        let provider = MockProvider::new("notes.txt", "one\n");
        assert!(matches!(
            head_lines(&provider, "notes.txt", 0).await,
            Err(StorageError::InvalidArgument { .. })
        ));
        assert!(matches!(
            head_lines_with_options(
                &provider,
                "notes.txt",
                1,
                LineOptions {
                    chunk_size: u64::MAX,
                    ..LineOptions::default()
                },
            )
            .await,
            Err(StorageError::InvalidArgument { .. })
        ));
    }

    #[tokio::test]
    async fn giant_unterminated_line_is_a_typed_limit_not_a_partial_result() {
        let provider = MockProvider::with_bytes(
            "notes.txt",
            vec![b'x'; InspectionBudget::default().pending_line_bytes as usize + 1],
        );
        assert!(matches!(
            head_lines(&provider, "notes.txt", 1).await,
            Err(StorageError::Inspection {
                kind: InspectionKind::LimitExceeded,
                limit_dimension: Some(InspectionLimitDimension::PendingLineBytes),
                ..
            })
        ));
    }

    #[tokio::test]
    async fn head_stops_at_requested_line_bound_for_hostile_short_lines() {
        let provider = MockProvider::with_bytes(
            "notes.txt",
            vec![b'\n'; InspectionBudget::default().pending_line_bytes as usize],
        );
        let result = head_lines(&provider, "notes.txt", 1)
            .await
            .expect("first line succeeds without retaining every short line");
        assert_eq!(result.lines, vec![""]);
    }

    #[tokio::test]
    async fn tail_lines_uses_one_bounded_guarded_suffix() {
        let provider = MockProvider::new("fixtures/data.txt", "a\nb\nc\nd\n");
        let result = tail_lines_with_options(
            &provider,
            "fixtures/data.txt",
            2,
            LineOptions {
                chunk_size: 2,
                ..LineOptions::default()
            },
        )
        .await
        .expect("tail lines succeeds");

        assert_eq!(result.lines, vec!["c", "d"]);
        assert_eq!(result.strategy, ReadStrategy::RangeBackward);
        assert!(provider
            .range_calls
            .lock()
            .expect("range mutex poisoned")
            .is_empty());
    }

    #[tokio::test]
    async fn mid_lines_returns_first_lines_after_midpoint_alignment() {
        let provider = MockProvider::new("fixtures/data.txt", "a\nb\nc\nd\ne\n");
        let result = mid_lines_with_options(
            &provider,
            "fixtures/data.txt",
            2,
            LineOptions {
                chunk_size: 4,
                ..LineOptions::default()
            },
        )
        .await
        .expect("mid lines succeeds");

        assert_eq!(result.lines, vec!["d", "e"]);
        assert_eq!(result.strategy, ReadStrategy::RangeMidpoint);
    }

    #[tokio::test]
    async fn count_lines_uses_logical_line_semantics() {
        let provider = MockProvider::new("fixtures/data.txt", "abc\ndef");
        let count = count_lines(&provider, "fixtures/data.txt")
            .await
            .expect("count succeeds");
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn count_lines_accepts_single_line_without_terminator() {
        let provider = MockProvider::new("fixtures/data.txt", "abc");
        let count = count_lines(&provider, "fixtures/data.txt")
            .await
            .expect("count succeeds");
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn count_lines_returns_zero_for_empty_object() {
        let provider = MockProvider::new("fixtures/data.txt", "");
        let count = count_lines(&provider, "fixtures/data.txt")
            .await
            .expect("count succeeds");
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn tail_lines_rejects_compressed_objects_without_force_stream() {
        let provider = MockProvider::new("fixtures/data.txt.gz", "a\nb\n");
        let error =
            tail_lines_with_options(&provider, "fixtures/data.txt.gz", 1, LineOptions::default())
                .await
                .expect_err("compressed tail should fail");

        assert!(matches!(
            error,
            StorageError::Inspection {
                kind: InspectionKind::EncodingRejected,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn head_lines_rejects_binary_content() {
        let provider = MockProvider::with_bytes("fixtures/data.bin", vec![0x00, 0xFF, 0xAA]);
        let error = head_lines(&provider, "fixtures/data.bin", 1)
            .await
            .expect_err("binary content should fail");

        assert!(matches!(
            error,
            StorageError::Inspection {
                kind: InspectionKind::EncodingRejected,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn head_lines_accepts_single_line_without_terminator() {
        let provider = MockProvider::new("fixtures/data.txt", "abc");
        let result = head_lines(&provider, "fixtures/data.txt", 1)
            .await
            .expect("head lines succeeds");

        assert_eq!(result.lines, vec!["abc"]);
        assert_eq!(result.byte_offset_start, 0);
        assert_eq!(result.byte_offset_end, 3);
    }

    #[tokio::test]
    async fn terminal_cr_is_a_delimiter_at_proven_eof_for_head_tail_and_mid() {
        let head = MockProvider::new("head.txt", "a\r");
        let result = head_lines(&head, "head.txt", 1)
            .await
            .expect("head accepts a terminal CR");
        assert_eq!(result.lines, vec!["a"]);
        assert_eq!(result.byte_offset_start, 0);
        assert_eq!(result.byte_offset_end, 2);
        assert_eq!(result.line_ending, LineEnding::Cr);

        let tail = MockProvider::new("tail.txt", "a\r");
        let result = tail_lines(&tail, "tail.txt", 1)
            .await
            .expect("tail accepts a terminal CR");
        assert_eq!(result.lines, vec!["a"]);
        assert_eq!(result.byte_offset_start, 0);
        assert_eq!(result.byte_offset_end, 2);
        assert_eq!(result.line_ending, LineEnding::Cr);

        let mid = MockProvider::new("mid.txt", "head\nmid\r");
        let result = mid_lines(&mid, "mid.txt", 1)
            .await
            .expect("mid accepts a terminal CR");
        assert_eq!(result.lines, vec!["mid"]);
        assert_eq!(result.byte_offset_start, 5);
        assert_eq!(result.byte_offset_end, 9);
        assert_eq!(result.line_ending, LineEnding::Cr);
    }

    #[tokio::test]
    async fn terminal_cr_preserves_mixed_endings_and_probe_edge_crlf_framing() {
        let provider = MockProvider::new("mixed.txt", "a\nb\r");
        let result = head_lines(&provider, "mixed.txt", 2)
            .await
            .expect("mixed ending object succeeds");
        assert_eq!(result.lines, vec!["a", "b"]);
        assert_eq!(result.line_ending, LineEnding::Mixed);

        let provider = MockProvider::new("edge.txt", "a\r\nb");
        let result = head_lines_with_options(
            &provider,
            "edge.txt",
            2,
            LineOptions {
                probe_size: 2,
                ..LineOptions::default()
            },
        )
        .await
        .expect("CRLF split at the probe edge remains one delimiter");
        assert_eq!(result.lines, vec!["a", "b"]);
        assert_eq!(result.line_ending, LineEnding::CrLf);
    }

    #[tokio::test]
    async fn count_lines_preflight_reports_the_actual_exhausted_limit_without_a_body_read() {
        let pending_size = 2 * 1024 * 1024;
        let pending = MockProvider::with_bytes("pending.txt", vec![b'\n'; pending_size]);
        let error = count_lines(&pending, "pending.txt")
            .await
            .expect_err("pending line budget is the first exhausted limit");
        assert!(matches!(
            error,
            StorageError::Inspection {
                kind: InspectionKind::LimitExceeded,
                limit_dimension: Some(InspectionLimitDimension::PendingLineBytes),
                configured_limit,
                consumed: 0,
                ..
            } if configured_limit == InspectionBudget::default().pending_line_bytes
        ));
        assert_eq!(
            *pending
                .guarded_range_calls
                .lock()
                .expect("guarded range mutex poisoned"),
            0
        );

        let payload_size = InspectionBudget::default().payload_bytes as usize + 1;
        let payload = MockProvider::with_bytes("payload.txt", vec![b'\n'; payload_size]);
        let error = count_lines(&payload, "payload.txt")
            .await
            .expect_err("payload budget is exhausted above its cap");
        assert!(matches!(
            error,
            StorageError::Inspection {
                kind: InspectionKind::LimitExceeded,
                limit_dimension: Some(InspectionLimitDimension::PayloadBytes),
                configured_limit,
                consumed: 0,
                ..
            } if configured_limit == InspectionBudget::default().payload_bytes
        ));
        assert_eq!(
            *payload
                .guarded_range_calls
                .lock()
                .expect("guarded range mutex poisoned"),
            0
        );
    }

    #[tokio::test]
    async fn mid_lines_handles_exact_newline_midpoint() {
        let provider = MockProvider::new("fixtures/data.txt", "aa\nbb\ncc\n");
        let result = mid_lines_with_options(
            &provider,
            "fixtures/data.txt",
            1,
            LineOptions {
                chunk_size: 6,
                ..LineOptions::default()
            },
        )
        .await
        .expect("mid lines succeeds");

        assert_eq!(result.lines, vec!["cc"]);
    }

    #[tokio::test]
    async fn mid_lines_returns_a_final_unterminated_line_at_proven_eof() {
        let provider = MockProvider::new("fixtures/data.txt", "abc");
        let result = mid_lines(&provider, "fixtures/data.txt", 1)
            .await
            .expect("one-line object succeeds");
        assert_eq!(result.lines, vec!["abc"]);
    }

    #[tokio::test]
    async fn parse_lines_handles_mixed_line_endings() {
        let parsed = parse_lines(
            b"a\r\nb\nc\rd",
            0,
            true,
            ProviderKind::Local,
            StorageOperation::HeadLines,
        )
        .expect("parse succeeds");

        assert_eq!(parsed.len(), 4);
        assert_eq!(parsed[0].ending, Some(LineEnding::CrLf));
        assert_eq!(parsed[1].ending, Some(LineEnding::Lf));
        assert_eq!(parsed[2].ending, Some(LineEnding::Cr));
        assert_eq!(parsed[3].ending, None);
    }

    #[tokio::test]
    async fn head_lines_handles_crlf_split_across_stream_chunks() {
        let mut provider = MockProvider::new("fixtures/data.txt", "alpha\r\nbeta\r\ngamma\r\n");
        provider.chunk_size = 6;

        let result = head_lines(&provider, "fixtures/data.txt", 2)
            .await
            .expect("head lines succeeds");

        assert_eq!(result.lines, vec!["alpha", "beta"]);
        assert_eq!(result.line_ending, LineEnding::CrLf);
    }

    #[tokio::test]
    async fn count_lines_handles_crlf_split_across_stream_chunks() {
        let mut provider = MockProvider::new("fixtures/data.txt", "alpha\r\nbeta\r\ngamma");
        provider.chunk_size = 6;

        let count = count_lines(&provider, "fixtures/data.txt")
            .await
            .expect("count succeeds");

        assert_eq!(count, 3);
    }

    #[tokio::test]
    async fn tail_lines_prefers_compressed_guard_over_binary_probe() {
        let provider =
            MockProvider::with_bytes("fixtures/data.txt.gz", vec![0x1F, 0x8B, 0x08, 0x00, 0x00]);
        let error =
            tail_lines_with_options(&provider, "fixtures/data.txt.gz", 1, LineOptions::default())
                .await
                .expect_err("compressed tail should fail with compressed guard");

        assert!(matches!(
            error,
            StorageError::Inspection {
                kind: InspectionKind::EncodingRejected,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn tail_lines_force_stream_reaches_stream_parser_for_compressed_bytes() {
        let provider =
            MockProvider::with_bytes("fixtures/data.txt.gz", vec![0x1F, 0x8B, 0x08, 0x00, 0x00]);
        let error = tail_lines_with_options(
            &provider,
            "fixtures/data.txt.gz",
            1,
            LineOptions {
                force_stream: true,
                ..LineOptions::default()
            },
        )
        .await
        .expect_err("compressed bytes should still fail once parsed");

        assert!(matches!(
            error,
            StorageError::Inspection {
                kind: InspectionKind::EncodingRejected,
                ..
            }
        ));
    }
}
