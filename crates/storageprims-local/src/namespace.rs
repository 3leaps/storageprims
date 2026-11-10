//! Lexical key, prefix, and reserved temporary-namespace rules.
//!
//! Lexical checks precede filesystem access; they are not containment proof.

use std::sync::atomic::{AtomicU64, Ordering};
use storageprims_core::{Result, StorageError, StorageOperation};

pub(crate) const MAX_KEY_BYTES: usize = 1024;
pub(crate) const RESERVED_TEMP_PREFIX: &str = ".storageprims-tmp";

static NEXT_TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedPrefix<'a> {
    pub dir_components: Vec<&'a str>,
    pub base_path: String,
    pub partial: &'a str,
}

pub(crate) fn validate(key: &str, operation: StorageOperation) -> Result<()> {
    validate_arg(key, "key", operation)
}

pub(crate) fn validate_arg(key: &str, argument: &str, operation: StorageOperation) -> Result<()> {
    if key.is_empty()
        || key.len() > MAX_KEY_BYTES
        || key.starts_with('/')
        || key.contains('\0')
        || key.split('/').any(|part| {
            part.is_empty()
                || part == "."
                || part == ".."
                || is_reserved_segment(part)
                || (cfg!(windows) && windows_invalid(part))
        })
    {
        return Err(StorageError::InvalidArgument {
            operation: Some(operation),
            argument: argument.into(),
            reason:
                "requires a representable non-reserved root-relative key of 1..=1024 UTF-8 bytes"
                    .into(),
        });
    }
    Ok(())
}

/// Returns `true` when `part` falls inside the provider-reserved temporary namespace
/// under supported native volume name equality (ASCII and Unicode simple case folding
/// plus Windows trailing dot/space trimming).
pub(crate) fn is_reserved_segment(part: &str) -> bool {
    let trimmed = part.trim_end_matches(['.', ' ']);
    let mut candidate = trimmed.chars().map(fold_native_char);
    for expected in RESERVED_TEMP_PREFIX.chars() {
        if candidate.next() != Some(expected) {
            return false;
        }
    }
    true
}

#[inline]
fn fold_native_char(ch: char) -> char {
    match ch {
        'A'..='Z' => ch.to_ascii_lowercase(),
        '\u{017f}' => 's', // Latin small letter long s (APFS / Unicode simple fold)
        '\u{212a}' => 'k', // Kelvin sign (Unicode simple fold)
        '\u{0130}' | '\u{0131}' => 'i', // Dotted / dotless I
        other => other,
    }
}

pub(crate) fn is_representable_segment(part: &str) -> bool {
    !part.is_empty()
        && part != "."
        && part != ".."
        && !part.contains('\0')
        && !part.contains('/')
        && (!cfg!(windows) || !windows_invalid(part))
}

pub(crate) fn parse_list_prefix(prefix: Option<&str>) -> Result<ParsedPrefix<'_>> {
    let raw = prefix.unwrap_or("");
    if raw.is_empty() {
        return Ok(ParsedPrefix {
            dir_components: Vec::new(),
            base_path: String::new(),
            partial: "",
        });
    }
    if raw.len() > MAX_KEY_BYTES || raw.contains('\0') || raw.starts_with('/') {
        return Err(invalid_prefix());
    }
    if let Some((dir_chain, partial)) = raw.rsplit_once('/') {
        if dir_chain.is_empty() {
            return Err(invalid_prefix());
        }
        let mut dir_components = Vec::new();
        for comp in dir_chain.split('/') {
            if comp.is_empty()
                || comp == "."
                || comp == ".."
                || is_reserved_segment(comp)
                || (cfg!(windows) && windows_invalid(comp))
            {
                return Err(invalid_prefix());
            }
            dir_components.push(comp);
        }
        if cfg!(windows)
            && !partial.is_empty()
            && partial.chars().any(|c| c < ' ' || "\\:<>\"|?*".contains(c))
        {
            return Err(invalid_prefix());
        }
        let mut base_path = String::with_capacity(dir_chain.len() + 1);
        base_path.push_str(dir_chain);
        base_path.push('/');
        Ok(ParsedPrefix {
            dir_components,
            base_path,
            partial,
        })
    } else {
        if cfg!(windows) && raw.chars().any(|c| c < ' ' || "\\:<>\"|?*".contains(c)) {
            return Err(invalid_prefix());
        }
        Ok(ParsedPrefix {
            dir_components: Vec::new(),
            base_path: String::new(),
            partial: raw,
        })
    }
}

