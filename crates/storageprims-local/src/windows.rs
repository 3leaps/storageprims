//! Windows handle-relative traversal, staging, and directory scanning anchored to opened directory handles.

use crate::{
    namespace, non_regular_object_error, os_error, os_error_key, symlink_refused_error, BaseDir,
    CandidateStat, ScannedEntry,
};
use std::{
    collections::BTreeMap,
    ffi::c_void,
    fs::File,
    mem::{align_of, offset_of, size_of, zeroed},
    ops::ControlFlow,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::Path,
    ptr::{null, null_mut},
};
use storageprims_core::{
    CommitKnowledge, ConflictKind, ObjectMetadata, ProviderKind, Result, StorageError,
    StorageOperation,
};
use windows_sys::{
    Wdk::{
        Foundation::OBJECT_ATTRIBUTES,
        Storage::FileSystem::{
            FileIdBothDirectoryInformation, FileRenameInformation, FileRenameInformationEx,
            NtCreateFile, NtQueryDirectoryFile, NtSetInformationFile, FILE_CREATE,
            FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_IF,
            FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT,
        },
    },
    Win32::{
        Foundation::{
            GetLastError, RtlNtStatusToDosError, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS,
            ERROR_DIRECTORY, ERROR_FILE_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_INVALID_FUNCTION,
            ERROR_INVALID_PARAMETER, ERROR_NOT_SUPPORTED, ERROR_NO_MORE_FILES,
            ERROR_PATH_NOT_FOUND, HANDLE, INVALID_HANDLE_VALUE, NTSTATUS, OBJ_CASE_INSENSITIVE,
            STATUS_DELETE_PENDING, STATUS_FILE_IS_A_DIRECTORY, STATUS_IO_REPARSE_TAG_NOT_HANDLED,
            STATUS_NOT_A_DIRECTORY, STATUS_NO_SUCH_FILE, STATUS_OBJECT_NAME_COLLISION,
            STATUS_OBJECT_NAME_NOT_FOUND, STATUS_OBJECT_PATH_NOT_FOUND, STATUS_STOPPED_ON_SYMLINK,
            UNICODE_STRING,
        },
        Storage::FileSystem::{
            CreateFileW, FileAttributeTagInfo, FileBasicInfo, FileDispositionInfo,
            FileDispositionInfoEx, FileIdBothDirectoryInfo, FileIdBothDirectoryRestartInfo,
            FileIdInfo, FileStandardInfo, FlushFileBuffers, GetFileInformationByHandleEx,
            GetFileType, GetFinalPathNameByHandleW, SetFileInformationByHandle, DELETE,
            FILE_ADD_FILE, FILE_ADD_SUBDIRECTORY, FILE_ATTRIBUTE_DEVICE, FILE_ATTRIBUTE_DIRECTORY,
            FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO,
            FILE_BASIC_INFO, FILE_DISPOSITION_FLAG_DELETE, FILE_DISPOSITION_FLAG_POSIX_SEMANTICS,
            FILE_DISPOSITION_INFO, FILE_DISPOSITION_INFO_EX, FILE_FLAG_BACKUP_SEMANTICS,
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
            FILE_ID_BOTH_DIR_INFO, FILE_ID_INFO, FILE_LIST_DIRECTORY, FILE_NAME_NORMALIZED,
            FILE_READ_ATTRIBUTES, FILE_RENAME_INFO, FILE_RENAME_INFO_0, FILE_SHARE_DELETE,
            FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_STANDARD_INFO, FILE_TRAVERSE, FILE_TYPE_DISK,
            OPEN_EXISTING, SYNCHRONIZE, VOLUME_NAME_NONE,
        },
        System::{
            WindowsProgramming::{
                FILE_RENAME_FLAG_POSIX_SEMANTICS, FILE_RENAME_FLAG_REPLACE_IF_EXISTS,
            },
            IO::IO_STATUS_BLOCK,
        },
    },
};

const DIR_TRAVERSE_ACCESS: u32 =
    FILE_LIST_DIRECTORY | FILE_TRAVERSE | FILE_READ_ATTRIBUTES | SYNCHRONIZE;
const DIR_MUTATE_ACCESS: u32 = DIR_TRAVERSE_ACCESS | FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY;
const DIR_SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;

pub(crate) struct Root {
    directory: OwnedHandle,
    root_id: u128,
}

pub(crate) struct DirHandle(OwnedHandle);

pub(crate) struct DirScanner {
    handle: OwnedHandle,
    buffer: Vec<u64>,
    cursor_offset: Option<usize>,
    first_call: bool,
    exhausted: bool,
}

pub(crate) struct StagedPut {
    parent: OwnedHandle,
    temp_name: String,
    leaf_name: String,
    file: Option<File>,
    cleanup_handle: Option<OwnedHandle>,
    armed_cleanup: bool,
}

impl Root {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let raw_wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        let mut wide = sanitize_windows_root_wide(&raw_wide)?;
        wide.push(0);

        let raw = unsafe {
            CreateFileW(
                wide.as_ptr(),
                DIR_MUTATE_ACCESS,
                DIR_SHARE_ALL,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                null_mut(),
            )
        };
        let handle = if raw == INVALID_HANDLE_VALUE {
            let first_err = unsafe { GetLastError() };
            // Allow read-only root directories where FILE_ADD_FILE is denied at open time.
            if first_err == ERROR_ACCESS_DENIED {
                let ro_raw = unsafe {
                    CreateFileW(
                        wide.as_ptr(),
                        DIR_TRAVERSE_ACCESS,
                        DIR_SHARE_ALL,
                        null(),
                        OPEN_EXISTING,
                        FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                        null_mut(),
                    )
                };
                if ro_raw == INVALID_HANDLE_VALUE {
                    let err_code = unsafe { GetLastError() };
                    return Err(os_error(
                        std::io::Error::from_raw_os_error(err_code as i32),
                        StorageOperation::Probe,
                    ));
                }
                unsafe { OwnedHandle::from_raw_handle(ro_raw) }
            } else {
                return Err(os_error(
                    std::io::Error::from_raw_os_error(first_err as i32),
                    StorageOperation::Probe,
                ));
            }
        } else {
            unsafe { OwnedHandle::from_raw_handle(raw) }
        };

