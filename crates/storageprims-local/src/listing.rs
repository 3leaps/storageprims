//! Bounded-window lexical DFS flat listing and slash-only single-directory delimiter listing.

use crate::{namespace, platform, BaseDir, CandidateStat, ScannedEntry};
use std::{
    collections::VecDeque,
    sync::OnceLock,
    time::{Duration, Instant},
};
use storageprims_core::{
    DelimiterListRequest, DelimiterListResult, InspectionKind, InspectionLimitDimension,
    ListOptions, ListResult, ObjectSummary, ProviderKind, Result, StorageError, StorageOperation,
};

pub(crate) const DEFAULT_MAX_KEYS: u32 = 1_000;
pub(crate) const MAX_LOCAL_KEYS: u32 = 1_000;
pub(crate) const DEFAULT_ENTRY_BUDGET: u64 = 100_000;
pub(crate) const DEFAULT_LIST_DEADLINE: Duration = Duration::from_millis(10_000);
pub(crate) const MAX_TOKEN_BYTES: usize = 8_192;

const TOKEN_VERSION_V1: u8 = 1;
const TOKEN_MODE_FLAT: u8 = 1;
const TOKEN_MODE_DELIMITED_SLASH: u8 = 2;

#[cfg(test)]
static TEST_POST_RESOLVE_DELAY_MS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

#[derive(Debug, Clone, Copy)]
pub(crate) struct ListLimits {
    pub max_entries: u64,
    pub max_duration: Duration,
}

impl Default for ListLimits {
    fn default() -> Self {
        Self {
            max_entries: DEFAULT_ENTRY_BUDGET,
            max_duration: DEFAULT_LIST_DEADLINE,
        }
    }
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Copy)]
pub(crate) struct ListStats {
    pub examined: u64,
    pub peak_slots: usize,
}

pub(crate) struct Ledger {
    started: Instant,
    limits: ListLimits,
    examined: u64,
}

impl Ledger {
    pub(crate) fn new(limits: ListLimits) -> Self {
        Self {
            started: Instant::now(),
            limits,
            examined: 0,
        }
    }

    #[cfg(all(unix, not(target_vendor = "apple"), test))]
    pub(crate) fn examined(&self) -> u64 {
        self.examined
    }

    pub(crate) fn check_deadline(&self) -> Result<()> {
        let elapsed = self.started.elapsed();
        if elapsed > self.limits.max_duration {
            let configured_ms =
                u64::try_from(self.limits.max_duration.as_millis()).unwrap_or(u64::MAX);
            let consumed_ms = u64::try_from(elapsed.as_millis())
                .unwrap_or(u64::MAX)
                .max(configured_ms.saturating_add(1));
            return Err(StorageError::Inspection {
                provider: Some(ProviderKind::Local),
                operation: StorageOperation::List,
                kind: InspectionKind::LimitExceeded,
                limit_dimension: Some(InspectionLimitDimension::Deadline),
                configured_limit: configured_ms,
                consumed: consumed_ms,
            });
        }
        Ok(())
    }

    pub(crate) fn charge_entry(&mut self) -> Result<()> {
        self.examined = self.examined.saturating_add(1);
        if self.examined > self.limits.max_entries {
            return Err(StorageError::Inspection {
                provider: Some(ProviderKind::Local),
                operation: StorageOperation::List,
                kind: InspectionKind::LimitExceeded,
                limit_dimension: Some(InspectionLimitDimension::Entries),
                configured_limit: self.limits.max_entries,
                consumed: self.examined,
            });
        }
        self.check_deadline()
    }
}

struct WindowEntry {
    label: String,
    is_dir: bool,
}

struct Frame {
    comp_len_before: usize,
    path_len_before: usize,
    last_label: String,
    need_rescan: bool,
}

pub(crate) fn resolve_max_keys(max_keys: Option<u32>) -> Result<usize> {
    match max_keys {
        None | Some(0) => Ok(DEFAULT_MAX_KEYS as usize),
        Some(value) if value <= MAX_LOCAL_KEYS => Ok(value as usize),
        Some(_) => Err(StorageError::InvalidArgument {
            operation: Some(StorageOperation::List),
            argument: "max_keys".into(),
            reason: "local listing max_keys must be 0 or within 1..=1000".into(),
        }),
    }
}