fn invalid_prefix() -> StorageError {
    StorageError::InvalidArgument {
        operation: Some(StorageOperation::List),
        argument: "prefix".into(),
        reason:
            "list prefix must be a valid root-relative literal prefix of at most 1024 UTF-8 bytes"
                .into(),
    }
}

pub(crate) fn generate_temp_name(operation: StorageOperation) -> Result<String> {
    let mut random = [0_u8; 16];
    getrandom::fill(&mut random).map_err(|_| StorageError::Io {
        operation: Some(operation),
        source: std::io::Error::other("system random source unavailable"),
    })?;
    let seq = NEXT_TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let mut hex = String::with_capacity(RESERVED_TEMP_PREFIX.len() + 1 + 32 + 1 + 20);
    hex.push_str(RESERVED_TEMP_PREFIX);
    hex.push('-');
    for byte in random {
        use std::fmt::Write as _;
        let _ = write!(&mut hex, "{byte:02x}");
    }
    {
        use std::fmt::Write as _;
        let _ = write!(&mut hex, "-{seq:x}");
    }
    Ok(hex)
}

pub(crate) fn format_rfc3339_utc(unix_secs: i64, _subsec_nanos: u32) -> Option<String> {
    if !(-62_167_219_200..=253_402_300_799).contains(&unix_secs) {
        return None;
    }
    let days = unix_secs.div_euclid(86_400);
    let secs_of_day = unix_secs.rem_euclid(86_400);
    let hour = secs_of_day / 3_600;
    let minute = (secs_of_day % 3_600) / 60;
    let second = secs_of_day % 60;

    // Howard Hinnant's civil_from_days algorithm (proleptic Gregorian calendar).
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let mut year = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    if month <= 2 {
        year += 1;
    }
    if !(0..=9999).contains(&year) {
        return None;
    }
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z"
    ))
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn format_windows_filetime(filetime_100ns: i64) -> Option<String> {
    const EPOCH_DIFF_100NS: i64 = 116_444_736_000_000_000;
    if filetime_100ns <= 0 {
        return None;
    }
    let unix_100ns = filetime_100ns.checked_sub(EPOCH_DIFF_100NS)?;
    let unix_secs = unix_100ns.div_euclid(10_000_000);
    let nanos = (unix_100ns.rem_euclid(10_000_000) * 100) as u32;
    format_rfc3339_utc(unix_secs, nanos)
}