        verify_disk_directory_handle(&handle, StorageOperation::Probe)?;
        let root_id = query_root_id(&handle)?;
        Ok(Self {
            directory: handle,
            root_id,
        })
    }

    pub(crate) fn root_id(&self) -> u128 {
        self.root_id
    }

    pub(crate) fn probe_root(&self) -> Result<()> {
        verify_disk_directory_handle(&self.directory, StorageOperation::Probe)
    }

    pub(crate) fn parent(
        &self,
        key: &str,
        operation: StorageOperation,
    ) -> Result<(OwnedHandle, String)> {
        namespace::validate(key, operation)?;
        let mut parts = key.split('/').peekable();
        let mut current = self
            .directory
            .try_clone()
            .map_err(|error| os_error_key(error, operation, Some(key)))?;
        while let Some(part) = parts.next() {
            if parts.peek().is_none() {
                return Ok((current, part.to_string()));
            }
            current = nt_open_child_dir(&current, part, DIR_TRAVERSE_ACCESS, operation, Some(key))?;
        }
        unreachable!("validated key has a leaf")
    }

    pub(crate) fn open_read(
        &self,
        key: &str,
        operation: StorageOperation,
    ) -> Result<(File, ObjectMetadata)> {
        let (parent, leaf) = self.parent(key, operation)?;
        let handle = nt_open_leaf_file(
            &parent,
            &leaf,
            FILE_GENERIC_READ | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            DIR_SHARE_ALL,
            operation,
            key,
        )?;
        let meta = query_object_metadata(&handle, key, operation)?;
        Ok((File::from(handle), meta))
    }

    pub(crate) fn head_metadata(&self, key: &str) -> Result<ObjectMetadata> {
        let (parent, leaf) = self.parent(key, StorageOperation::Head)?;
        let handle = nt_open_leaf_file(
            &parent,
            &leaf,
            FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            DIR_SHARE_ALL,
            StorageOperation::Head,
            key,
        )?;
        query_object_metadata(&handle, key, StorageOperation::Head)
    }

    pub(crate) fn delete_object(&self, key: &str) -> Result<()> {
        namespace::validate(key, StorageOperation::Delete)?;
        let mut parts = key.split('/').peekable();
        let mut current = self
            .directory
            .try_clone()
            .map_err(|error| os_error_key(error, StorageOperation::Delete, Some(key)))?;
        let leaf = loop {
            let part = parts.next().expect("validated key has a leaf");
            if parts.peek().is_none() {
                break part;
            }
            match nt_try_open_child_dir(
                &current,
                part,
                DIR_TRAVERSE_ACCESS,
                StorageOperation::Delete,
                Some(key),
            ) {
                Ok(Some(next)) => current = next,
                Ok(None) => return Ok(()),
                Err(error) => return Err(error),
            }
        };

        let handle = match nt_try_open_leaf_for_delete(&current, leaf, key)? {
            Some(h) => h,
            None => return Ok(()),
        };
        mark_handle_deleted(&handle, key, StorageOperation::Delete)
    }

    pub(crate) fn create_staged_put(
        &self,
        key: &str,
        operation: StorageOperation,
    ) -> Result<StagedPut> {
        namespace::validate(key, operation)?;
        let parent_dir = self.ensure_parent_dirs(key, operation)?;
        let leaf_name = key
            .rsplit('/')
            .next()
            .expect("validated key has a leaf")
            .to_string();

        for _ in 0..8 {
            let temp_name = namespace::generate_temp_name(operation)?;
            match nt_create_exclusive_temp(&parent_dir, &temp_name, operation, key) {
                Ok(Some(handle)) => {
                    let cleanup_handle = match handle.try_clone() {
                        Ok(dup) => Some(dup),
                        Err(err) => {
                            let _ = mark_handle_deleted(&handle, &temp_name, operation);
                            return Err(os_error_key(err, operation, Some(key)));
                        }
                    };
                    return Ok(StagedPut {
                        parent: parent_dir,
                        temp_name,
                        leaf_name,
                        file: Some(File::from(handle)),
                        cleanup_handle,
                        armed_cleanup: true,
                    });
                }
                Ok(None) => continue,
                Err(error) => return Err(error),
            }
        }

        Err(StorageError::Io {
            operation: Some(operation),
            source: std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "unable to allocate unique staging entry",
            ),
        })
    }

    fn ensure_parent_dirs(&self, key: &str, operation: StorageOperation) -> Result<OwnedHandle> {
        let mut parts = key.split('/').peekable();
        let mut current = self
            .directory
            .try_clone()
            .map_err(|error| os_error_key(error, operation, Some(key)))?;
        while let Some(part) = parts.next() {
            if parts.peek().is_none() {
                if !key.contains('/') {
                    // Direct child of root: re-open "." with DIR_MUTATE_ACCESS in case root opened read-only.
                    return nt_open_self_dir(&current, DIR_MUTATE_ACCESS, operation, Some(key));
                }
                return Ok(current);
            }
            current = nt_open_or_create_child_dir(&current, part, operation, key)?;
        }
        Ok(current)
    }

    pub(crate) fn resolve_list_base(
        &self,
        components: &[&str],
        ledger: &mut crate::listing::Ledger,
    ) -> Result<BaseDir> {
        let mut current = self
            .directory
            .try_clone()
            .map_err(|error| os_error(error, StorageOperation::List))?;
        for part in components {
            ledger.check_deadline()?;
            match nt_resolve_list_component(&current, part)? {
                ListComponent::Directory {
                    handle: child,
                    actual_name,
                } => {
                    if actual_name != *part {
                        return Ok(BaseDir::Empty);
                    }
                    current = child;
                }
                ListComponent::AbsentOrRegularFile => return Ok(BaseDir::Empty),
            }
        }
        Ok(BaseDir::Ready(DirHandle(current)))
    }
}

impl DirHandle {
    pub(crate) fn reopen_branch(&self, rel_components: &[&str]) -> Result<Option<DirHandle>> {
        let mut current = self
            .0
            .try_clone()
            .map_err(|error| os_error(error, StorageOperation::List))?;
        for part in rel_components {
            match nt_try_open_child_dir(
                &current,
                part,
                DIR_TRAVERSE_ACCESS,
                StorageOperation::List,
                None,
            ) {
                Ok(Some(next)) => current = next,
                Ok(None) => return Ok(None),
                Err(error) => return Err(error),
            }
        }
        Ok(Some(DirHandle(current)))
    }

    pub(crate) fn open_scanner(&self) -> Result<DirScanner> {
        let stream_handle =
            nt_open_self_dir(&self.0, DIR_TRAVERSE_ACCESS, StorageOperation::List, None)?;
        Ok(DirScanner {
            handle: stream_handle,
            buffer: vec![0_u64; 2048],
            cursor_offset: None,
            first_call: true,
            exhausted: false,
        })
    }

    pub(crate) fn stat_candidate_file(&self, name: &str) -> Result<CandidateStat> {
        let wide: Vec<u16> = name.encode_utf16().collect();
        let us = make_unicode_string(&wide, StorageOperation::List)?;
        let oa = make_object_attributes(self.0.as_raw_handle() as HANDLE, &us);
        let mut iosb: IO_STATUS_BLOCK = unsafe { zeroed() };
        let mut raw: HANDLE = null_mut();
        let status = unsafe {
            NtCreateFile(
                &mut raw,
                FILE_READ_ATTRIBUTES | SYNCHRONIZE,
                &oa,
                &mut iosb,
                null(),
                0,
                DIR_SHARE_ALL,
                FILE_OPEN,
                FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
                null(),
                0,
            )
        };
        if status < 0 {
            if status == STATUS_OBJECT_NAME_NOT_FOUND || status == STATUS_DELETE_PENDING {
                return Ok(CandidateStat::Missing);
            }
            if status == STATUS_FILE_IS_A_DIRECTORY
                || status == STATUS_STOPPED_ON_SYMLINK
                || status == STATUS_IO_REPARSE_TAG_NOT_HANDLED
            {
                return Ok(CandidateStat::NotRegular);
            }
            return Err(ntstatus_to_error(status, StorageOperation::List, None));
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(raw as _) };
        let tag = query_tag_info(&handle, StorageOperation::List)?;
        if (tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0
            || (tag.FileAttributes & FILE_ATTRIBUTE_DIRECTORY) != 0
            || (tag.FileAttributes & FILE_ATTRIBUTE_DEVICE) != 0
        {
            return Ok(CandidateStat::NotRegular);
        }
        let meta = query_object_metadata(&handle, name, StorageOperation::List)?;
        Ok(CandidateStat::File {
            size: meta.size,
            last_modified: meta.last_modified,
        })
    }
}

impl DirScanner {
    pub(crate) fn next_entry(&mut self) -> Result<Option<ScannedEntry>> {
        loop {
            if let Some(offset) = self.cursor_offset {
                let byte_len = std::mem::size_of_val(self.buffer.as_slice());
                let (record, next_offset) =
                    decode_dir_record_at(&self.buffer, byte_len, offset, StorageOperation::List)?;
                let is_dot = record.name_u16 == [b'.' as u16]
                    || record.name_u16 == [b'.' as u16, b'.' as u16];
                let parsed = if is_dot {
                    None
                } else {
                    Some(classify_dir_record(&record))
                };
                self.cursor_offset = next_offset;
                if let Some(entry) = parsed {
                    return Ok(Some(entry));
                }
                continue;
            }
            if self.exhausted {
                return Ok(None);
            }
            let class = if self.first_call {
                self.first_call = false;
                FileIdBothDirectoryRestartInfo
            } else {
                FileIdBothDirectoryInfo
            };
            let byte_len = std::mem::size_of_val(self.buffer.as_slice()) as u32;
            let ok = unsafe {
                GetFileInformationByHandleEx(
                    self.handle.as_raw_handle() as HANDLE,
                    class,
                    self.buffer.as_mut_ptr().cast::<c_void>(),
                    byte_len,
                )
            };
            if ok == 0 {
                let code = unsafe { GetLastError() };
                if code == ERROR_NO_MORE_FILES {
                    self.exhausted = true;
                    return Ok(None);
                }
                return Err(os_error(
                    std::io::Error::from_raw_os_error(code as i32),
                    StorageOperation::List,
                ));
            }
            self.cursor_offset = Some(0);
        }
    }
}

struct RawDirRecord<'a> {
    file_attributes: u32,
    file_id: i64,
    short_name_len: i8,
    short_name: [u16; 12],
    name_u16: &'a [u16],
}

fn corrupt_dir_buffer_error(operation: StorageOperation) -> StorageError {
    StorageError::Io {
        operation: Some(operation),
        source: std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "malformed directory information record returned by filesystem",
        ),
    }
}

