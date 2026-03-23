//! Higher-level provider-agnostic operations composed from storageprims-core.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use storageprims_core::{
    BoxedByteStream, GetRangeRequest, ObjectMetadata, ProviderKind, StorageError, StorageOperation,
    StorageProvider,
};
use tokio::io::AsyncReadExt;

const STREAM_BUFFER_SIZE: usize = 4 * 1024 * 1024;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContentKind {
    Text,
    Empty,
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
    validate_line_count(StorageOperation::HeadLines, n)?;
    let metadata = provider.head(key).await?;
    if metadata.size == 0 {
        return Ok(empty_result(ReadStrategy::ForwardStream, metadata.size));
    }

    classify_content(
        provider,
        key,
        &metadata,
        &options,
        StorageOperation::HeadLines,
    )
    .await?;
    let lines = read_head_lines(provider, key, n).await?;
    Ok(build_line_result(
        lines,
        metadata.size,
        ReadStrategy::ForwardStream,
    ))
}

pub async fn tail_lines(
    provider: &dyn StorageProvider,
    key: &str,
    n: usize,
    options: LineOptions,
) -> storageprims_core::Result<LineResult> {
    validate_line_count(StorageOperation::TailLines, n)?;
    let metadata = provider.head(key).await?;
    if metadata.size == 0 {
        return Ok(empty_result(ReadStrategy::RangeBackward, metadata.size));
    }

    if is_compressed_key(key) && !options.force_stream {
        return Err(line_ops_error(
            provider.provider_kind(),
            StorageOperation::TailLines,
            LineOpsError::CompressedObject,
        ));
    }

    if is_compressed_key(key) && options.force_stream {
        return tail_lines_forward_stream(provider, key, n, metadata.size).await;
    }

    classify_content(
        provider,
        key,
        &metadata,
        &options,
        StorageOperation::TailLines,
    )
    .await?;

    let mut window = metadata.size.min(options.chunk_size.max(1));
    loop {
        let start = metadata.size.saturating_sub(window);
        let bytes = read_range(
            provider,
            key,
            start,
            metadata.size - start,
            StorageOperation::TailLines,
        )
        .await?;
        let boundary_before_start = slice_starts_on_line_boundary(
            previous_byte(provider, key, start, StorageOperation::TailLines).await?,
            bytes.first().copied(),
        );
        let Some(aligned_start) = first_full_line_start(&bytes, boundary_before_start) else {
            if start == 0 {
                let parsed = parse_lines(
                    &bytes,
                    start,
                    true,
                    provider.provider_kind(),
                    StorageOperation::TailLines,
                )?;
                return Ok(build_line_result(
                    parsed
                        .into_iter()
                        .rev()
                        .take(n)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect(),
                    metadata.size,
                    ReadStrategy::RangeBackward,
                ));
            }
            window = grow_window(window, metadata.size);
            continue;
        };

        let parsed = parse_lines(
            &bytes[aligned_start..],
            start + aligned_start as u64,
            true,
            provider.provider_kind(),
            StorageOperation::TailLines,
        )?;

        if parsed.len() >= n || start == 0 {
            let selected = parsed
                .into_iter()
                .rev()
                .take(n)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>();
            return Ok(build_line_result(
                selected,
                metadata.size,
                ReadStrategy::RangeBackward,
            ));
        }

        window = grow_window(window, metadata.size);
    }
}