fn windows_invalid(part: &str) -> bool {
    if part.ends_with(['.', ' ']) || part.chars().any(|c| c < ' ' || "\\:<>\"|?*".contains(c)) {
        return true;
    }
    let base = part
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    matches!(
        base.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || ["COM", "LPT"].iter().any(|prefix| {
        base.strip_prefix(prefix).is_some_and(|suffix| {
            matches!(
                suffix,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsafe_grammar_and_counts_utf8_bytes_not_characters() {
        for key in [
            "", "/a", "a/", "a//b", ".", "../a", "a/../b", "a/./b", "a\0b",
        ] {
            assert!(validate(key, StorageOperation::Put).is_err(), "{key:?}");
        }
        assert!(validate(&"é".repeat(512), StorageOperation::Put).is_ok());
        assert!(validate(&format!("{}a", "é".repeat(512)), StorageOperation::Put).is_err());
        assert!(validate("a-b/a.txt", StorageOperation::Get).is_ok());
        assert!(validate(".gitignore", StorageOperation::Get).is_ok());
    }

    #[test]
    fn reserved_temp_namespace_rejects_case_unicode_and_trailing_aliases() {
        for reserved in [
            ".storageprims-tmp",
            ".storageprims-tmp-0123abcdef",
            ".STORAGEPRIMS-TMP-0123ABCDEF",
            ".\u{017f}torageprim\u{017f}-tmp-99",
            ".storagepr\u{0130}ms-tmp-99",
            ".storagepr\u{0131}ms-tmp-99",
            ".storageprims-tmp...",
            ".storageprims-tmp   ",
            "nested/.Storageprims-Tmp-42/file.bin",
            "nested/.storageprims-tmp",
        ] {
            assert!(
                validate(reserved, StorageOperation::Put).is_err(),
                "expected reserved rejection for {reserved:?}"
            );
        }
        for allowed in [
            ".storage",
            ".storageprims",
            "storageprims-tmp-123",
            "nested/.storageprims-other",
        ] {
            assert!(
                validate(allowed, StorageOperation::Put).is_ok(),
                "expected {allowed:?} to remain caller-usable"
            );
        }
    }

    #[test]
    fn list_prefix_parsing_distinguishes_complete_directories_from_partial_segment() {
        let root = parse_list_prefix(None).unwrap();
        assert!(root.dir_components.is_empty());
        assert_eq!(root.base_path, "");
        assert_eq!(root.partial, "");

        let partial_only = parse_list_prefix(Some(".git")).unwrap();
        assert!(partial_only.dir_components.is_empty());
        assert_eq!(partial_only.base_path, "");
        assert_eq!(partial_only.partial, ".git");

        let dir_boundary = parse_list_prefix(Some("reports/2026/")).unwrap();
        assert_eq!(dir_boundary.dir_components, vec!["reports", "2026"]);
        assert_eq!(dir_boundary.base_path, "reports/2026/");
        assert_eq!(dir_boundary.partial, "");

        let dir_and_partial = parse_list_prefix(Some("reports/2026/oct")).unwrap();
        assert_eq!(dir_and_partial.dir_components, vec!["reports", "2026"]);
        assert_eq!(dir_and_partial.base_path, "reports/2026/");
        assert_eq!(dir_and_partial.partial, "oct");

        for invalid in [
            "/",
            "/a",
            "a//b",
            "./a",
            "../a",
            "a/./b",
            "a/../b",
            ".storageprims-tmp/a",
            "a/.STORAGEPRIMS-TMP-1/b",
            "a\0b",
        ] {
            assert!(
                parse_list_prefix(Some(invalid)).is_err(),
                "expected invalid prefix {invalid:?}"
            );
        }
    }

    #[test]
    fn generated_temp_names_are_reserved_and_distinct() {
        let first = generate_temp_name(StorageOperation::Put).unwrap();
        let second = generate_temp_name(StorageOperation::Put).unwrap();
        assert_ne!(first, second);
        assert!(is_reserved_segment(&first));
        assert!(is_reserved_segment(&second));
    }

    #[test]
    fn rfc3339_and_filetime_formatting_match_known_utc_timestamps() {
        assert_eq!(
            format_rfc3339_utc(0, 0).as_deref(),
            Some("1970-01-01T00:00:00Z")
        );
        assert_eq!(
            format_rfc3339_utc(951_827_696, 0).as_deref(),
            Some("2000-02-29T12:34:56Z")
        );
        assert_eq!(
            format_windows_filetime(116_444_736_000_000_000).as_deref(),
            Some("1970-01-01T00:00:00Z")
        );
    }

    #[test]
    fn windows_name_rules_are_testable_without_claiming_windows_native_support() {
        for part in [
            "CON",
            "con.txt",
            "COM1",
            "LPT9.log",
            "aux",
            "NUL",
            "a:",
            "a\\b",
            "a.",
            "a ",
            "a*",
            "COM¹.txt",
        ] {
            assert!(windows_invalid(part), "{part}");
        }
        for part in ["console", "COM10", "LPT0", "résumé.txt", "sample.bin"] {
            assert!(!windows_invalid(part));
        }
    }
}