fn decode_dir_record_at(
    buffer: &[u64],
    valid_bytes: usize,
    offset: usize,
    operation: StorageOperation,
) -> Result<(RawDirRecord<'_>, Option<usize>)> {
    let total_cap = std::mem::size_of_val(buffer);
    let bound = valid_bytes.min(total_cap);
    let align = align_of::<FILE_ID_BOTH_DIR_INFO>();
    let name_field_offset = offset_of!(FILE_ID_BOTH_DIR_INFO, FileName);
    if bound < name_field_offset || !offset.is_multiple_of(align) {
        return Err(corrupt_dir_buffer_error(operation));
    }
    let Some(header_end) = offset.checked_add(name_field_offset) else {
        return Err(corrupt_dir_buffer_error(operation));
    };
    if header_end > bound {
        return Err(corrupt_dir_buffer_error(operation));
    }

    // SAFETY: `buffer` is `u64`-aligned (`>= align_of::<FILE_ID_BOTH_DIR_INFO>()`), `offset`
    // is aligned, and `offset + offset_of!(FILE_ID_BOTH_DIR_INFO, FileName) <= bound`.
    // Every field is read via raw pointer projection without creating `&FILE_ID_BOTH_DIR_INFO`
    // so provenance covers the full variable-length `FileName` payload within `buffer`.
    let base_ptr = buffer.as_ptr().cast::<u8>();
    let rec_ptr = unsafe { base_ptr.add(offset).cast::<FILE_ID_BOTH_DIR_INFO>() };
    let next_entry_offset = unsafe { (*rec_ptr).NextEntryOffset } as usize;
    let file_attributes = unsafe { (*rec_ptr).FileAttributes };
    let file_id = unsafe { (*rec_ptr).FileId };
    let short_name_len = unsafe { (*rec_ptr).ShortNameLength };
    let short_name = unsafe { (*rec_ptr).ShortName };
    let name_bytes = unsafe { (*rec_ptr).FileNameLength } as usize;

    let (record_bound, next_offset) = if next_entry_offset == 0 {
        (bound, None)
    } else {
        let Some(next_off) = offset.checked_add(next_entry_offset) else {
            return Err(corrupt_dir_buffer_error(operation));
        };
        if next_off <= offset || next_off > bound || !next_off.is_multiple_of(align) {
            return Err(corrupt_dir_buffer_error(operation));
        }
        (next_off, Some(next_off))
    };

    if !name_bytes.is_multiple_of(size_of::<u16>()) {
        return Err(corrupt_dir_buffer_error(operation));
    }
    let Some(name_end) = header_end.checked_add(name_bytes) else {
        return Err(corrupt_dir_buffer_error(operation));
    };
    if name_end > record_bound {
        return Err(corrupt_dir_buffer_error(operation));
    }

    let name_u16_len = name_bytes / size_of::<u16>();
    let name_ptr = unsafe { (&raw const (*rec_ptr).FileName).cast::<u16>() };
    let name_u16 = if name_u16_len == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(name_ptr, name_u16_len) }
    };

    Ok((
        RawDirRecord {
            file_attributes,
            file_id,
            short_name_len,
            short_name,
            name_u16,
        },
        next_offset,
    ))
}