pub async fn mid_lines(
    provider: &dyn StorageProvider,
    key: &str,
    n: usize,
    options: LineOptions,
) -> storageprims_core::Result<LineResult> {
    validate_line_count(StorageOperation::MidLines, n)?;
    let metadata = provider.head(key).await?;
    if metadata.size == 0 {
        return Ok(empty_result(ReadStrategy::RangeMidpoint, metadata.size));
    }

    if is_compressed_key(key) && !options.force_stream {
        return Err(line_ops_error(
            provider.provider_kind(),
            StorageOperation::MidLines,
            LineOpsError::CompressedObject,
        ));
    }

    if is_compressed_key(key) && options.force_stream {
        return mid_lines_forward_stream(provider, key, n, metadata.size).await;
    }

    classify_content(
        provider,
        key,
        &metadata,
        &options,
        StorageOperation::MidLines,
    )
    .await?;

    let midpoint = metadata.size / 2;
    let mut window = metadata.size.min(options.chunk_size.max(1));
    loop {
        let start = midpoint.saturating_sub(window / 2);
        let end = metadata.size.min(start.saturating_add(window));
        let bytes = read_range(
            provider,
            key,
            start,
            end - start,
            StorageOperation::MidLines,
        )
        .await?;
        let Some(aligned_start) = midpoint_alignment(&bytes, midpoint - start) else {
            if start == 0 && end == metadata.size {
                let parsed = parse_lines(
                    &bytes,
                    start,
                    true,
                    provider.provider_kind(),
                    StorageOperation::MidLines,
                )?;
                let selected = parsed.into_iter().take(n).collect::<Vec<_>>();
                return Ok(build_line_result(
                    selected,
                    metadata.size,
                    ReadStrategy::RangeMidpoint,
                ));
            }
            window = grow_window(window, metadata.size);
            continue;
        };

        let parsed = parse_lines(
            &bytes[aligned_start..],
            start + aligned_start as u64,
            end == metadata.size,
            provider.provider_kind(),
            StorageOperation::MidLines,
        )?;
        if parsed.len() >= n || end == metadata.size {
            let selected = parsed.into_iter().take(n).collect::<Vec<_>>();
            return Ok(build_line_result(
                selected,
                metadata.size,
                ReadStrategy::RangeMidpoint,
            ));
        }

        window = grow_window(window, metadata.size);
    }
}

pub async fn count_lines(
    provider: &dyn StorageProvider,
    key: &str,
) -> storageprims_core::Result<u64> {
    let options = LineOptions::default();
    let metadata = provider.head(key).await?;
    if metadata.size == 0 {
        return Ok(0);
    }

    classify_content(
        provider,
        key,
        &metadata,
        &options,
        StorageOperation::CountLines,
    )
    .await?;
    let mut stream = provider.get(key).await?;
    let mut pending = Vec::new();
    let mut count = 0_u64;
    let mut buffer = vec![0_u8; STREAM_BUFFER_SIZE];

    loop {
        let read = stream
            .read(&mut buffer)
            .await
            .map_err(|error| StorageError::Io {
                operation: Some(StorageOperation::CountLines),
                source: error,
            })?;
        if read == 0 {
            break;
        }

        pending.extend_from_slice(&buffer[..read]);
        while let Some((consumed, _ending)) = take_first_complete_line(
            &mut pending,
            0,
            provider.provider_kind(),
            StorageOperation::CountLines,
        )? {
            let _ = consumed;
            count += 1;
        }
    }

    if !pending.is_empty() {
        std::str::from_utf8(&pending).map_err(|_| {
            line_ops_error(
                provider.provider_kind(),
                StorageOperation::CountLines,
                LineOpsError::BinaryContent,
            )
        })?;
        count += 1;
    }

    Ok(count)
}