pub(crate) fn list_flat(
    root: &platform::Root,
    options: &ListOptions,
    limits: ListLimits,
) -> Result<(ListResult, ListStats)> {
    let ledger_init = Ledger::new(limits);
    let k = resolve_max_keys(options.max_keys)?;
    let raw_prefix = options.prefix.as_deref().unwrap_or("");
    let parsed = namespace::parse_list_prefix(options.prefix.as_deref())?;

    let after = match options.continuation_token.as_deref() {
        Some(token) => {
            let cursor = decode_token(
                token,
                root.root_id(),
                TOKEN_MODE_FLAT,
                raw_prefix,
                "",
                k as u32,
            )?;
            namespace::validate(&cursor, StorageOperation::List)
                .map_err(|_| invalid_continuation_token())?;
            if !cursor.starts_with(raw_prefix) {
                return Err(invalid_continuation_token());
            }
            cursor
        }
        None => String::new(),
    };

    let mut ledger = ledger_init;
    ledger.check_deadline()?;
    let resolved_base = root.resolve_list_base(&parsed.dir_components, &mut ledger)?;
    #[cfg(test)]
    {
        let delay_ms = TEST_POST_RESOLVE_DELAY_MS.load(std::sync::atomic::Ordering::Relaxed);
        if delay_ms > 0 {
            std::thread::sleep(Duration::from_millis(delay_ms));
        }
    }
    ledger.check_deadline()?;
    let base_dir = match resolved_base {
        BaseDir::Empty => {
            return Ok((
                ListResult {
                    objects: Vec::new(),
                    continuation_token: None,
                    is_truncated: false,
                },
                ListStats {
                    examined: ledger.examined,
                    peak_slots: 0,
                },
            ));
        }
        BaseDir::Ready(dir) => dir,
    };

    let mut output: Vec<ObjectSummary> = Vec::with_capacity(k + 1);
    let mut peak_slots: usize = 0;
    let mut branch_components: Vec<String> = Vec::new();
    let mut branch_path = parsed.base_path.clone();
    let mut frames = vec![Frame {
        comp_len_before: 0,
        path_len_before: branch_path.len(),
        last_label: String::new(),
        need_rescan: true,
    }];

    while let Some(should_scan) = frames
        .last()
        .map(|frame| frame.need_rescan && output.len() < k + 1)
    {
        if !should_scan {
            let popped = frames.pop().expect("frame checked");
            branch_components.truncate(popped.comp_len_before);
            branch_path.truncate(popped.path_len_before);
            continue;
        }

        ledger.check_deadline()?;
        let rel_refs: Vec<&str> = branch_components.iter().map(String::as_str).collect();
        let Some(active_dir) = base_dir.reopen_branch(&rel_refs)? else {
            let popped = frames.pop().expect("frame present");
            branch_components.truncate(popped.comp_len_before);
            branch_path.truncate(popped.path_len_before);
            continue;
        };

        let capacity = (k + 1) - output.len();
        let last_label_snapshot = frames.last().expect("frame present").last_label.clone();
        let mut window: VecDeque<WindowEntry> = VecDeque::with_capacity(capacity);
        let mut later = false;

        {
            let mut scanner = active_dir.open_scanner()?;
            while let Some(entry) = scanner.next_entry()? {
                ledger.charge_entry()?;
                let (label, is_dir) = match entry {
                    ScannedEntry::Excluded => continue,
                    ScannedEntry::Unsupported(reason) => return Err(listing_other_error(reason)),
                    ScannedEntry::File(name) => {
                        if branch_path.len() + name.len() > namespace::MAX_KEY_BYTES {
                            return Err(listing_other_error(
                                "object key exceeds 1024 UTF-8 bytes during listing",
                            ));
                        }
                        (name, false)
                    }
                    ScannedEntry::Directory(name) => {
                        if branch_path.len() + name.len() + 1 > namespace::MAX_KEY_BYTES {
                            return Err(listing_other_error(
                                "directory prefix exceeds 1024 UTF-8 bytes during listing",
                            ));
                        }
                        let mut dir_label = String::with_capacity(name.len() + 1);
                        dir_label.push_str(&name);
                        dir_label.push('/');
                        (dir_label, true)
                    }
                };

                if label <= last_label_snapshot {
                    continue;
                }
                let full_key_or_prefix = format!("{branch_path}{label}");
                let relevant = full_key_or_prefix.starts_with(raw_prefix)
                    || (is_dir && raw_prefix.starts_with(&full_key_or_prefix));
                if !relevant {
                    continue;
                }
                let resumable = after.is_empty()
                    || full_key_or_prefix.as_str() > after.as_str()
                    || (is_dir && after.starts_with(&full_key_or_prefix));
                if !resumable {
                    continue;
                }

                if window.len() == capacity {
                    later = true;
                    if label.as_str() >= window.back().expect("non-empty window").label.as_str() {
                        continue;
                    }
                    window.pop_back();
                }
                let insert_at = window.partition_point(|item| item.label < label);
                window.insert(insert_at, WindowEntry { label, is_dir });
                peak_slots = peak_slots.max(output.len() + window.len());
            }
        }

        if window.is_empty() {
            frames.last_mut().expect("frame present").need_rescan = false;
            continue;
        }

        let mut child_descent: Option<String> = None;
        while let Some(item) = window.pop_front() {
            ledger.check_deadline()?;
            frames.last_mut().expect("frame present").last_label = item.label.clone();
            if item.is_dir {
                let more_siblings = later || !window.is_empty();
                frames.last_mut().expect("frame present").need_rescan = more_siblings;
                window.clear();
                child_descent = Some(item.label);
                break;
            }

            match active_dir.stat_candidate_file(&item.label)? {
                CandidateStat::Missing => continue,
                CandidateStat::NotRegular => {
                    return Err(listing_other_error(
                        "directory entry changed to non-regular type during listing",
                    ));
                }
                CandidateStat::File {
                    size,
                    last_modified,
                } => {
                    output.push(ObjectSummary {
                        path: format!("{branch_path}{}", item.label),
                        size,
                        etag: None,
                        content_type: None,
                        last_modified,
                    });
                    peak_slots = peak_slots.max(output.len() + window.len());
                    if output.len() == k + 1 {
                        break;
                    }
                }
            }
        }

        drop(active_dir);

        if let Some(dir_label) = child_descent {
            let comp_len_before = branch_components.len();
            let path_len_before = branch_path.len();
            let dir_name = dir_label
                .strip_suffix('/')
                .expect("directory label ends with slash")
                .to_string();
            branch_components.push(dir_name);
            branch_path.push_str(&dir_label);
            frames.push(Frame {
                comp_len_before,
                path_len_before,
                last_label: String::new(),
                need_rescan: true,
            });
        } else {
            frames.last_mut().expect("frame present").need_rescan = later && output.len() < k + 1;
        }
    }

    ledger.check_deadline()?;
    ledger.check_deadline()?;
    let is_truncated = output.len() > k;
    if is_truncated {
        output.truncate(k);
    }
    let continuation_token = if is_truncated {
        let cursor_key = &output.last().expect("k >= 1").path;
        Some(encode_token(
            root.root_id(),
            TOKEN_MODE_FLAT,
            raw_prefix,
            "",
            k as u32,
            cursor_key,
        )?)
    } else {
        None
    };

    Ok((
        ListResult {
            objects: output,
            continuation_token,
            is_truncated,
        },
        ListStats {
            examined: ledger.examined,
            peak_slots,
        },
    ))
}