fn for_each_dir_record(
    buffer: &[u64],
    valid_bytes: usize,
    operation: StorageOperation,
    mut visit: impl FnMut(RawDirRecord<'_>) -> ControlFlow<()>,
) -> Result<()> {
    let mut cursor = Some(0_usize);
    while let Some(offset) = cursor {
        let (record, next_offset) = decode_dir_record_at(buffer, valid_bytes, offset, operation)?;
        if visit(record).is_break() {
            break;
        }
        cursor = next_offset;
    }
    Ok(())
}

fn classify_dir_record(record: &RawDirRecord<'_>) -> ScannedEntry {
    let Ok(name) = String::from_utf16(record.name_u16) else {
        return ScannedEntry::Unsupported(
            "ill-formed UTF-16 directory entry encountered during listing",
        );
    };
    let short_reserved = if record.short_name_len > 0 {
        let short_u16 = (record.short_name_len as usize / 2).min(record.short_name.len());
        String::from_utf16(&record.short_name[..short_u16])
            .map(|short| namespace::is_reserved_segment(&short))
            .unwrap_or(false)
    } else {
        false
    };
    if namespace::is_reserved_segment(&name) || short_reserved {
        return ScannedEntry::Excluded;
    }
    if (record.file_attributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
        return ScannedEntry::Unsupported("reparse point encountered during listing");
    }
    if (record.file_attributes & FILE_ATTRIBUTE_DEVICE) != 0 {
        return ScannedEntry::Unsupported("device entry encountered during listing");
    }
    if !namespace::is_representable_segment(&name) {
        return ScannedEntry::Unsupported(
            "unrepresentable directory entry encountered during listing",
        );
    }
    if (record.file_attributes & FILE_ATTRIBUTE_DIRECTORY) != 0 {
        ScannedEntry::Directory(name)
    } else {
        ScannedEntry::File(name)
    }
}

impl StagedPut {
    pub(crate) fn take_file(&mut self) -> File {
        self.file
            .take()
            .expect("staged temporary file extracted once")
    }

    pub(crate) fn restore_file(&mut self, file: File) {
        self.file = Some(file);
    }

    pub(crate) fn sync_temp(&self, key: &str, operation: StorageOperation) -> Result<()> {
        let file = self
            .file
            .as_ref()
            .expect("staged temporary file present for sync");
        let ok = unsafe { FlushFileBuffers(file.as_raw_handle() as HANDLE) };
        if ok == 0 {
            let code = unsafe { GetLastError() };
            return Err(os_error_key(
                std::io::Error::from_raw_os_error(code as i32),
                operation,
                Some(key),
            ));
        }
        Ok(())
    }

    pub(crate) fn publish(
        &mut self,
        key: &str,
        create_only: bool,
        operation: StorageOperation,
    ) -> std::result::Result<(), (CommitKnowledge, StorageError)> {
        // Refuse overwriting existing directories or reparse points at the destination leaf.
        if let Err(err) =
            verify_destination_leaf_not_reparse_or_dir(&self.parent, &self.leaf_name, operation)
        {
            return Err((CommitKnowledge::NotCommitted, err));
        }

        let file = self
            .file
            .as_ref()
            .expect("staged temporary file open during Windows rename");
        let raw_file = file.as_raw_handle() as HANDLE;
        let wide_leaf: Vec<u16> = self.leaf_name.encode_utf16().collect();
        // Drop the duplicate cleanup handle so `raw_file` is the sole open handle during rename.
        // `self.file` remains `Some(...)` if rename fails deterministically before commit.
        self.cleanup_handle = None;

        let rename_result = if create_only {
            set_rename_info(
                raw_file,
                self.parent.as_raw_handle() as HANDLE,
                &wide_leaf,
                false,
                false,
            )
        } else {
            match set_rename_info(
                raw_file,
                self.parent.as_raw_handle() as HANDLE,
                &wide_leaf,
                true,
                true,
            ) {
                Ok(()) => Ok(()),
                Err(ERROR_INVALID_PARAMETER | ERROR_NOT_SUPPORTED | ERROR_INVALID_FUNCTION) => {
                    set_rename_info(
                        raw_file,
                        self.parent.as_raw_handle() as HANDLE,
                        &wide_leaf,
                        true,
                        false,
                    )
                }
                Err(code) => Err(code),
            }
        };

        match rename_result {
            Ok(()) => {
                self.armed_cleanup = false;
                let _ = self.file.take();
                Ok(())
            }
            Err(code)
                if create_only && (code == ERROR_ALREADY_EXISTS || code == ERROR_FILE_EXISTS) =>
            {
                Err((
                    CommitKnowledge::NotCommitted,
                    StorageError::Conflict {
                        provider: ProviderKind::Local,
                        operation,
                        target: Some(key.to_string()),
                        kind: ConflictKind::AlreadyExists,
                        detail: "destination object already exists".into(),
                    },
                ))
            }
            Err(code) => {
                let deterministically_uncommitted = matches!(
                    code,
                    ERROR_ACCESS_DENIED
                        | ERROR_ALREADY_EXISTS
                        | ERROR_FILE_EXISTS
                        | ERROR_DIRECTORY
                        | ERROR_INVALID_PARAMETER
                        | ERROR_NOT_SUPPORTED
                );
                let err = os_error_key(
                    std::io::Error::from_raw_os_error(code as i32),
                    operation,
                    Some(key),
                );
                if deterministically_uncommitted {
                    Err((CommitKnowledge::NotCommitted, err))
                } else {
                    self.armed_cleanup = false;
                    let _ = self.file.take();
                    Err((CommitKnowledge::Unknown, err))
                }
            }
        }
    }

    // Per DDR-0012, Windows durability syncs the staged file (`FlushFileBuffers`) prior to
    // handle-relative atomic rename and deliberately omits `FlushFileBuffers` on the directory
    // handle (which Win32 rejects on directory-mutation handles without `FILE_WRITE_DATA` or
    // escalates to a volume-wide flush).
    pub(crate) fn sync_parent(&self, _key: &str, _operation: StorageOperation) -> Result<()> {
        Ok(())
    }

    pub(crate) fn cleanup_now(&mut self) {
        if !self.armed_cleanup {
            let _ = self.file.take();
            self.cleanup_handle = None;
            return;
        }
        self.armed_cleanup = false;
        let primary = self.file.take().map(OwnedHandle::from);
        let backup = self.cleanup_handle.take();
        if let Some(handle) = primary {
            if mark_handle_deleted(&handle, &self.temp_name, StorageOperation::Put).is_ok() {
                return;
            }
        }
        if let Some(handle) = backup {
            if mark_handle_deleted(&handle, &self.temp_name, StorageOperation::Put).is_ok() {
                return;
            }
        }
        if let Ok(Some(handle)) = nt_try_open_temp_for_delete(&self.parent, &self.temp_name) {
            let _ = mark_handle_deleted(&handle, &self.temp_name, StorageOperation::Put);
        }
    }
}

impl Drop for StagedPut {
    fn drop(&mut self) {
        self.cleanup_now();
    }
}

fn set_rename_info(
    file_handle: HANDLE,
    parent_handle: HANDLE,
    wide_leaf: &[u16],
    replace_if_exists: bool,
    use_ex: bool,
) -> std::result::Result<(), u32> {
    let name_bytes = std::mem::size_of_val(wide_leaf);
    let total_bytes = size_of::<FILE_RENAME_INFO>() + name_bytes;
    let word_count = total_bytes.div_ceil(size_of::<u64>());
    let mut backing = vec![0_u64; word_count];
    let info_ptr = backing.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    unsafe {
        (*info_ptr).Anonymous = if use_ex {
            FILE_RENAME_INFO_0 {
                Flags: if replace_if_exists {
                    FILE_RENAME_FLAG_REPLACE_IF_EXISTS | FILE_RENAME_FLAG_POSIX_SEMANTICS
                } else {
                    FILE_RENAME_FLAG_POSIX_SEMANTICS
                },
            }
        } else {
            FILE_RENAME_INFO_0 {
                ReplaceIfExists: replace_if_exists,
            }
        };
        (*info_ptr).RootDirectory = parent_handle;
        (*info_ptr).FileNameLength = name_bytes as u32;
        let dst = std::ptr::addr_of_mut!((*info_ptr).FileName).cast::<u16>();
        std::ptr::copy_nonoverlapping(wide_leaf.as_ptr(), dst, wide_leaf.len());
    }
    let class = if use_ex {
        FileRenameInformationEx
    } else {
        FileRenameInformation
    };
    let mut iosb: IO_STATUS_BLOCK = unsafe { zeroed() };
    let status = unsafe {
        NtSetInformationFile(
            file_handle,
            &mut iosb,
            info_ptr.cast::<c_void>(),
            total_bytes as u32,
            class,
        )
    };
    if status < 0 {
        Err(unsafe { RtlNtStatusToDosError(status) })
    } else {
        Ok(())
    }
}

fn verify_destination_leaf_not_reparse_or_dir(
    parent: &OwnedHandle,
    leaf: &str,
    operation: StorageOperation,
) -> Result<()> {
    let wide: Vec<u16> = leaf.encode_utf16().collect();
    let us = make_unicode_string(&wide, operation)?;
    let oa = make_object_attributes(parent.as_raw_handle() as HANDLE, &us);
    let mut iosb: IO_STATUS_BLOCK = unsafe { zeroed() };
    let mut raw: HANDLE = null_mut();
    let status = unsafe {
        NtCreateFile(
            &mut raw,
            FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            &oa,
            &mut iosb,
            null(),
            0,
            DIR_SHARE_ALL,
            FILE_OPEN,
            FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
            null(),
            0,
        )
    };
    if status < 0 {
        if status == STATUS_OBJECT_NAME_NOT_FOUND || status == STATUS_DELETE_PENDING {
            return Ok(());
        }
        if status == STATUS_STOPPED_ON_SYMLINK || status == STATUS_IO_REPARSE_TAG_NOT_HANDLED {
            return Err(symlink_refused_error(operation));
        }
        return Err(ntstatus_to_error(status, operation, None));
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(raw as _) };
    let tag = query_tag_info(&handle, operation)?;
    if (tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
        return Err(symlink_refused_error(operation));
    }
    if (tag.FileAttributes & FILE_ATTRIBUTE_DIRECTORY) != 0
        || (tag.FileAttributes & FILE_ATTRIBUTE_DEVICE) != 0
    {
        return Err(non_regular_object_error(operation));
    }
    let _ = verify_not_reserved_long_name(parent, &handle, leaf, operation)?;
    Ok(())
}

fn verify_disk_directory_handle(handle: &OwnedHandle, operation: StorageOperation) -> Result<()> {
    let ftype = unsafe { GetFileType(handle.as_raw_handle() as HANDLE) };
    if ftype != FILE_TYPE_DISK {
        return Err(StorageError::InvalidArgument {
            operation: Some(operation),
            argument: "root".into(),
            reason: "configured local root must reside on a disk filesystem".into(),
        });
    }
    let tag = query_tag_info(handle, operation)?;
    if (tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
        return Err(symlink_refused_error(operation));
    }
    if (tag.FileAttributes & FILE_ATTRIBUTE_DIRECTORY) == 0 {
        return Err(StorageError::InvalidArgument {
            operation: Some(operation),
            argument: "root".into(),
            reason: "configured local root must be a directory".into(),
        });
    }
    Ok(())
}

fn query_root_id(handle: &OwnedHandle) -> Result<u128> {
    let mut id_info: FILE_ID_INFO = unsafe { zeroed() };
    let ok = unsafe {
        GetFileInformationByHandleEx(
            handle.as_raw_handle() as HANDLE,
            FileIdInfo,
            (&raw mut id_info).cast::<c_void>(),
            size_of::<FILE_ID_INFO>() as u32,
        )
    };
    if ok != 0 {
        let fid = u128::from_le_bytes(id_info.FileId.Identifier);
        let vol = (id_info.VolumeSerialNumber as u128) << 64
            | (id_info.VolumeSerialNumber as u128).rotate_left(17);
        return Ok(fid ^ vol);
    }
    // Fallback if a Windows filesystem does not implement FileIdInfo: random per-handle identity.
    let mut fallback = [0_u8; 16];
    getrandom::fill(&mut fallback).map_err(|_| StorageError::Io {
        operation: Some(StorageOperation::Probe),
        source: std::io::Error::other("system random source unavailable"),
    })?;
    Ok(u128::from_le_bytes(fallback))
}

fn query_tag_info(
    handle: &OwnedHandle,
    operation: StorageOperation,
) -> Result<FILE_ATTRIBUTE_TAG_INFO> {
    let mut info: FILE_ATTRIBUTE_TAG_INFO = unsafe { zeroed() };
    let ok = unsafe {
        GetFileInformationByHandleEx(
            handle.as_raw_handle() as HANDLE,
            FileAttributeTagInfo,
            (&raw mut info).cast::<c_void>(),
            size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    };
    if ok == 0 {
        let code = unsafe { GetLastError() };
        return Err(os_error(
            std::io::Error::from_raw_os_error(code as i32),
            operation,
        ));
    }
    Ok(info)
}

#[inline]
fn is_win_sep(ch: u16) -> bool {
    ch == b'\\' as u16 || ch == b'/' as u16
}

fn windows_root_prefix_len(wide: &[u16]) -> usize {
    // Literal verbatim `\\?\` prefix (`RtlDosPathNameToNtPathName_U` bypasses DOS path parsing
    // only for literal `\\?\`, not forward-slash variants or `\\.\`).
    if wide.len() >= 4
        && wide[0] == b'\\' as u16
        && wide[1] == b'\\' as u16
        && wide[2] == b'?' as u16
        && wide[3] == b'\\' as u16
    {
        // Verbatim `\\?\X:\`
        if wide.len() >= 7 && wide[5] == b':' as u16 && wide[6] == b'\\' as u16 {
            return 7;
        }
        // Verbatim `\\?\Volume{...}\`
        if wide.len() >= 11 && wide[4..11].eq_ignore_ascii_case_u16(b"volume{") {
            if let Some(rel_close) = wide[11..].iter().position(|&c| c == b'}' as u16) {
                let close_idx = 11 + rel_close;
                if close_idx + 1 < wide.len() && wide[close_idx + 1] == b'\\' as u16 {
                    return close_idx + 2;
                }
            }
        }
        // Verbatim `\\?\UNC\server\share\`
        if wide.len() >= 8 && wide[4..7].eq_ignore_ascii_case_u16(b"unc") && wide[7] == b'\\' as u16
        {
            return unc_share_prefix_len(wide, 8);
        }
        return 4;
    }
    // Win32 device namespace `\\.\X:\` or `\\.\`
    if wide.len() >= 4
        && is_win_sep(wide[0])
        && is_win_sep(wide[1])
        && wide[2] == b'.' as u16
        && is_win_sep(wide[3])
    {
        if wide.len() >= 7 && wide[5] == b':' as u16 && is_win_sep(wide[6]) {
            return 7;
        }
        return 4;
    }
    // Standard UNC `\\server\share\`
    if wide.len() >= 2 && is_win_sep(wide[0]) && is_win_sep(wide[1]) {
        return unc_share_prefix_len(wide, 2);
    }
    // Drive root `X:\` or drive-relative `X:`
    if wide.len() >= 3 && wide[1] == b':' as u16 && is_win_sep(wide[2]) {
        return 3;
    }
    if wide.len() >= 2 && wide[1] == b':' as u16 {
        return 2;
    }
    if !wide.is_empty() && is_win_sep(wide[0]) {
        return 1;
    }
    0
}

fn unc_share_prefix_len(wide: &[u16], start: usize) -> usize {
    let mut idx = start;
    while idx < wide.len() && !is_win_sep(wide[idx]) {
        idx += 1;
    }
    while idx < wide.len() && is_win_sep(wide[idx]) {
        idx += 1;
    }
    while idx < wide.len() && !is_win_sep(wide[idx]) {
        idx += 1;
    }
    if idx < wide.len() && is_win_sep(wide[idx]) {
        idx + 1
    } else {
        idx
    }
}

trait AsciiCaseU16 {
    fn eq_ignore_ascii_case_u16(&self, ascii_lower: &[u8]) -> bool;
}

impl AsciiCaseU16 for [u16] {
    fn eq_ignore_ascii_case_u16(&self, ascii_lower: &[u8]) -> bool {
        if self.len() != ascii_lower.len() {
            return false;
        }
        self.iter()
            .zip(ascii_lower.iter())
            .all(|(&w, &b)| u8::try_from(w).is_ok_and(|ch| ch.to_ascii_lowercase() == b))
    }
}

fn sanitize_windows_root_wide(wide: &[u16]) -> Result<Vec<u16>> {
    if wide.is_empty() {
        return Err(StorageError::InvalidArgument {
            operation: Some(StorageOperation::Probe),
            argument: "root".into(),
            reason: "configured local root path must not be empty".into(),
        });
    }
    if wide.contains(&0) {
        return Err(StorageError::InvalidArgument {
            operation: Some(StorageOperation::Probe),
            argument: "root".into(),
            reason: "configured local root path must not contain interior NUL".into(),
        });
    }

    let min_len = windows_root_prefix_len(wide);
    let mut end = wide.len();
    while end > min_len && is_win_sep(wide[end - 1]) {
        end -= 1;
    }
    let trimmed = &wide[..end];

    let dot = b'.' as u16;
    let space = b' ' as u16;
    // Win32 DOS-to-NT normalization (`RtlDosPathNameToNtPathName_U`) strips trailing ASCII spaces
    // (`0x20`) and periods (`0x2E`) as well as `.` / `..` segments; after stripping trailing
    // separators, rejecting any path other than bare `"."` that ends in `.` or `' '` closes all
    // trailing-dot/space/dot-segment reparse-point root bypasses (`root\.`, `root\..`, `root\...`,
    // `root\. `, `root\ `, `root\ \`, `root `, `C:.`, `C:..`).
    if trimmed != [dot] && trimmed.last().is_some_and(|&ch| ch == dot || ch == space) {
        return Err(StorageError::InvalidArgument {
            operation: Some(StorageOperation::Probe),
            argument: "root".into(),
            reason:
                "configured local root path must not end with '.'/'..' segments or trailing dots/spaces"
                    .into(),
        });
    }
    Ok(trimmed.to_vec())
}

fn query_file_id_low64(handle: &OwnedHandle) -> Option<i64> {
    let mut id_info: FILE_ID_INFO = unsafe { zeroed() };
    let ok = unsafe {
        GetFileInformationByHandleEx(
            handle.as_raw_handle() as HANDLE,
            FileIdInfo,
            (&raw mut id_info).cast::<c_void>(),
            size_of::<FILE_ID_INFO>() as u32,
        )
    };
    if ok == 0 || id_info.FileId.Identifier[8..16] != [0_u8; 8] {
        return None;
    }
    let mut lo = [0_u8; 8];
    lo.copy_from_slice(&id_info.FileId.Identifier[..8]);
    Some(i64::from_le_bytes(lo))
}

fn query_normalized_final_leaf_name(
    handle: &OwnedHandle,
    operation: StorageOperation,
) -> Result<Option<String>> {
    let mut buf = vec![0_u16; 1024];
    let mut len = unsafe {
        GetFinalPathNameByHandleW(
            handle.as_raw_handle() as HANDLE,
            buf.as_mut_ptr(),
            buf.len() as u32,
            FILE_NAME_NORMALIZED | VOLUME_NAME_NONE,
        )
    };
    if len as usize >= buf.len() {
        buf.resize(len as usize + 1, 0);
        len = unsafe {
            GetFinalPathNameByHandleW(
                handle.as_raw_handle() as HANDLE,
                buf.as_mut_ptr(),
                buf.len() as u32,
                FILE_NAME_NORMALIZED | VOLUME_NAME_NONE,
            )
        };
    }
    if len == 0 || len as usize >= buf.len() {
        let code = unsafe { GetLastError() };
        if matches!(
            code,
            ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND | ERROR_ACCESS_DENIED
        ) {
            return Ok(None);
        }
        return Err(os_error(
            std::io::Error::from_raw_os_error(code as i32),
            operation,
        ));
    }
    let slice = &buf[..len as usize];
    let trimmed = slice.strip_suffix(&[b'\\' as u16]).unwrap_or(slice);
    let leaf_u16 = trimmed
        .rsplit(|ch| *ch == b'\\' as u16)
        .next()
        .unwrap_or(trimmed);
    if leaf_u16.is_empty() {
        return Ok(None);
    }
    String::from_utf16(leaf_u16)
        .map(Some)
        .map_err(|_| StorageError::Io {
            operation: Some(operation),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "ill-formed UTF-16 component name on handle",
            ),
        })
}

/// Resolves the canonical on-disk long directory-entry name of `child` inside `parent`
/// without requiring traverse permissions above the configured root.
fn resolve_on_disk_leaf_name(
    parent: &OwnedHandle,
    child: &OwnedHandle,
    lookup_segment: &str,
    operation: StorageOperation,
) -> Result<Option<String>> {
    let expected_id = query_file_id_low64(child);
    let scan_handle = nt_open_self_dir(parent, DIR_TRAVERSE_ACCESS, operation, None)?;
    let wide: Vec<u16> = lookup_segment.encode_utf16().collect();
    let us = make_unicode_string(&wide, operation)?;
    let mut iosb: IO_STATUS_BLOCK = unsafe { zeroed() };
    let mut backing = [0_u64; 256];
    let byte_len = std::mem::size_of_val(&backing) as u32;
    let status = unsafe {
        NtQueryDirectoryFile(
            scan_handle.as_raw_handle() as HANDLE,
            null_mut(),
            None,
            null(),
            &mut iosb,
            backing.as_mut_ptr().cast::<c_void>(),
            byte_len,
            FileIdBothDirectoryInformation,
            true,
            &us,
            true,
        )
    };
    if status >= 0 {
        let written = iosb.Information.min(byte_len as usize);
        let mut matched_u16: Option<Vec<u16>> = None;
        for_each_dir_record(&backing, written, operation, |rec| {
            if expected_id.is_none_or(|cid| cid == rec.file_id) {
                matched_u16 = Some(rec.name_u16.to_vec());
                return ControlFlow::Break(());
            }
            ControlFlow::Continue(())
        })?;
        if let Some(raw_u16) = matched_u16 {
            let name = String::from_utf16(&raw_u16).map_err(|_| StorageError::Io {
                operation: Some(operation),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "ill-formed UTF-16 component name in directory entry",
                ),
            })?;
            return Ok(Some(name));
        }
    } else if status != STATUS_NO_SUCH_FILE
        && status != STATUS_OBJECT_NAME_NOT_FOUND
        && status != STATUS_DELETE_PENDING
    {
        return Err(ntstatus_to_error(status, operation, None));
    }

    query_normalized_final_leaf_name(child, operation)
}