fn validate_line_count(operation: StorageOperation, n: usize) -> storageprims_core::Result<()> {
    if n == 0 {
        return Err(StorageError::InvalidArgument {
            operation: Some(operation),
            argument: "n".to_string(),
            reason: "line count must be greater than zero".to_string(),
        });
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
    StorageError::Other {
        provider: Some(provider),
        operation: Some(operation),
        detail: error.to_string(),
        source: None,
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

async fn classify_content(
    provider: &dyn StorageProvider,
    key: &str,
    metadata: &ObjectMetadata,
    options: &LineOptions,
    operation: StorageOperation,
) -> storageprims_core::Result<ContentKind> {
    if metadata.size == 0 {
        return Ok(ContentKind::Empty);
    }

    let probe_len = metadata.size.min(options.probe_size as u64);
    let probe = read_range(provider, key, 0, probe_len, operation).await?;
    if probe.is_empty() {
        return Ok(ContentKind::Empty);
    }

    if starts_with_utf16_bom(&probe) || probe.contains(&0) {
        return Err(line_ops_error(
            provider.provider_kind(),
            operation,
            LineOpsError::BinaryContent,
        ));
    }

    let decode_probe = strip_utf8_bom(&probe);
    if std::str::from_utf8(decode_probe).is_err() {
        return Err(line_ops_error(
            provider.provider_kind(),
            operation,
            LineOpsError::BinaryContent,
        ));
    }

    if contains_line_terminator(decode_probe) {
        return Ok(ContentKind::Text);
    }

    if probe_len == metadata.size {
        Ok(ContentKind::Text)
    } else {
        Err(line_ops_error(
            provider.provider_kind(),
            operation,
            LineOpsError::NoLineTerminatorInProbe {
                probe_size: options.probe_size,
            },
        ))
    }
}

async fn read_head_lines(
    provider: &dyn StorageProvider,
    key: &str,
    n: usize,
) -> storageprims_core::Result<Vec<ParsedLine>> {
    let mut stream = provider.get(key).await?;
    let mut pending = Vec::new();
    let mut pending_start = 0_u64;
    let mut lines = Vec::new();
    let mut buffer = vec![0_u8; STREAM_BUFFER_SIZE];

    loop {
        if lines.len() >= n {
            break;
        }

        let read = stream
            .read(&mut buffer)
            .await
            .map_err(|error| StorageError::Io {
                operation: Some(StorageOperation::HeadLines),
                source: error,
            })?;
        if read == 0 {
            break;
        }

        pending.extend_from_slice(&buffer[..read]);
        while lines.len() < n {
            let Some((line, consumed)) = extract_first_complete_line(
                &pending,
                pending_start,
                provider.provider_kind(),
                StorageOperation::HeadLines,
                false,
            )?
            else {
                break;
            };
            pending.drain(..consumed);
            pending_start += consumed as u64;
            lines.push(line);
        }
    }

    while lines.len() < n {
        let Some((line, consumed)) = extract_first_complete_line(
            &pending,
            pending_start,
            provider.provider_kind(),
            StorageOperation::HeadLines,
            true,
        )?
        else {
            break;
        };
        pending.drain(..consumed);
        pending_start += consumed as u64;
        lines.push(line);
    }

    if lines.len() < n && !pending.is_empty() {
        let text = std::str::from_utf8(&pending)
            .map_err(|_| {
                line_ops_error(
                    provider.provider_kind(),
                    StorageOperation::HeadLines,
                    LineOpsError::BinaryContent,
                )
            })?
            .to_string();
        lines.push(ParsedLine {
            text,
            start: pending_start,
            end: pending_start + pending.len() as u64,
            ending: None,
        });
    }

    Ok(lines)
}

async fn tail_lines_forward_stream(
    provider: &dyn StorageProvider,
    key: &str,
    n: usize,
    total_size: u64,
) -> storageprims_core::Result<LineResult> {
    let bytes = read_all(provider.get(key).await?, StorageOperation::TailLines).await?;
    let parsed = parse_lines(
        &bytes,
        0,
        true,
        provider.provider_kind(),
        StorageOperation::TailLines,
    )?;
    let selected = parsed
        .into_iter()
        .rev()
        .take(n)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>();
    Ok(build_line_result(
        selected,
        total_size,
        ReadStrategy::ForwardStream,
    ))
}

async fn mid_lines_forward_stream(
    provider: &dyn StorageProvider,
    key: &str,
    n: usize,
    total_size: u64,
) -> storageprims_core::Result<LineResult> {
    let bytes = read_all(provider.get(key).await?, StorageOperation::MidLines).await?;
    let midpoint = total_size / 2;
    let aligned_start = midpoint_alignment(&bytes, midpoint).unwrap_or(bytes.len());
    let parsed = parse_lines(
        &bytes[aligned_start..],
        aligned_start as u64,
        true,
        provider.provider_kind(),
        StorageOperation::MidLines,
    )?;
    Ok(build_line_result(
        parsed.into_iter().take(n).collect(),
        total_size,
        ReadStrategy::ForwardStream,
    ))
}

async fn read_range(
    provider: &dyn StorageProvider,
    key: &str,
    offset: u64,
    length: u64,
    operation: StorageOperation,
) -> storageprims_core::Result<Vec<u8>> {
    if length == 0 {
        return Ok(Vec::new());
    }

    let stream = provider
        .get_range(GetRangeRequest {
            key: key.to_string(),
            offset,
            length,
        })
        .await?;
    read_all(stream, operation).await
}

async fn read_all(
    mut stream: BoxedByteStream,
    operation: StorageOperation,
) -> storageprims_core::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    stream
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| StorageError::Io {
            operation: Some(operation),
            source: error,
        })?;
    Ok(bytes)
}

async fn previous_byte(
    provider: &dyn StorageProvider,
    key: &str,
    start: u64,
    operation: StorageOperation,
) -> storageprims_core::Result<Option<u8>> {
    if start == 0 {
        return Ok(None);
    }
    let byte = read_range(provider, key, start - 1, 1, operation).await?;
    Ok(byte.first().copied())
}

fn slice_starts_on_line_boundary(previous_byte: Option<u8>, first_byte: Option<u8>) -> bool {
    match previous_byte {
        None => true,
        Some(b'\n') => true,
        Some(b'\r') if first_byte != Some(b'\n') => true,
        _ => false,
    }
}

fn first_full_line_start(bytes: &[u8], boundary_before_start: bool) -> Option<usize> {
    if boundary_before_start {
        return Some(0);
    }

    next_line_boundary(bytes, 0)
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

fn take_first_complete_line(
    pending: &mut Vec<u8>,
    base_offset: u64,
    provider: ProviderKind,
    operation: StorageOperation,
) -> storageprims_core::Result<Option<(usize, Option<LineEnding>)>> {
    let Some((line, consumed)) =
        extract_first_complete_line(pending, base_offset, provider, operation, false)?
    else {
        return Ok(None);
    };
    pending.drain(..consumed);
    Ok(Some((consumed, line.ending)))
}

fn contains_line_terminator(bytes: &[u8]) -> bool {
    bytes.iter().any(|byte| matches!(byte, b'\n' | b'\r'))
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

fn grow_window(current: u64, total_size: u64) -> u64 {
    current.saturating_mul(2).min(total_size)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, VecDeque};
    use std::sync::{Arc, Mutex};

    use bytes::Bytes;
    use storageprims_core::{
        BoxFuture, CopyRequest, CopyResult, CopyStrategy, GetRangeRequest, ListOptions, ListResult,
        ObjectMetadata, ProviderKind, PutOptions, PutResult, StorageError, StorageProvider,
    };
    use tokio_util::io::StreamReader;

    use super::*;

    #[derive(Default, Clone)]
    struct MockProvider {
        key: String,
        bytes: Arc<Vec<u8>>,
        chunk_size: usize,
        head_calls: Arc<Mutex<u64>>,
        range_calls: Arc<Mutex<Vec<GetRangeRequest>>>,
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

    impl StorageProvider for MockProvider {
        fn provider_kind(&self) -> ProviderKind {
            ProviderKind::Local
        }

        fn capabilities(&self) -> Vec<storageprims_core::Capability> {
            Vec::new()
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
            _options: PutOptions,
        ) -> BoxFuture<'_, PutResult> {
            let key = key.to_string();
            Box::pin(async move {
                Ok(PutResult {
                    path: key,
                    etag: None,
                    size: None,
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
    async fn tail_lines_uses_expanding_ranges() {
        let provider = MockProvider::new("fixtures/data.txt", "a\nb\nc\nd\n");
        let result = tail_lines(
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
        assert!(
            provider
                .range_calls
                .lock()
                .expect("range mutex poisoned")
                .len()
                >= 2
        );
    }

    #[tokio::test]
    async fn mid_lines_returns_first_lines_after_midpoint_alignment() {
        let provider = MockProvider::new("fixtures/data.txt", "a\nb\nc\nd\ne\n");
        let result = mid_lines(
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
        let error = tail_lines(&provider, "fixtures/data.txt.gz", 1, LineOptions::default())
            .await
            .expect_err("compressed tail should fail");

        assert!(matches!(error, StorageError::Other { .. }));
        assert!(error.to_string().contains("compressed object"));
    }

    #[tokio::test]
    async fn head_lines_rejects_binary_content() {
        let provider = MockProvider::with_bytes("fixtures/data.bin", vec![0x00, 0xFF, 0xAA]);
        let error = head_lines(&provider, "fixtures/data.bin", 1)
            .await
            .expect_err("binary content should fail");

        assert!(error.to_string().contains("binary content"));
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
    async fn mid_lines_handles_exact_newline_midpoint() {
        let provider = MockProvider::new("fixtures/data.txt", "aa\nbb\ncc\n");
        let result = mid_lines(
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
        let error = tail_lines(&provider, "fixtures/data.txt.gz", 1, LineOptions::default())
            .await
            .expect_err("compressed tail should fail with compressed guard");

        assert!(error.to_string().contains("compressed object"));
        assert!(!error.to_string().contains("binary content"));
    }

    #[tokio::test]
    async fn tail_lines_force_stream_reaches_stream_parser_for_compressed_bytes() {
        let provider =
            MockProvider::with_bytes("fixtures/data.txt.gz", vec![0x1F, 0x8B, 0x08, 0x00, 0x00]);
        let error = tail_lines(
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

        assert!(error.to_string().contains("binary content"));
        assert!(!error.to_string().contains("compressed object"));
    }
}