pub(crate) fn list_delimited(
    root: &platform::Root,
    request: &DelimiterListRequest,
    limits: ListLimits,
) -> Result<(DelimiterListResult, ListStats)> {
    let ledger_init = Ledger::new(limits);
    if request.delimiter != "/" {
        return Err(StorageError::InvalidArgument {
            operation: Some(StorageOperation::List),
            argument: "delimiter".into(),
            reason: "local delimiter listing supports only '/'".into(),
        });
    }
    let k = resolve_max_keys(request.max_keys)?;
    let raw_prefix = request.prefix.as_deref().unwrap_or("");
    let parsed = namespace::parse_list_prefix(request.prefix.as_deref())?;

    let after_label = match request.continuation_token.as_deref() {
        Some(token) => {
            let cursor = decode_token(
                token,
                root.root_id(),
                TOKEN_MODE_DELIMITED_SLASH,
                raw_prefix,
                "/",
                k as u32,
            )?;
            validate_delimited_cursor(&cursor, raw_prefix, &parsed.base_path)?;
            cursor[parsed.base_path.len()..].to_string()
        }
        None => String::new(),
    };

    let mut ledger = ledger_init;
    ledger.check_deadline()?;
    let resolved_base = root.resolve_list_base(&parsed.dir_components, &mut ledger)?;
    #[cfg(test)]
    {
        let delay_ms = TEST_POST_RESOLVE_DELAY_MS.load(std::sync::atomic::Ordering::Relaxed);
        if delay_ms > 0 {
            std::thread::sleep(Duration::from_millis(delay_ms));
        }
    }
    ledger.check_deadline()?;
    let base_dir = match resolved_base {
        BaseDir::Empty => {
            return Ok((
                DelimiterListResult {
                    objects: Vec::new(),
                    common_prefixes: Vec::new(),
                    continuation_token: None,
                    is_truncated: false,
                },
                ListStats {
                    examined: ledger.examined,
                    peak_slots: 0,
                },
            ));
        }
        BaseDir::Ready(dir) => dir,
    };

    enum EmittedItem {
        Object(ObjectSummary),
        Prefix(String),
    }

    impl EmittedItem {
        fn cursor_key(&self) -> &str {
            match self {
                Self::Object(obj) => &obj.path,
                Self::Prefix(pfx) => pfx,
            }
        }
    }

    let mut emitted: Vec<EmittedItem> = Vec::with_capacity(k + 1);
    let mut peak_slots: usize = 0;
    let mut last_label = after_label;

    while emitted.len() < k + 1 {
        ledger.check_deadline()?;
        let capacity = (k + 1) - emitted.len();
        let mut window: VecDeque<WindowEntry> = VecDeque::with_capacity(capacity);
        let mut later = false;

        {
            let mut scanner = base_dir.open_scanner()?;
            while let Some(entry) = scanner.next_entry()? {
                ledger.charge_entry()?;
                let (label, is_dir) = match entry {
                    ScannedEntry::Excluded => continue,
                    ScannedEntry::Unsupported(reason) => return Err(listing_other_error(reason)),
                    ScannedEntry::File(name) => {
                        if parsed.base_path.len() + name.len() > namespace::MAX_KEY_BYTES {
                            return Err(listing_other_error(
                                "object key exceeds 1024 UTF-8 bytes during delimiter listing",
                            ));
                        }
                        (name, false)
                    }
                    ScannedEntry::Directory(name) => {
                        if parsed.base_path.len() + name.len() + 1 > namespace::MAX_KEY_BYTES {
                            return Err(listing_other_error(
                                "common prefix exceeds 1024 UTF-8 bytes during delimiter listing",
                            ));
                        }
                        let mut dir_label = String::with_capacity(name.len() + 1);
                        dir_label.push_str(&name);
                        dir_label.push('/');
                        (dir_label, true)
                    }
                };

                if label <= last_label || !label.starts_with(parsed.partial) {
                    continue;
                }

                if window.len() == capacity {
                    later = true;
                    if label.as_str() >= window.back().expect("non-empty window").label.as_str() {
                        continue;
                    }
                    window.pop_back();
                }
                let insert_at = window.partition_point(|item| item.label < label);
                window.insert(insert_at, WindowEntry { label, is_dir });
                peak_slots = peak_slots.max(emitted.len() + window.len());
            }
        }

        if window.is_empty() {
            break;
        }

        while let Some(item) = window.pop_front() {
            ledger.check_deadline()?;
            last_label = item.label.clone();
            if item.is_dir {
                emitted.push(EmittedItem::Prefix(format!(
                    "{}{}",
                    parsed.base_path, item.label
                )));
                peak_slots = peak_slots.max(emitted.len() + window.len());
            } else {
                match base_dir.stat_candidate_file(&item.label)? {
                    CandidateStat::Missing => continue,
                    CandidateStat::NotRegular => {
                        return Err(listing_other_error(
                            "directory entry changed to non-regular type during delimiter listing",
                        ));
                    }
                    CandidateStat::File {
                        size,
                        last_modified,
                    } => {
                        emitted.push(EmittedItem::Object(ObjectSummary {
                            path: format!("{}{}", parsed.base_path, item.label),
                            size,
                            etag: None,
                            content_type: None,
                            last_modified,
                        }));
                        peak_slots = peak_slots.max(emitted.len() + window.len());
                    }
                }
            }
            if emitted.len() == k + 1 {
                break;
            }
        }

        if !later {
            break;
        }
    }

    ledger.check_deadline()?;
    let is_truncated = emitted.len() > k;
    if is_truncated {
        emitted.truncate(k);
    }
    let continuation_token = if is_truncated {
        let cursor = emitted.last().expect("k >= 1").cursor_key();
        Some(encode_token(
            root.root_id(),
            TOKEN_MODE_DELIMITED_SLASH,
            raw_prefix,
            "/",
            k as u32,
            cursor,
        )?)
    } else {
        None
    };

    let mut objects = Vec::new();
    let mut common_prefixes = Vec::new();
    for item in emitted {
        match item {
            EmittedItem::Object(obj) => objects.push(obj),
            EmittedItem::Prefix(pfx) => common_prefixes.push(pfx),
        }
    }

    Ok((
        DelimiterListResult {
            objects,
            common_prefixes,
            continuation_token,
            is_truncated,
        },
        ListStats {
            examined: ledger.examined,
            peak_slots,
        },
    ))
}