fn verify_not_reserved_long_name(
    parent: &OwnedHandle,
    handle: &OwnedHandle,
    lookup_segment: &str,
    operation: StorageOperation,
) -> Result<Option<String>> {
    let Some(leaf) = resolve_on_disk_leaf_name(parent, handle, lookup_segment, operation)? else {
        return Ok(None);
    };
    if namespace::is_reserved_segment(&leaf) {
        return Err(StorageError::InvalidArgument {
            operation: Some(operation),
            argument: "key".into(),
            reason: "short-name or alias resolved into reserved temporary namespace".into(),
        });
    }
    Ok(Some(leaf))
}

fn query_object_metadata(
    handle: &OwnedHandle,
    key: &str,
    operation: StorageOperation,
) -> Result<ObjectMetadata> {
    let mut basic: FILE_BASIC_INFO = unsafe { zeroed() };
    let mut standard: FILE_STANDARD_INFO = unsafe { zeroed() };
    let ok_basic = unsafe {
        GetFileInformationByHandleEx(
            handle.as_raw_handle() as HANDLE,
            FileBasicInfo,
            (&raw mut basic).cast::<c_void>(),
            size_of::<FILE_BASIC_INFO>() as u32,
        )
    };
    if ok_basic == 0 {
        let code = unsafe { GetLastError() };
        return Err(os_error_key(
            std::io::Error::from_raw_os_error(code as i32),
            operation,
            Some(key),
        ));
    }
    let ok_std = unsafe {
        GetFileInformationByHandleEx(
            handle.as_raw_handle() as HANDLE,
            FileStandardInfo,
            (&raw mut standard).cast::<c_void>(),
            size_of::<FILE_STANDARD_INFO>() as u32,
        )
    };
    if ok_std == 0 {
        let code = unsafe { GetLastError() };
        return Err(os_error_key(
            std::io::Error::from_raw_os_error(code as i32),
            operation,
            Some(key),
        ));
    }
    if standard.Directory {
        return Err(non_regular_object_error(operation));
    }
    let size = u64::try_from(standard.EndOfFile).map_err(|_| StorageError::Io {
        operation: Some(operation),
        source: std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "negative regular file size reported by filesystem",
        ),
    })?;
    Ok(ObjectMetadata {
        path: key.to_string(),
        size,
        etag: None,
        content_type: None,
        last_modified: namespace::format_windows_filetime(basic.LastWriteTime),
        metadata: BTreeMap::new(),
    })
}

fn make_unicode_string(wide: &[u16], operation: StorageOperation) -> Result<UNICODE_STRING> {
    if wide.is_empty() {
        return Ok(UNICODE_STRING {
            Length: 0,
            MaximumLength: 0,
            Buffer: null_mut(),
        });
    }
    let byte_len = wide
        .len()
        .checked_mul(size_of::<u16>())
        .and_then(|n| u16::try_from(n).ok())
        .ok_or_else(|| StorageError::InvalidArgument {
            operation: Some(operation),
            argument: "key".into(),
            reason: "component exceeds UTF-16 length limit".into(),
        })?;
    Ok(UNICODE_STRING {
        Length: byte_len,
        MaximumLength: byte_len,
        Buffer: wide.as_ptr() as *mut u16,
    })
}

fn make_object_attributes(root: HANDLE, name: &UNICODE_STRING) -> OBJECT_ATTRIBUTES {
    // Attributes intentionally omits OBJ_INHERIT so every handle is non-inheritable.
    OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: root,
        ObjectName: name,
        Attributes: OBJ_CASE_INSENSITIVE,
        SecurityDescriptor: null(),
        SecurityQualityOfService: null(),
    }
}

fn ntstatus_to_error(
    status: NTSTATUS,
    operation: StorageOperation,
    key: Option<&str>,
) -> StorageError {
    if status == STATUS_STOPPED_ON_SYMLINK || status == STATUS_IO_REPARSE_TAG_NOT_HANDLED {
        return symlink_refused_error(operation);
    }
    let dos = unsafe { RtlNtStatusToDosError(status) };
    let io_err = if dos != 0 {
        std::io::Error::from_raw_os_error(dos as i32)
    } else {
        std::io::Error::other("contained NT filesystem call failed")
    };
    os_error_key(io_err, operation, key)
}

fn nt_open_self_dir(
    dir: &OwnedHandle,
    access: u32,
    operation: StorageOperation,
    key: Option<&str>,
) -> Result<OwnedHandle> {
    let empty: [u16; 0] = [];
    let us = make_unicode_string(&empty, operation)?;
    let oa = make_object_attributes(dir.as_raw_handle() as HANDLE, &us);
    let mut iosb: IO_STATUS_BLOCK = unsafe { zeroed() };
    let mut raw: HANDLE = null_mut();
    let status = unsafe {
        NtCreateFile(
            &mut raw,
            access,
            &oa,
            &mut iosb,
            null(),
            0,
            DIR_SHARE_ALL,
            FILE_OPEN,
            FILE_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
            null(),
            0,
        )
    };
    if status < 0 {
        return Err(ntstatus_to_error(status, operation, key));
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(raw as _) };
    let tag = query_tag_info(&handle, operation)?;
    if (tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
        return Err(symlink_refused_error(operation));
    }
    Ok(handle)
}

fn nt_try_open_child_dir(
    parent: &OwnedHandle,
    part: &str,
    access: u32,
    operation: StorageOperation,
    key: Option<&str>,
) -> Result<Option<OwnedHandle>> {
    let wide: Vec<u16> = part.encode_utf16().collect();
    let us = make_unicode_string(&wide, operation)?;
    let oa = make_object_attributes(parent.as_raw_handle() as HANDLE, &us);
    let mut iosb: IO_STATUS_BLOCK = unsafe { zeroed() };
    let mut raw: HANDLE = null_mut();
    let status = unsafe {
        NtCreateFile(
            &mut raw,
            access,
            &oa,
            &mut iosb,
            null(),
            0,
            DIR_SHARE_ALL,
            FILE_OPEN,
            FILE_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
            null(),
            0,
        )
    };
    if status < 0 {
        if status == STATUS_OBJECT_NAME_NOT_FOUND
            || status == STATUS_OBJECT_PATH_NOT_FOUND
            || status == STATUS_DELETE_PENDING
        {
            return Ok(None);
        }
        return Err(ntstatus_to_error(status, operation, key));
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(raw as _) };
    let tag = query_tag_info(&handle, operation)?;
    if (tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
        return Err(symlink_refused_error(operation));
    }
    if verify_not_reserved_long_name(parent, &handle, part, operation)?.is_none() {
        return Ok(None);
    }
    Ok(Some(handle))
}

fn nt_open_child_dir(
    parent: &OwnedHandle,
    part: &str,
    access: u32,
    operation: StorageOperation,
    key: Option<&str>,
) -> Result<OwnedHandle> {
    match nt_try_open_child_dir(parent, part, access, operation, key)? {
        Some(handle) => Ok(handle),
        None => Err(StorageError::NotFound {
            provider: ProviderKind::Local,
            operation,
            path: key.unwrap_or("[relative object]").to_string(),
        }),
    }
}

fn nt_open_or_create_child_dir(
    parent: &OwnedHandle,
    part: &str,
    operation: StorageOperation,
    key: &str,
) -> Result<OwnedHandle> {
    let wide: Vec<u16> = part.encode_utf16().collect();
    let us = make_unicode_string(&wide, operation)?;
    let oa = make_object_attributes(parent.as_raw_handle() as HANDLE, &us);
    let mut iosb: IO_STATUS_BLOCK = unsafe { zeroed() };
    let mut raw: HANDLE = null_mut();
    let status = unsafe {
        NtCreateFile(
            &mut raw,
            DIR_MUTATE_ACCESS,
            &oa,
            &mut iosb,
            null(),
            FILE_ATTRIBUTE_NORMAL,
            DIR_SHARE_ALL,
            FILE_OPEN_IF,
            FILE_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
            null(),
            0,
        )
    };
    if status < 0 {
        return Err(ntstatus_to_error(status, operation, Some(key)));
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(raw as _) };
    let tag = query_tag_info(&handle, operation)?;
    if (tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
        return Err(symlink_refused_error(operation));
    }
    if verify_not_reserved_long_name(parent, &handle, part, operation)?.is_none() {
        return Err(StorageError::NotFound {
            provider: ProviderKind::Local,
            operation,
            path: key.to_string(),
        });
    }
    Ok(handle)
}

enum ListComponent {
    Directory {
        handle: OwnedHandle,
        actual_name: String,
    },
    AbsentOrRegularFile,
}