fn validate_delimited_cursor(cursor: &str, raw_prefix: &str, base_path: &str) -> Result<()> {
    if cursor.is_empty()
        || cursor.len() > namespace::MAX_KEY_BYTES
        || !cursor.starts_with(raw_prefix)
        || !cursor.starts_with(base_path)
    {
        return Err(invalid_continuation_token());
    }
    let tail = &cursor[base_path.len()..];
    if let Some(dir_name) = tail.strip_suffix('/') {
        if !namespace::is_representable_segment(dir_name)
            || namespace::is_reserved_segment(dir_name)
        {
            return Err(invalid_continuation_token());
        }
        Ok(())
    } else {
        if tail.contains('/') {
            return Err(invalid_continuation_token());
        }
        namespace::validate(cursor, StorageOperation::List)
            .map_err(|_| invalid_continuation_token())
    }
}

fn listing_other_error(detail: &'static str) -> StorageError {
    StorageError::Other {
        provider: Some(ProviderKind::Local),
        operation: Some(StorageOperation::List),
        detail: detail.into(),
        source: None,
    }
}

fn invalid_continuation_token() -> StorageError {
    StorageError::InvalidArgument {
        operation: Some(StorageOperation::List),
        argument: "continuation_token".into(),
        reason: "invalid, tampered, or out-of-scope continuation token".into(),
    }
}

fn token_secret() -> Result<&'static [u8; 32]> {
    static SECRET: OnceLock<[u8; 32]> = OnceLock::new();
    if let Some(secret) = SECRET.get() {
        return Ok(secret);
    }
    let mut generated = [0_u8; 32];
    getrandom::fill(&mut generated).map_err(|_| StorageError::Io {
        operation: Some(StorageOperation::List),
        source: std::io::Error::other("system random source unavailable"),
    })?;
    let _ = SECRET.set(generated);
    Ok(SECRET.get().expect("initialized"))
}

fn encode_token(
    root_id: u128,
    mode: u8,
    prefix: &str,
    delimiter: &str,
    resolved_max_keys: u32,
    cursor: &str,
) -> Result<String> {
    let tag = compute_token_tag(
        token_secret()?,
        root_id,
        mode,
        prefix,
        delimiter,
        resolved_max_keys,
        cursor.as_bytes(),
    );
    let raw_len = 2 + 16 + cursor.len();
    let mut raw = Vec::with_capacity(raw_len);
    raw.push(TOKEN_VERSION_V1);
    raw.push(mode);
    raw.extend_from_slice(&tag);
    raw.extend_from_slice(cursor.as_bytes());

    let mut hex = String::with_capacity(raw.len() * 2);
    for byte in raw {
        use std::fmt::Write as _;
        let _ = write!(&mut hex, "{byte:02x}");
    }
    Ok(hex)
}