fn nt_resolve_list_component(parent: &OwnedHandle, part: &str) -> Result<ListComponent> {
    let wide: Vec<u16> = part.encode_utf16().collect();
    let us = make_unicode_string(&wide, StorageOperation::List)?;
    let oa = make_object_attributes(parent.as_raw_handle() as HANDLE, &us);
    let mut iosb: IO_STATUS_BLOCK = unsafe { zeroed() };
    let mut raw: HANDLE = null_mut();
    let status = unsafe {
        NtCreateFile(
            &mut raw,
            DIR_TRAVERSE_ACCESS,
            &oa,
            &mut iosb,
            null(),
            0,
            DIR_SHARE_ALL,
            FILE_OPEN,
            FILE_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
            null(),
            0,
        )
    };
    if status >= 0 {
        let handle = unsafe { OwnedHandle::from_raw_handle(raw as _) };
        let tag = query_tag_info(&handle, StorageOperation::List)?;
        if (tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
            return Err(symlink_refused_error(StorageOperation::List));
        }
        let Some(actual_name) =
            verify_not_reserved_long_name(parent, &handle, part, StorageOperation::List)?
        else {
            return Ok(ListComponent::AbsentOrRegularFile);
        };
        return Ok(ListComponent::Directory {
            handle,
            actual_name,
        });
    }
    if status == STATUS_OBJECT_NAME_NOT_FOUND
        || status == STATUS_OBJECT_PATH_NOT_FOUND
        || status == STATUS_DELETE_PENDING
    {
        return Ok(ListComponent::AbsentOrRegularFile);
    }
    if status == STATUS_NOT_A_DIRECTORY {
        // Distinguish a regular file (empty list prefix) from a file symlink/reparse point.
        let mut raw_any: HANDLE = null_mut();
        let mut iosb_any: IO_STATUS_BLOCK = unsafe { zeroed() };
        let st_any = unsafe {
            NtCreateFile(
                &mut raw_any,
                FILE_READ_ATTRIBUTES | SYNCHRONIZE,
                &oa,
                &mut iosb_any,
                null(),
                0,
                DIR_SHARE_ALL,
                FILE_OPEN,
                FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
                null(),
                0,
            )
        };
        if st_any < 0 {
            if st_any == STATUS_OBJECT_NAME_NOT_FOUND || st_any == STATUS_DELETE_PENDING {
                return Ok(ListComponent::AbsentOrRegularFile);
            }
            return Err(ntstatus_to_error(st_any, StorageOperation::List, None));
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(raw_any as _) };
        let tag = query_tag_info(&handle, StorageOperation::List)?;
        if (tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
            return Err(symlink_refused_error(StorageOperation::List));
        }
        if (tag.FileAttributes & FILE_ATTRIBUTE_DEVICE) != 0 {
            return Err(non_regular_object_error(StorageOperation::List));
        }
        let _ = verify_not_reserved_long_name(parent, &handle, part, StorageOperation::List)?;
        return Ok(ListComponent::AbsentOrRegularFile);
    }
    Err(ntstatus_to_error(status, StorageOperation::List, None))
}

fn nt_open_leaf_file(
    parent: &OwnedHandle,
    leaf: &str,
    access: u32,
    share: u32,
    operation: StorageOperation,
    key: &str,
) -> Result<OwnedHandle> {
    let wide: Vec<u16> = leaf.encode_utf16().collect();
    let us = make_unicode_string(&wide, operation)?;
    let oa = make_object_attributes(parent.as_raw_handle() as HANDLE, &us);
    let mut iosb: IO_STATUS_BLOCK = unsafe { zeroed() };
    let mut raw: HANDLE = null_mut();
    let status = unsafe {
        NtCreateFile(
            &mut raw,
            access,
            &oa,
            &mut iosb,
            null(),
            0,
            share,
            FILE_OPEN,
            FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
            null(),
            0,
        )
    };
    if status < 0 {
        if status == STATUS_FILE_IS_A_DIRECTORY {
            // Check whether the directory entry is actually a directory reparse point/junction.
            let mut dir_raw: HANDLE = null_mut();
            let mut dir_iosb: IO_STATUS_BLOCK = unsafe { zeroed() };
            let dir_st = unsafe {
                NtCreateFile(
                    &mut dir_raw,
                    FILE_READ_ATTRIBUTES | SYNCHRONIZE,
                    &oa,
                    &mut dir_iosb,
                    null(),
                    0,
                    DIR_SHARE_ALL,
                    FILE_OPEN,
                    FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
                    null(),
                    0,
                )
            };
            if dir_st >= 0 {
                let dir_handle = unsafe { OwnedHandle::from_raw_handle(dir_raw as _) };
                if let Ok(tag) = query_tag_info(&dir_handle, operation) {
                    if (tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
                        return Err(symlink_refused_error(operation));
                    }
                }
            }
            return Err(non_regular_object_error(operation));
        }
        return Err(ntstatus_to_error(status, operation, Some(key)));
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(raw as _) };
    let ftype = unsafe { GetFileType(handle.as_raw_handle() as HANDLE) };
    if ftype != FILE_TYPE_DISK {
        return Err(non_regular_object_error(operation));
    }
    let tag = query_tag_info(&handle, operation)?;
    if (tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
        return Err(symlink_refused_error(operation));
    }
    if (tag.FileAttributes & FILE_ATTRIBUTE_DIRECTORY) != 0
        || (tag.FileAttributes & FILE_ATTRIBUTE_DEVICE) != 0
    {
        return Err(non_regular_object_error(operation));
    }
    if verify_not_reserved_long_name(parent, &handle, leaf, operation)?.is_none() {
        return Err(StorageError::NotFound {
            provider: ProviderKind::Local,
            operation,
            path: key.to_string(),
        });
    }
    Ok(handle)
}

fn nt_try_open_leaf_for_delete(
    parent: &OwnedHandle,
    leaf: &str,
    key: &str,
) -> Result<Option<OwnedHandle>> {
    match nt_open_leaf_file(
        parent,
        leaf,
        DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        DIR_SHARE_ALL,
        StorageOperation::Delete,
        key,
    ) {
        Ok(handle) => Ok(Some(handle)),
        Err(StorageError::NotFound { .. }) => Ok(None),
        Err(other) => Err(other),
    }
}