fn decode_token(
    token: &str,
    root_id: u128,
    expected_mode: u8,
    prefix: &str,
    delimiter: &str,
    resolved_max_keys: u32,
) -> Result<String> {
    if token.is_empty() || token.len() > MAX_TOKEN_BYTES || !token.len().is_multiple_of(2) {
        return Err(invalid_continuation_token());
    }
    let mut raw = Vec::with_capacity(token.len() / 2);
    let bytes = token.as_bytes();
    for pair in bytes.chunks_exact(2) {
        let hi = decode_hex_nibble(pair[0]).ok_or_else(invalid_continuation_token)?;
        let lo = decode_hex_nibble(pair[1]).ok_or_else(invalid_continuation_token)?;
        raw.push((hi << 4) | lo);
    }
    if raw.len() <= 18 || raw[0] != TOKEN_VERSION_V1 || raw[1] != expected_mode {
        return Err(invalid_continuation_token());
    }
    let provided_tag = &raw[2..18];
    let cursor_bytes = &raw[18..];
    let expected_tag = compute_token_tag(
        token_secret()?,
        root_id,
        expected_mode,
        prefix,
        delimiter,
        resolved_max_keys,
        cursor_bytes,
    );
    // Constant-time tag comparison.
    let mut diff = 0_u8;
    for (lhs, rhs) in provided_tag.iter().zip(expected_tag.iter()) {
        diff |= std::hint::black_box(*lhs ^ *rhs);
    }
    if std::hint::black_box(diff) != 0 {
        return Err(invalid_continuation_token());
    }
    String::from_utf8(cursor_bytes.to_vec()).map_err(|_| invalid_continuation_token())
}

fn decode_hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

/// Computes a 128-bit SipHash-2-4 tag (two independent 64-bit keyed instances)
/// over length-delimited scope fields and cursor bytes.
fn compute_token_tag(
    secret: &[u8; 32],
    root_id: u128,
    mode: u8,
    prefix: &str,
    delimiter: &str,
    resolved_max_keys: u32,
    cursor_bytes: &[u8],
) -> [u8; 16] {
    let mut msg = Vec::with_capacity(
        1 + 16 + 1 + 4 + 4 + prefix.len() + 4 + delimiter.len() + 4 + cursor_bytes.len(),
    );
    msg.push(TOKEN_VERSION_V1);
    msg.extend_from_slice(&root_id.to_le_bytes());
    msg.push(mode);
    msg.extend_from_slice(&resolved_max_keys.to_le_bytes());
    msg.extend_from_slice(&(prefix.len() as u32).to_le_bytes());
    msg.extend_from_slice(prefix.as_bytes());
    msg.extend_from_slice(&(delimiter.len() as u32).to_le_bytes());
    msg.extend_from_slice(delimiter.as_bytes());
    msg.extend_from_slice(&(cursor_bytes.len() as u32).to_le_bytes());
    msg.extend_from_slice(cursor_bytes);

    let k0_a = u64::from_le_bytes(secret[0..8].try_into().unwrap());
    let k1_a = u64::from_le_bytes(secret[8..16].try_into().unwrap());
    let k0_b = u64::from_le_bytes(secret[16..24].try_into().unwrap());
    let k1_b = u64::from_le_bytes(secret[24..32].try_into().unwrap());

    let tag_lo = siphash24(k0_a, k1_a, &msg);
    let tag_hi = siphash24(k0_b, k1_b, &msg);
    let mut out = [0_u8; 16];
    out[..8].copy_from_slice(&tag_lo.to_le_bytes());
    out[8..].copy_from_slice(&tag_hi.to_le_bytes());
    out
}

fn siphash24(k0: u64, k1: u64, data: &[u8]) -> u64 {
    let mut v0 = k0 ^ 0x736f6d6570736575;
    let mut v1 = k1 ^ 0x646f72616e646f6d;
    let mut v2 = k0 ^ 0x6c7967656e657261;
    let mut v3 = k1 ^ 0x7465646279746573;

    let mut chunks = data.chunks_exact(8);
    for chunk in &mut chunks {
        let m = u64::from_le_bytes(chunk.try_into().unwrap());
        v3 ^= m;
        sip_round(&mut v0, &mut v1, &mut v2, &mut v3);
        sip_round(&mut v0, &mut v1, &mut v2, &mut v3);
        v0 ^= m;
    }

    let rem = chunks.remainder();
    let mut last = ((data.len() & 0xff) as u64) << 56;
    for (idx, &byte) in rem.iter().enumerate() {
        last |= (byte as u64) << (idx * 8);
    }
    v3 ^= last;
    sip_round(&mut v0, &mut v1, &mut v2, &mut v3);
    sip_round(&mut v0, &mut v1, &mut v2, &mut v3);
    v0 ^= last;

    v2 ^= 0xff;
    for _ in 0..4 {
        sip_round(&mut v0, &mut v1, &mut v2, &mut v3);
    }
    v0 ^ v1 ^ v2 ^ v3
}

#[inline]
fn sip_round(v0: &mut u64, v1: &mut u64, v2: &mut u64, v3: &mut u64) {
    *v0 = v0.wrapping_add(*v1);
    *v1 = v1.rotate_left(13);
    *v1 ^= *v0;
    *v0 = v0.rotate_left(32);
    *v2 = v2.wrapping_add(*v3);
    *v3 = v3.rotate_left(16);
    *v3 ^= *v2;
    *v0 = v0.wrapping_add(*v3);
    *v3 = v3.rotate_left(21);
    *v3 ^= *v0;
    *v2 = v2.wrapping_add(*v1);
    *v1 = v1.rotate_left(17);
    *v1 ^= *v2;
    *v2 = v2.rotate_left(32);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_roundtrip_and_cross_scope_or_tamper_rejection() {
        let token =
            encode_token(42, TOKEN_MODE_FLAT, "reports/", "", 1000, "reports/a.txt").unwrap();
        assert!(token.len() <= MAX_TOKEN_BYTES);
        assert_eq!(
            decode_token(&token, 42, TOKEN_MODE_FLAT, "reports/", "", 1000).unwrap(),
            "reports/a.txt"
        );

        // Cross-root, cross-mode, cross-prefix, cross-delimiter, cross-page-size must all fail.
        assert!(decode_token(&token, 43, TOKEN_MODE_FLAT, "reports/", "", 1000).is_err());
        assert!(decode_token(
            &token,
            42,
            TOKEN_MODE_DELIMITED_SLASH,
            "reports/",
            "/",
            1000
        )
        .is_err());
        assert!(decode_token(&token, 42, TOKEN_MODE_FLAT, "other/", "", 1000).is_err());
        assert!(decode_token(&token, 42, TOKEN_MODE_FLAT, "reports/", "/", 1000).is_err());
        assert!(decode_token(&token, 42, TOKEN_MODE_FLAT, "reports/", "", 500).is_err());

        // Single-bit / single-nibble tamper must fail.
        let mut tampered = token.into_bytes();
        let last_idx = tampered.len() - 1;
        tampered[last_idx] = if tampered[last_idx] == b'0' {
            b'1'
        } else {
            b'0'
        };
        let tampered_str = String::from_utf8(tampered).unwrap();
        assert!(decode_token(&tampered_str, 42, TOKEN_MODE_FLAT, "reports/", "", 1000).is_err());

        // Oversized (>8192 bytes) and uppercase/non-hex tokens fail before walking.
        assert!(decode_token(
            &"a".repeat(MAX_TOKEN_BYTES + 2),
            42,
            TOKEN_MODE_FLAT,
            "",
            "",
            1000
        )
        .is_err());
        assert!(decode_token("ABCDEF", 42, TOKEN_MODE_FLAT, "", "", 1000).is_err());
    }

    #[test]
    fn delayed_missing_prefix_resolution_enforces_deadline_before_empty_return() {
        struct DelayGuard;
        impl Drop for DelayGuard {
            fn drop(&mut self) {
                TEST_POST_RESOLVE_DELAY_MS.store(0, std::sync::atomic::Ordering::Relaxed);
            }
        }

        let dir = std::env::temp_dir().join(format!(
            "storageprims-list-deadline-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let root = platform::Root::open(&dir).unwrap();

        let _guard = DelayGuard;
        TEST_POST_RESOLVE_DELAY_MS.store(25, std::sync::atomic::Ordering::Relaxed);
        let limits = ListLimits {
            max_entries: DEFAULT_ENTRY_BUDGET,
            max_duration: Duration::from_millis(5),
        };

        let flat_err = list_flat(
            &root,
            &ListOptions {
                prefix: Some("missing/nested/".into()),
                ..ListOptions::default()
            },
            limits,
        )
        .unwrap_err();
        assert!(matches!(
            flat_err,
            StorageError::Inspection {
                kind: InspectionKind::LimitExceeded,
                limit_dimension: Some(InspectionLimitDimension::Deadline),
                ..
            }
        ));

        let delim_err = list_delimited(
            &root,
            &DelimiterListRequest {
                prefix: Some("missing/nested/".into()),
                delimiter: "/".into(),
                continuation_token: None,
                max_keys: None,
            },
            limits,
        )
        .unwrap_err();
        assert!(matches!(
            delim_err,
            StorageError::Inspection {
                kind: InspectionKind::LimitExceeded,
                limit_dimension: Some(InspectionLimitDimension::Deadline),
                ..
            }
        ));

        drop(root);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