fn nt_try_open_temp_for_delete(
    parent: &OwnedHandle,
    temp_name: &str,
) -> Result<Option<OwnedHandle>> {
    let wide: Vec<u16> = temp_name.encode_utf16().collect();
    let us = make_unicode_string(&wide, StorageOperation::Put)?;
    let oa = make_object_attributes(parent.as_raw_handle() as HANDLE, &us);
    let mut iosb: IO_STATUS_BLOCK = unsafe { zeroed() };
    let mut raw: HANDLE = null_mut();
    let status = unsafe {
        NtCreateFile(
            &mut raw,
            DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            &oa,
            &mut iosb,
            null(),
            0,
            DIR_SHARE_ALL,
            FILE_OPEN,
            FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
            null(),
            0,
        )
    };
    if status < 0 {
        if status == STATUS_OBJECT_NAME_NOT_FOUND
            || status == STATUS_OBJECT_PATH_NOT_FOUND
            || status == STATUS_DELETE_PENDING
        {
            return Ok(None);
        }
        return Err(ntstatus_to_error(status, StorageOperation::Put, None));
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(raw as _) };
    let ftype = unsafe { GetFileType(handle.as_raw_handle() as HANDLE) };
    if ftype != FILE_TYPE_DISK {
        return Ok(None);
    }
    let tag = query_tag_info(&handle, StorageOperation::Put)?;
    if (tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0
        || (tag.FileAttributes & FILE_ATTRIBUTE_DIRECTORY) != 0
        || (tag.FileAttributes & FILE_ATTRIBUTE_DEVICE) != 0
    {
        return Ok(None);
    }
    Ok(Some(handle))
}

fn nt_create_exclusive_temp(
    parent: &OwnedHandle,
    temp_name: &str,
    operation: StorageOperation,
    key: &str,
) -> Result<Option<OwnedHandle>> {
    let wide: Vec<u16> = temp_name.encode_utf16().collect();
    let us = make_unicode_string(&wide, operation)?;
    let oa = make_object_attributes(parent.as_raw_handle() as HANDLE, &us);
    let mut iosb: IO_STATUS_BLOCK = unsafe { zeroed() };
    let mut raw: HANDLE = null_mut();
    let status = unsafe {
        NtCreateFile(
            &mut raw,
            FILE_GENERIC_WRITE | FILE_READ_ATTRIBUTES | DELETE | SYNCHRONIZE,
            &oa,
            &mut iosb,
            null(),
            FILE_ATTRIBUTE_NORMAL,
            FILE_SHARE_DELETE,
            FILE_CREATE,
            FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
            null(),
            0,
        )
    };
    if status < 0 {
        if status == STATUS_OBJECT_NAME_COLLISION {
            return Ok(None);
        }
        return Err(ntstatus_to_error(status, operation, Some(key)));
    }
    Ok(Some(unsafe { OwnedHandle::from_raw_handle(raw as _) }))
}

fn mark_handle_deleted(handle: &OwnedHandle, key: &str, operation: StorageOperation) -> Result<()> {
    let ex = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS,
    };
    let ok_ex = unsafe {
        SetFileInformationByHandle(
            handle.as_raw_handle() as HANDLE,
            FileDispositionInfoEx,
            (&raw const ex).cast::<c_void>(),
            size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
        )
    };
    if ok_ex != 0 {
        return Ok(());
    }
    let first_code = unsafe { GetLastError() };
    if matches!(
        first_code,
        ERROR_INVALID_PARAMETER | ERROR_NOT_SUPPORTED | ERROR_INVALID_FUNCTION
    ) {
        let legacy = FILE_DISPOSITION_INFO { DeleteFile: true };
        let ok_legacy = unsafe {
            SetFileInformationByHandle(
                handle.as_raw_handle() as HANDLE,
                FileDispositionInfo,
                (&raw const legacy).cast::<c_void>(),
                size_of::<FILE_DISPOSITION_INFO>() as u32,
            )
        };
        if ok_legacy != 0 {
            return Ok(());
        }
        let legacy_code = unsafe { GetLastError() };
        return Err(os_error_key(
            std::io::Error::from_raw_os_error(legacy_code as i32),
            operation,
            Some(key),
        ));
    }
    Err(os_error_key(
        std::io::Error::from_raw_os_error(first_code as i32),
        operation,
        Some(key),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        sync::atomic::{AtomicU64, Ordering},
    };
    use windows_sys::Win32::Foundation::{GetHandleInformation, HANDLE_FLAG_INHERIT};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "storageprims-win-fixture-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn assert_non_inheritable(raw: HANDLE) {
        let mut flags = 0_u32;
        let ok = unsafe { GetHandleInformation(raw, &mut flags) };
        assert_ne!(ok, 0);
        assert_eq!(flags & HANDLE_FLAG_INHERIT, 0);
    }

    #[test]
    fn windows_handles_are_non_inheritable_and_staged_create_only_enforces_single_winner() {
        let fixture = Fixture::new();
        std::fs::create_dir(fixture.0.join("branch")).unwrap();
        std::fs::write(fixture.0.join("branch/sample.bin"), "hello").unwrap();

        let root = Root::open(&fixture.0).unwrap();
        assert_non_inheritable(root.directory.as_raw_handle() as HANDLE);

        let (parent, _) = root
            .parent("branch/sample.bin", StorageOperation::Get)
            .unwrap();
        assert_non_inheritable(parent.as_raw_handle() as HANDLE);

        let (mut file, meta) = root
            .open_read("branch/sample.bin", StorageOperation::Get)
            .unwrap();
        assert_non_inheritable(file.as_raw_handle() as HANDLE);
        assert_eq!(meta.size, 5);
        let mut content = String::new();
        file.read_to_string(&mut content).unwrap();
        assert_eq!(content, "hello");

        let mut first = root
            .create_staged_put("branch/once.bin", StorageOperation::Put)
            .unwrap();
        assert_non_inheritable(first.parent.as_raw_handle() as HANDLE);
        assert_non_inheritable(first.file.as_ref().unwrap().as_raw_handle() as HANDLE);
        assert_non_inheritable(first.cleanup_handle.as_ref().unwrap().as_raw_handle() as HANDLE);
        first.file.as_mut().unwrap().write_all(b"winner").unwrap();
        first
            .sync_temp("branch/once.bin", StorageOperation::Put)
            .unwrap();
        first
            .publish("branch/once.bin", true, StorageOperation::Put)
            .unwrap();

        let mut second = root
            .create_staged_put("branch/once.bin", StorageOperation::Put)
            .unwrap();
        let second_temp = second.temp_name.clone();
        second.file.as_mut().unwrap().write_all(b"loser").unwrap();
        second
            .sync_temp("branch/once.bin", StorageOperation::Put)
            .unwrap();
        let (commit, err) = second
            .publish("branch/once.bin", true, StorageOperation::Put)
            .unwrap_err();
        assert_eq!(commit, CommitKnowledge::NotCommitted);
        assert!(matches!(
            err,
            StorageError::Conflict {
                kind: ConflictKind::AlreadyExists,
                ..
            }
        ));
        drop(second);
        assert!(!fixture.0.join("branch").join(second_temp).exists());

        // Aborted stream after `take_file()` must still delete the `.storageprims-tmp-*` entry!
        let mut aborted = root
            .create_staged_put("branch/aborted.bin", StorageOperation::Put)
            .unwrap();
        let aborted_temp = aborted.temp_name.clone();
        let mut taken = aborted.take_file();
        taken.write_all(b"partial-bytes").unwrap();
        drop(taken);
        aborted.cleanup_now();
        assert!(!fixture.0.join("branch").join(&aborted_temp).exists());
    }

    #[test]
    fn windows_root_path_sanitization_strips_trailing_separators_and_refuses_dot_suffixes() {
        let encode = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        let decode = |v: Vec<u16>| String::from_utf16(&v).unwrap();

        assert_eq!(
            decode(sanitize_windows_root_wide(&encode(r"C:\\")).unwrap()),
            r"C:\"
        );
        assert_eq!(
            decode(sanitize_windows_root_wide(&encode(r"\\?\C:\\")).unwrap()),
            r"\\?\C:\"
        );
        assert_eq!(
            decode(
                sanitize_windows_root_wide(&encode(
                    r"\\?\Volume{01234567-89ab-cdef-0123-456789abcdef}\\"
                ))
                .unwrap()
            ),
            r"\\?\Volume{01234567-89ab-cdef-0123-456789abcdef}\"
        );
        assert_eq!(
            decode(sanitize_windows_root_wide(&encode(r"C:\store\root\/")).unwrap()),
            r"C:\store\root"
        );
        assert_eq!(
            decode(sanitize_windows_root_wide(&encode(r".\")).unwrap()),
            "."
        );

        for invalid in [
            "",
            "..",
            r".\.",
            r"C:\store\root\.",
            r"C:\store\root\..",
            r"C:\store\root\...",
            r"C:\store\root\. ",
            r"C:\store\root\.. ",
            r"C:\store\root\ ",
            r"C:\store\root\ \",
            r"C:\store\root ",
            r"\\server\share\ ",
            "C:.",
            "C:..",
            "C: ",
        ] {
            assert!(
                sanitize_windows_root_wide(&encode(invalid)).is_err(),
                "expected {invalid:?} to be rejected"
            );
        }

        let fixture = Fixture::new();
        std::fs::create_dir(fixture.0.join("real")).unwrap();
        let real_slash =
            std::path::PathBuf::from(format!("{}\\", fixture.0.join("real").display()));
        assert!(Root::open(&real_slash).is_ok());

        let dot_root = std::path::PathBuf::from(format!(r"{}\.", fixture.0.join("real").display()));
        assert!(Root::open(&dot_root).is_err());

        let link_path = fixture.0.join("root-link");
        if std::os::windows::fs::symlink_dir(fixture.0.join("real"), &link_path).is_ok() {
            for candidate in [
                link_path.clone(),
                std::path::PathBuf::from(format!(r"{}\", link_path.display())),
                std::path::PathBuf::from(format!("{}/", link_path.display())),
                std::path::PathBuf::from(format!(r"{}\.", link_path.display())),
                std::path::PathBuf::from(format!(r"{}\...", link_path.display())),
                std::path::PathBuf::from(format!(r"{}\ ", link_path.display())),
                std::path::PathBuf::from(format!(r"{}\ \", link_path.display())),
            ] {
                assert!(
                    Root::open(&candidate).is_err(),
                    "expected symlink root {candidate:?} to be refused"
                );
            }
        }
    }

    #[test]
    fn incremental_dir_record_cursor_decodes_one_entry_at_a_time() {
        let align = align_of::<FILE_ID_BOTH_DIR_INFO>();
        let header_bytes = offset_of!(FILE_ID_BOTH_DIR_INFO, FileName);
        let names = ["alpha.txt", "beta.txt", "gamma.txt"];
        let mut raw_bytes = Vec::new();
        for (idx, name) in names.iter().enumerate() {
            let wide: Vec<u16> = name.encode_utf16().collect();
            let name_len = wide.len() * size_of::<u16>();
            let unpadded = header_bytes + name_len;
            let padded = (unpadded + (align - 1)) & !(align - 1);
            let start = raw_bytes.len();
            raw_bytes.resize(start + padded, 0_u8);
            let next_off = if idx + 1 == names.len() {
                0_u32
            } else {
                padded as u32
            };
            unsafe {
                let rec = raw_bytes
                    .as_mut_ptr()
                    .add(start)
                    .cast::<FILE_ID_BOTH_DIR_INFO>();
                (*rec).NextEntryOffset = next_off;
                (*rec).FileNameLength = name_len as u32;
                let dst = (&raw mut (*rec).FileName).cast::<u16>();
                std::ptr::copy_nonoverlapping(wide.as_ptr(), dst, wide.len());
            }
        }
        let u64_len = raw_bytes.len().div_ceil(size_of::<u64>());
        let mut aligned = vec![0_u64; u64_len];
        unsafe {
            std::ptr::copy_nonoverlapping(
                raw_bytes.as_ptr(),
                aligned.as_mut_ptr().cast::<u8>(),
                raw_bytes.len(),
            );
        }

        let mut seen = Vec::new();
        let mut cursor = Some(0_usize);
        while let Some(offset) = cursor {
            let (rec, next) =
                decode_dir_record_at(&aligned, raw_bytes.len(), offset, StorageOperation::List)
                    .unwrap();
            seen.push(String::from_utf16(rec.name_u16).unwrap());
            cursor = next;
        }
        assert_eq!(seen, vec!["alpha.txt", "beta.txt", "gamma.txt"]);
    }
}
