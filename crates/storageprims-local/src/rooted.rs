//! Unix component traversal, staging, and directory scanning anchored to opened directory descriptors.

use crate::{
    namespace, non_regular_object_error, os_error, os_error_key, symlink_refused_error, BaseDir,
    CandidateStat, ScannedEntry,
};
use rustix::{
    fd::OwnedFd,
    fs::{self, AtFlags, Dir, FileType, Mode, OFlags},
    io::Errno,
};
use std::{collections::BTreeMap, ffi::OsStr, fs::File, os::unix::ffi::OsStrExt, path::Path};
use storageprims_core::{
    CommitKnowledge, ConflictKind, ObjectMetadata, ProviderKind, Result, StorageError,
    StorageOperation,
};

pub(crate) struct Root {
    directory: OwnedFd,
    root_id: u128,
}

pub(crate) struct DirHandle(OwnedFd);

pub(crate) struct DirScanner {
    dir: Dir,
}

pub(crate) struct StagedPut {
    parent: OwnedFd,
    temp_name: String,
    leaf_name: String,
    file: Option<File>,
    armed_cleanup: bool,
}

impl Root {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let sanitized = sanitize_root_path(path)?;
        let directory = fs::open(
            sanitized,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| os_error(error.into(), StorageOperation::Probe))?;
        let metadata = fs::fstat(&directory)
            .map_err(|error| os_error(error.into(), StorageOperation::Probe))?;
        if FileType::from_raw_mode(metadata.st_mode) != FileType::Directory {
            return Err(StorageError::InvalidArgument {
                operation: Some(StorageOperation::Probe),
                argument: "root".into(),
                reason: "configured local root must be a directory".into(),
            });
        }
        let dev = metadata.st_dev as u64 as u128;
        let ino = metadata.st_ino as u64 as u128;
        let root_id = (dev << 64) | ino;
        Ok(Self { directory, root_id })
    }

    pub(crate) fn root_id(&self) -> u128 {
        self.root_id
    }

    pub(crate) fn probe_root(&self) -> Result<()> {
        let current = fs::openat(
            &self.directory,
            ".",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| os_error(error.into(), StorageOperation::Probe))?;
        let metadata =
            fs::fstat(&current).map_err(|error| os_error(error.into(), StorageOperation::Probe))?;
        if FileType::from_raw_mode(metadata.st_mode) != FileType::Directory {
            return Err(StorageError::InvalidArgument {
                operation: Some(StorageOperation::Probe),
                argument: "root".into(),
                reason: "configured local root must be a directory".into(),
            });
        }
        Ok(())
    }

    pub(crate) fn parent(
        &self,
        key: &str,
        operation: StorageOperation,
    ) -> Result<(OwnedFd, String)> {
        namespace::validate(key, operation)?;
        let mut parts = key.split('/').peekable();
        let mut directory = rustix::io::fcntl_dupfd_cloexec(&self.directory, 0)
            .map_err(|error| os_error_key(error.into(), operation, Some(key)))?;
        while let Some(part) = parts.next() {
            if parts.peek().is_none() {
                return Ok((directory, part.into()));
            }
            directory = fs::openat(
                &directory,
                part,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|error| map_ancestor_open_err(error, operation, key))?;
        }
        unreachable!("validated key has a leaf")
    }

    pub(crate) fn open_read(
        &self,
        key: &str,
        operation: StorageOperation,
    ) -> Result<(File, ObjectMetadata)> {
        let (parent, leaf) = self.parent(key, operation)?;
        let descriptor = fs::openat(
            &parent,
            &leaf,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| map_leaf_open_err(error, &parent, &leaf, operation, key))?;
        let metadata = fs::fstat(&descriptor)
            .map_err(|error| os_error_key(error.into(), operation, Some(key)))?;
        if FileType::from_raw_mode(metadata.st_mode) != FileType::RegularFile {
            return Err(non_regular_object_error(operation));
        }
        let _ = fs::fcntl_setfl(&descriptor, OFlags::RDONLY);
        let obj_meta = stat_to_object_metadata(key, &metadata)?;
        Ok((File::from(descriptor), obj_meta))
    }

    pub(crate) fn head_metadata(&self, key: &str) -> Result<ObjectMetadata> {
        let (parent, leaf) = self.parent(key, StorageOperation::Head)?;
        let metadata = fs::statat(&parent, &leaf, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|error| os_error_key(error.into(), StorageOperation::Head, Some(key)))?;
        match FileType::from_raw_mode(metadata.st_mode) {
            FileType::RegularFile => stat_to_object_metadata(key, &metadata),
            FileType::Symlink => Err(symlink_refused_error(StorageOperation::Head)),
            _ => Err(non_regular_object_error(StorageOperation::Head)),
        }
    }

    pub(crate) fn delete_object(&self, key: &str) -> Result<()> {
        namespace::validate(key, StorageOperation::Delete)?;
        let mut parts = key.split('/').peekable();
        let mut directory = rustix::io::fcntl_dupfd_cloexec(&self.directory, 0)
            .map_err(|error| os_error_key(error.into(), StorageOperation::Delete, Some(key)))?;
        let leaf = loop {
            let part = parts
                .next()
                .expect("validated key has at least one segment");
            if parts.peek().is_none() {
                break part;
            }
            match fs::openat(
                &directory,
                part,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            ) {
                Ok(child) => directory = child,
                Err(Errno::NOENT) => return Ok(()),
                Err(error) => {
                    return Err(map_ancestor_open_err(error, StorageOperation::Delete, key))
                }
            }
        };

        let metadata = match fs::statat(&directory, leaf, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(meta) => meta,
            Err(Errno::NOENT) => return Ok(()),
            Err(error) => {
                return Err(os_error_key(
                    error.into(),
                    StorageOperation::Delete,
                    Some(key),
                ))
            }
        };
        match FileType::from_raw_mode(metadata.st_mode) {
            FileType::RegularFile => {}
            FileType::Symlink => return Err(symlink_refused_error(StorageOperation::Delete)),
            _ => return Err(non_regular_object_error(StorageOperation::Delete)),
        }

        match fs::unlinkat(&directory, leaf, AtFlags::empty()) {
            Ok(()) => {
                fs::fsync(&directory).map_err(|error| {
                    os_error_key(error.into(), StorageOperation::Delete, Some(key))
                })?;
                Ok(())
            }
            Err(Errno::NOENT) => Ok(()),
            Err(error) => Err(os_error_key(
                error.into(),
                StorageOperation::Delete,
                Some(key),
            )),
        }
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
            match fs::openat(
                &parent_dir,
                &temp_name,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            ) {
                Ok(fd) => {
                    return Ok(StagedPut {
                        parent: parent_dir,
                        temp_name,
                        leaf_name,
                        file: Some(File::from(fd)),
                        armed_cleanup: true,
                    });
                }
                Err(Errno::EXIST) => continue,
                Err(error) => return Err(os_error_key(error.into(), operation, Some(key))),
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

    #[cfg(test)]
    pub(crate) fn create_staged_put_with_temp_name(
        &self,
        key: &str,
        temp_name: &str,
        operation: StorageOperation,
    ) -> Result<StagedPut> {
        namespace::validate(key, operation)?;
        let parent_dir = self.ensure_parent_dirs(key, operation)?;
        let leaf_name = key.rsplit('/').next().unwrap().to_string();
        let fd = fs::openat(
            &parent_dir,
            temp_name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(|error| os_error_key(error.into(), operation, Some(key)))?;
        Ok(StagedPut {
            parent: parent_dir,
            temp_name: temp_name.to_string(),
            leaf_name,
            file: Some(File::from(fd)),
            armed_cleanup: true,
        })
    }

    fn ensure_parent_dirs(&self, key: &str, operation: StorageOperation) -> Result<OwnedFd> {
        let mut parts = key.split('/').peekable();
        let mut directory = rustix::io::fcntl_dupfd_cloexec(&self.directory, 0)
            .map_err(|error| os_error_key(error.into(), operation, Some(key)))?;
        while let Some(part) = parts.next() {
            if parts.peek().is_none() {
                return Ok(directory);
            }
            let next = match fs::openat(
                &directory,
                part,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            ) {
                Ok(fd) => fd,
                Err(Errno::NOENT) => {
                    let dir_mode = Mode::RWXU | Mode::RGRP | Mode::XGRP | Mode::ROTH | Mode::XOTH;
                    let mut created = false;
                    match fs::mkdirat(&directory, part, dir_mode) {
                        Ok(()) => created = true,
                        Err(Errno::EXIST) => {}
                        Err(error) => {
                            return Err(os_error_key(error.into(), operation, Some(key)));
                        }
                    }
                    let opened = fs::openat(
                        &directory,
                        part,
                        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                        Mode::empty(),
                    )
                    .map_err(|error| map_ancestor_open_err(error, operation, key))?;
                    if created {
                        fs::fsync(&directory)
                            .map_err(|error| os_error_key(error.into(), operation, Some(key)))?;
                    }
                    opened
                }
                Err(error) => return Err(map_ancestor_open_err(error, operation, key)),
            };
            directory = next;
        }
        Ok(directory)
    }

    pub(crate) fn resolve_list_base(
        &self,
        components: &[&str],
        ledger: &mut crate::listing::Ledger,
    ) -> Result<BaseDir> {
        let mut directory = rustix::io::fcntl_dupfd_cloexec(&self.directory, 0)
            .map_err(|error| os_error(error.into(), StorageOperation::List))?;
        for part in components {
            ledger.check_deadline()?;
            match fs::openat(
                &directory,
                *part,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            ) {
                Ok(child) => {
                    if !matches_exact_leaf_case(&directory, &child, part, ledger)? {
                        return Ok(BaseDir::Empty);
                    }
                    directory = child;
                }
                Err(Errno::NOENT) => return Ok(BaseDir::Empty),
                Err(Errno::NOTDIR) => {
                    match fs::statat(&directory, *part, AtFlags::SYMLINK_NOFOLLOW) {
                        Ok(st) if FileType::from_raw_mode(st.st_mode) == FileType::RegularFile => {
                            return Ok(BaseDir::Empty);
                        }
                        Ok(st) if FileType::from_raw_mode(st.st_mode) == FileType::Symlink => {
                            return Err(symlink_refused_error(StorageOperation::List));
                        }
                        Ok(_) => return Err(non_regular_object_error(StorageOperation::List)),
                        Err(Errno::NOENT) => return Ok(BaseDir::Empty),
                        Err(error) => {
                            return Err(os_error(error.into(), StorageOperation::List));
                        }
                    }
                }
                Err(Errno::LOOP) => return Err(symlink_refused_error(StorageOperation::List)),
                Err(error) => return Err(os_error(error.into(), StorageOperation::List)),
            }
        }
        Ok(BaseDir::Ready(DirHandle(directory)))
    }
}

impl DirHandle {
    pub(crate) fn reopen_branch(&self, rel_components: &[&str]) -> Result<Option<DirHandle>> {
        let mut current = rustix::io::fcntl_dupfd_cloexec(&self.0, 0)
            .map_err(|error| os_error(error.into(), StorageOperation::List))?;
        for part in rel_components {
            match fs::openat(
                &current,
                *part,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            ) {
                Ok(next) => current = next,
                Err(Errno::NOENT) => return Ok(None),
                Err(Errno::LOOP) => return Err(symlink_refused_error(StorageOperation::List)),
                Err(Errno::NOTDIR) => {
                    return Err(StorageError::Other {
                        provider: Some(ProviderKind::Local),
                        operation: Some(StorageOperation::List),
                        detail:
                            "directory branch changed to non-directory or symlink during listing"
                                .into(),
                        source: None,
                    });
                }
                Err(error) => return Err(os_error(error.into(), StorageOperation::List)),
            }
        }
        Ok(Some(DirHandle(current)))
    }

    pub(crate) fn open_scanner(&self) -> Result<DirScanner> {
        let stream_fd = fs::openat(
            &self.0,
            ".",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| os_error(error.into(), StorageOperation::List))?;
        let dir =
            Dir::new(stream_fd).map_err(|error| os_error(error.into(), StorageOperation::List))?;
        Ok(DirScanner { dir })
    }

    pub(crate) fn stat_candidate_file(&self, name: &str) -> Result<CandidateStat> {
        match fs::statat(&self.0, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(metadata) => {
                if FileType::from_raw_mode(metadata.st_mode) != FileType::RegularFile {
                    return Ok(CandidateStat::NotRegular);
                }
                let size = u64::try_from(metadata.st_size).map_err(|_| StorageError::Io {
                    operation: Some(StorageOperation::List),
                    source: std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "negative regular file size reported by filesystem",
                    ),
                })?;
                let last_modified = stat_mtime_rfc3339(&metadata);
                Ok(CandidateStat::File {
                    size,
                    last_modified,
                })
            }
            Err(Errno::NOENT) => Ok(CandidateStat::Missing),
            Err(error) => Err(os_error(error.into(), StorageOperation::List)),
        }
    }
}

impl DirScanner {
    pub(crate) fn next_entry(&mut self) -> Result<Option<ScannedEntry>> {
        loop {
            let Some(item) = self.dir.read() else {
                return Ok(None);
            };
            let entry = item.map_err(|error| os_error(error.into(), StorageOperation::List))?;
            let raw_name = entry.file_name().to_bytes();
            if raw_name == b"." || raw_name == b".." {
                continue;
            }
            let Ok(name) = std::str::from_utf8(raw_name) else {
                return Ok(Some(ScannedEntry::Unsupported(
                    "non-UTF-8 directory entry encountered during listing",
                )));
            };
            if namespace::is_reserved_segment(name) {
                return Ok(Some(ScannedEntry::Excluded));
            }
            if !namespace::is_representable_segment(name) {
                return Ok(Some(ScannedEntry::Unsupported(
                    "unrepresentable directory entry encountered during listing",
                )));
            }
            let kind = match entry.file_type() {
                FileType::RegularFile => ScannedEntry::File(name.to_string()),
                FileType::Directory => ScannedEntry::Directory(name.to_string()),
                FileType::Unknown => {
                    let dir_fd = self
                        .dir
                        .fd()
                        .map_err(|error| os_error(error.into(), StorageOperation::List))?;
                    match fs::statat(dir_fd, entry.file_name(), AtFlags::SYMLINK_NOFOLLOW) {
                        Ok(st) => match FileType::from_raw_mode(st.st_mode) {
                            FileType::RegularFile => ScannedEntry::File(name.to_string()),
                            FileType::Directory => ScannedEntry::Directory(name.to_string()),
                            _ => ScannedEntry::Unsupported(
                                "non-regular or symlink entry encountered during listing",
                            ),
                        },
                        Err(Errno::NOENT) => ScannedEntry::Excluded,
                        Err(error) => {
                            return Err(os_error(error.into(), StorageOperation::List));
                        }
                    }
                }
                _ => ScannedEntry::Unsupported(
                    "non-regular or symlink entry encountered during listing",
                ),
            };
            return Ok(Some(kind));
        }
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
        fs::fsync(file).map_err(|error| os_error_key(error.into(), operation, Some(key)))
    }

    pub(crate) fn publish(
        &mut self,
        key: &str,
        create_only: bool,
        operation: StorageOperation,
    ) -> std::result::Result<(), (CommitKnowledge, StorageError)> {
        let _ = self.file.take();

        if create_only {
            return self.publish_create_only(key, operation);
        }

        // Refuse overwriting symlinks, directories, FIFOs, or special files at the leaf.
        match fs::statat(&self.parent, &self.leaf_name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(st) => match FileType::from_raw_mode(st.st_mode) {
                FileType::RegularFile => {}
                FileType::Symlink => {
                    return Err((
                        CommitKnowledge::NotCommitted,
                        symlink_refused_error(operation),
                    ));
                }
                _ => {
                    return Err((
                        CommitKnowledge::NotCommitted,
                        non_regular_object_error(operation),
                    ));
                }
            },
            Err(Errno::NOENT) => {}
            Err(error) => {
                return Err((
                    CommitKnowledge::NotCommitted,
                    os_error_key(error.into(), operation, Some(key)),
                ));
            }
        }

        match fs::renameat(&self.parent, &self.temp_name, &self.parent, &self.leaf_name) {
            Ok(()) => {
                self.armed_cleanup = false;
                Ok(())
            }
            Err(error) => Err(self.classify_rename_failure(error, key, operation)),
        }
    }

    #[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
    fn publish_create_only(
        &mut self,
        key: &str,
        operation: StorageOperation,
    ) -> std::result::Result<(), (CommitKnowledge, StorageError)> {
        match fs::renameat_with(
            &self.parent,
            &self.temp_name,
            &self.parent,
            &self.leaf_name,
            fs::RenameFlags::NOREPLACE,
        ) {
            Ok(()) => {
                self.armed_cleanup = false;
                Ok(())
            }
            Err(Errno::EXIST) => {
                if let Ok(st) = fs::statat(&self.parent, &self.leaf_name, AtFlags::SYMLINK_NOFOLLOW)
                {
                    match FileType::from_raw_mode(st.st_mode) {
                        FileType::RegularFile => {}
                        FileType::Symlink => {
                            return Err((
                                CommitKnowledge::NotCommitted,
                                symlink_refused_error(operation),
                            ));
                        }
                        _ => {
                            return Err((
                                CommitKnowledge::NotCommitted,
                                non_regular_object_error(operation),
                            ));
                        }
                    }
                }
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
            Err(error) => Err(self.classify_rename_failure(error, key, operation)),
        }
    }

    #[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android")))]
    fn publish_create_only(
        &mut self,
        _key: &str,
        operation: StorageOperation,
    ) -> std::result::Result<(), (CommitKnowledge, StorageError)> {
        Err((
            CommitKnowledge::NotCommitted,
            StorageError::ProviderUnavailable {
                provider: ProviderKind::Local,
                operation,
                detail: "atomic create-only rename is unavailable on this unix target".into(),
            },
        ))
    }

    fn classify_rename_failure(
        &mut self,
        error: Errno,
        key: &str,
        operation: StorageOperation,
    ) -> (CommitKnowledge, StorageError) {
        let deterministically_uncommitted = matches!(
            error,
            Errno::ACCESS
                | Errno::PERM
                | Errno::ROFS
                | Errno::NOSPC
                | Errno::DQUOT
                | Errno::NAMETOOLONG
                | Errno::NOTDIR
                | Errno::ISDIR
                | Errno::LOOP
                | Errno::NOENT
                | Errno::INVAL
                | Errno::NOSYS
                | Errno::OPNOTSUPP
        );
        if deterministically_uncommitted {
            (
                CommitKnowledge::NotCommitted,
                os_error_key(error.into(), operation, Some(key)),
            )
        } else {
            self.armed_cleanup = false;
            (
                CommitKnowledge::Unknown,
                os_error_key(error.into(), operation, Some(key)),
            )
        }
    }

    pub(crate) fn sync_parent(&self, key: &str, operation: StorageOperation) -> Result<()> {
        fs::fsync(&self.parent).map_err(|error| os_error_key(error.into(), operation, Some(key)))
    }

    pub(crate) fn cleanup_now(&mut self) {
        let _ = self.file.take();
        if self.armed_cleanup {
            self.armed_cleanup = false;
            let _ = fs::unlinkat(&self.parent, &self.temp_name, AtFlags::empty());
        }
    }
}

impl Drop for StagedPut {
    fn drop(&mut self) {
        self.cleanup_now();
    }
}

fn stat_to_object_metadata(key: &str, metadata: &fs::Stat) -> Result<ObjectMetadata> {
    let size = u64::try_from(metadata.st_size).map_err(|_| StorageError::Io {
        operation: Some(StorageOperation::Head),
        source: std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "negative regular file size reported by filesystem",
        ),
    })?;
    let last_modified = stat_mtime_rfc3339(metadata);
    Ok(ObjectMetadata {
        path: key.to_string(),
        size,
        etag: None,
        content_type: None,
        last_modified,
        metadata: BTreeMap::new(),
    })
}

#[allow(clippy::useless_conversion, clippy::unnecessary_cast)]
fn stat_mtime_rfc3339(metadata: &fs::Stat) -> Option<String> {
    let secs = i64::try_from(metadata.st_mtime).ok()?;
    namespace::format_rfc3339_utc(secs, metadata.st_mtime_nsec as u32)
}

fn map_ancestor_open_err(error: Errno, operation: StorageOperation, key: &str) -> StorageError {
    if error == Errno::LOOP {
        return symlink_refused_error(operation);
    }
    os_error_key(error.into(), operation, Some(key))
}

fn map_leaf_open_err(
    error: Errno,
    parent: &OwnedFd,
    leaf: &str,
    operation: StorageOperation,
    key: &str,
) -> StorageError {
    if error == Errno::LOOP {
        return symlink_refused_error(operation);
    }
    if error == Errno::ISDIR {
        return non_regular_object_error(operation);
    }
    // On FreeBSD/DragonFly O_NOFOLLOW can yield EMLINK; on macOS/Linux ELOOP is returned.
    // Check statat(SYMLINK_NOFOLLOW) if an unexpected error occurs on a symlink leaf.
    if let Ok(st) = fs::statat(parent, leaf, AtFlags::SYMLINK_NOFOLLOW) {
        if FileType::from_raw_mode(st.st_mode) == FileType::Symlink {
            return symlink_refused_error(operation);
        }
    }
    os_error_key(error.into(), operation, Some(key))
}

fn sanitize_root_path(path: &Path) -> Result<&OsStr> {
    let raw = path.as_os_str().as_bytes();
    if raw.is_empty() {
        return Err(StorageError::InvalidArgument {
            operation: Some(StorageOperation::Probe),
            argument: "root".into(),
            reason: "configured local root path must not be empty".into(),
        });
    }
    if raw.contains(&0) {
        return Err(StorageError::InvalidArgument {
            operation: Some(StorageOperation::Probe),
            argument: "root".into(),
            reason: "configured local root path must not contain interior NUL".into(),
        });
    }
    let mut trimmed = raw;
    while trimmed.len() > 1 && trimmed.ends_with(b"/") {
        trimmed = &trimmed[..trimmed.len() - 1];
    }
    if trimmed == b".." || trimmed.ends_with(b"/.") || trimmed.ends_with(b"/..") {
        return Err(StorageError::InvalidArgument {
            operation: Some(StorageOperation::Probe),
            argument: "root".into(),
            reason: "configured local root path must not end with '.' or '..' traversal segments"
                .into(),
        });
    }
    Ok(OsStr::from_bytes(trimmed))
}

#[cfg(target_vendor = "apple")]
fn matches_exact_leaf_case(
    _parent_fd: &OwnedFd,
    dir_fd: &OwnedFd,
    expected_component: &str,
    _ledger: &mut crate::listing::Ledger,
) -> Result<bool> {
    let raw_path =
        fs::getpath(dir_fd).map_err(|error| os_error(error.into(), StorageOperation::List))?;
    let bytes = raw_path.to_bytes();
    let trimmed = bytes.strip_suffix(b"/").unwrap_or(bytes);
    let actual_leaf = trimmed
        .rsplit(|byte| *byte == b'/')
        .next()
        .unwrap_or(trimmed);
    Ok(actual_leaf == expected_component.as_bytes())
}

#[cfg(not(target_vendor = "apple"))]
fn matches_exact_leaf_case(
    parent_fd: &OwnedFd,
    dir_fd: &OwnedFd,
    expected_component: &str,
    ledger: &mut crate::listing::Ledger,
) -> Result<bool> {
    let child_stat =
        fs::fstat(dir_fd).map_err(|error| os_error(error.into(), StorageOperation::List))?;

    // O(1) case-sensitive filesystem fast path: for ASCII segments that do not end with
    // characters subject to SMB/VFAT trailing-dot/space folding, flipping the case of an ASCII
    // letter probes whether `parent_fd` performs case folding without any `readdir` scan or `/proc`
    // dependency (and unlike `/proc/self/fd`, cannot be fooled by FUSE/VirtioFS negative-dentry
    // alias caching).
    if expected_component.is_ascii() && !expected_component.ends_with(['.', ' ']) {
        match expected_component
            .bytes()
            .position(|byte| byte.is_ascii_alphabetic())
        {
            None => return Ok(true),
            Some(alpha_idx) => {
                let mut flipped = expected_component.as_bytes().to_vec();
                flipped[alpha_idx] ^= 0x20;
                // SAFETY: flipping ASCII bit 0x20 on an ASCII alphabetic byte preserves UTF-8.
                let flipped_str =
                    std::str::from_utf8(&flipped).expect("ASCII case flip preserves valid UTF-8");
                match fs::statat(parent_fd, flipped_str, AtFlags::SYMLINK_NOFOLLOW) {
                    Err(Errno::NOENT) => return Ok(true),
                    Ok(alt_stat)
                        if (alt_stat.st_dev, alt_stat.st_ino)
                            != (child_stat.st_dev, child_stat.st_ino) =>
                    {
                        return Ok(true);
                    }
                    Ok(_) => {}
                    Err(error) => return Err(os_error(error.into(), StorageOperation::List)),
                }
            }
        }
    }

    scan_parent_for_exact_leaf(parent_fd, &child_stat, expected_component, ledger)
}

#[cfg(any(not(target_vendor = "apple"), test))]
fn scan_parent_for_exact_leaf(
    parent_fd: &OwnedFd,
    child_stat: &fs::Stat,
    expected_component: &str,
    ledger: &mut crate::listing::Ledger,
) -> Result<bool> {
    let stream_fd = fs::openat(
        parent_fd,
        ".",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| os_error(error.into(), StorageOperation::List))?;
    let mut dir =
        Dir::new(stream_fd).map_err(|error| os_error(error.into(), StorageOperation::List))?;
    while let Some(item) = dir.read() {
        let entry = item.map_err(|error| os_error(error.into(), StorageOperation::List))?;
        let raw_name = entry.file_name().to_bytes();
        if raw_name == b"." || raw_name == b".." {
            continue;
        }
        ledger.charge_entry()?;
        if raw_name == expected_component.as_bytes() {
            return match fs::statat(parent_fd, entry.file_name(), AtFlags::SYMLINK_NOFOLLOW) {
                Ok(st) => Ok(st.st_dev == child_stat.st_dev && st.st_ino == child_stat.st_ino),
                Err(Errno::NOENT) => Ok(false),
                Err(error) => Err(os_error(error.into(), StorageOperation::List)),
            };
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        os::unix::fs::{symlink, PermissionsExt},
        sync::atomic::{AtomicU64, Ordering},
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "storageprims-root-fixture-{}-{}",
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
    fn read(root: &Root, key: &str) -> String {
        let mut text = String::new();
        let (mut file, _) = root.open_read(key, StorageOperation::Get).unwrap();
        file.read_to_string(&mut text).unwrap();
        text
    }
    fn safe_chain(error: &StorageError, root: &Path) {
        let mut cause: &(dyn std::error::Error + 'static) = error;
        loop {
            assert!(!format!("{cause:?} {cause}").contains(root.to_str().unwrap()));
            match cause.source() {
                Some(next) => cause = next,
                None => break,
            }
        }
    }

    #[test]
    fn root_ancestor_and_leaf_symlinks_are_refused_even_when_contained() {
        let fixture = Fixture::new();
        std::fs::create_dir(fixture.0.join("root")).unwrap();
        std::fs::create_dir(fixture.0.join("root/sub")).unwrap();
        std::fs::write(fixture.0.join("root/sub/sample.bin"), "contained").unwrap();
        symlink("root", fixture.0.join("root-link")).unwrap();
        let link_base = fixture.0.join("root-link");
        for candidate in [
            link_base.clone(),
            std::path::PathBuf::from(format!("{}/", link_base.display())),
            std::path::PathBuf::from(format!("{}//", link_base.display())),
            std::path::PathBuf::from(format!("{}/.", link_base.display())),
            std::path::PathBuf::from(format!("{}/..", link_base.display())),
        ] {
            let err = Root::open(&candidate).err().unwrap_or_else(|| {
                panic!(
                    "expected symlink root candidate {:?} to be refused",
                    candidate
                )
            });
            safe_chain(&err, &fixture.0);
        }
        // Trailing slashes on a real directory root remain accepted.
        let real_with_slash =
            std::path::PathBuf::from(format!("{}/", fixture.0.join("root").display()));
        assert!(Root::open(&real_with_slash).is_ok());
        symlink("sub", fixture.0.join("root/branch-link")).unwrap();
        symlink("sub/sample.bin", fixture.0.join("root/leaf-link")).unwrap();
        let root = Root::open(&fixture.0.join("root")).unwrap();
        assert_eq!(read(&root, "sub/sample.bin"), "contained");
        for key in [
            "branch-link/sample.bin",
            "leaf-link",
            "sub",
            "missing.bin",
            "../outside.bin",
        ] {
            let error = root.open_read(key, StorageOperation::Get).unwrap_err();
            safe_chain(&error, &fixture.0);
        }
    }

    #[test]
    fn opened_root_survives_path_replacement_without_rebinding() {
        let fixture = Fixture::new();
        std::fs::create_dir(fixture.0.join("root")).unwrap();
        std::fs::write(fixture.0.join("root/sample.bin"), "original").unwrap();
        let root = Root::open(&fixture.0.join("root")).unwrap();
        std::fs::rename(fixture.0.join("root"), fixture.0.join("retained")).unwrap();
        std::fs::create_dir(fixture.0.join("root")).unwrap();
        std::fs::write(fixture.0.join("root/sample.bin"), "replacement").unwrap();
        assert_eq!(read(&root, "sample.bin"), "original");
    }

    #[test]
    fn opened_parent_survives_ancestor_substitution_without_path_rewalk() {
        let fixture = Fixture::new();
        std::fs::create_dir(fixture.0.join("branch")).unwrap();
        std::fs::create_dir(fixture.0.join("outside")).unwrap();
        std::fs::write(fixture.0.join("branch/sample.bin"), "original").unwrap();
        std::fs::write(fixture.0.join("outside/sample.bin"), "outside").unwrap();
        let root = Root::open(&fixture.0).unwrap();
        let (parent, leaf) = root
            .parent("branch/sample.bin", StorageOperation::Get)
            .unwrap();
        std::fs::rename(fixture.0.join("branch"), fixture.0.join("retained")).unwrap();
        symlink("outside", fixture.0.join("branch")).unwrap();
        let descriptor = fs::openat(
            &parent,
            leaf,
            OFlags::RDONLY | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .unwrap();
        let mut text = String::new();
        File::from(descriptor).read_to_string(&mut text).unwrap();
        assert_eq!(text, "original");
        assert!(root
            .open_read("branch/sample.bin", StorageOperation::Get)
            .is_err());
    }

    #[test]
    fn constructor_and_real_os_error_chains_do_not_disclose_absolute_root() {
        let fixture = Fixture::new();
        let provider = crate::LocalProvider::new(&fixture.0).unwrap();
        assert!(!format!("{provider:?}").contains(fixture.0.to_str().unwrap()));
        let error = crate::LocalProvider::new(fixture.0.join("absent-root")).unwrap_err();
        safe_chain(&error, &fixture.0);
        let root = Root::open(&fixture.0).unwrap();
        std::fs::write(fixture.0.join("regular.bin"), "data").unwrap();
        let error = root
            .open_read("regular.bin/child", StorageOperation::Get)
            .unwrap_err();
        assert!(matches!(error, StorageError::Io { .. }));
        safe_chain(&error, &fixture.0);
    }

    #[test]
    fn every_root_derived_parent_read_and_temp_descriptor_is_close_on_exec() {
        let fixture = Fixture::new();
        std::fs::create_dir(fixture.0.join("branch")).unwrap();
        std::fs::write(fixture.0.join("sample.bin"), "sample").unwrap();
        std::fs::write(fixture.0.join("branch/sample.bin"), "nested").unwrap();
        let root = Root::open(&fixture.0).unwrap();
        assert!(rustix::io::fcntl_getfd(&root.directory)
            .unwrap()
            .contains(rustix::io::FdFlags::CLOEXEC));
        for key in ["sample.bin", "branch/sample.bin"] {
            let (parent, _) = root.parent(key, StorageOperation::Get).unwrap();
            assert!(rustix::io::fcntl_getfd(&parent)
                .unwrap()
                .contains(rustix::io::FdFlags::CLOEXEC));
            let (file, _) = root.open_read(key, StorageOperation::Get).unwrap();
            assert!(rustix::io::fcntl_getfd(&file)
                .unwrap()
                .contains(rustix::io::FdFlags::CLOEXEC));
        }

        let staged = root
            .create_staged_put("branch/new.bin", StorageOperation::Put)
            .unwrap();
        assert!(rustix::io::fcntl_getfd(&staged.parent)
            .unwrap()
            .contains(rustix::io::FdFlags::CLOEXEC));
        let temp_file = staged.file.as_ref().unwrap();
        assert!(rustix::io::fcntl_getfd(temp_file)
            .unwrap()
            .contains(rustix::io::FdFlags::CLOEXEC));
        let temp_meta = temp_file.metadata().unwrap();
        assert_eq!(temp_meta.permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn staged_put_collision_on_temp_creation_refuses_without_deleting_preexisting_entry() {
        let fixture = Fixture::new();
        let colliding = fixture.0.join(".storageprims-tmp-preexisting");
        std::fs::write(&colliding, "external-or-stale").unwrap();
        let root = Root::open(&fixture.0).unwrap();
        let err = root
            .create_staged_put_with_temp_name(
                "target.bin",
                ".storageprims-tmp-preexisting",
                StorageOperation::Put,
            )
            .err()
            .expect("must refuse colliding temp name");
        safe_chain(&err, &fixture.0);
        assert_eq!(
            std::fs::read_to_string(&colliding).unwrap(),
            "external-or-stale"
        );
    }

    #[test]
    fn staged_put_create_only_enforces_single_winner_and_cleans_up_temp() {
        let fixture = Fixture::new();
        let root = Root::open(&fixture.0).unwrap();
        let mut first = root
            .create_staged_put("once.bin", StorageOperation::Put)
            .unwrap();
        first.file.as_mut().unwrap().write_all(b"winner").unwrap();
        first.sync_temp("once.bin", StorageOperation::Put).unwrap();
        first
            .publish("once.bin", true, StorageOperation::Put)
            .unwrap();
        first
            .sync_parent("once.bin", StorageOperation::Put)
            .unwrap();

        let mut second = root
            .create_staged_put("once.bin", StorageOperation::Put)
            .unwrap();
        let second_temp = second.temp_name.clone();
        second.file.as_mut().unwrap().write_all(b"loser").unwrap();
        second.sync_temp("once.bin", StorageOperation::Put).unwrap();
        let (commit, err) = second
            .publish("once.bin", true, StorageOperation::Put)
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
        assert!(!fixture.0.join(second_temp).exists());
        assert_eq!(read(&root, "once.bin"), "winner");
    }

    #[test]
    fn fifo_is_opened_nonblocking_then_refused_as_non_object() {
        let fixture = Fixture::new();
        let root = Root::open(&fixture.0).unwrap();
        assert!(std::process::Command::new("mkfifo")
            .arg(fixture.0.join("pipe"))
            .status()
            .unwrap()
            .success());
        let error = root.open_read("pipe", StorageOperation::Get).unwrap_err();
        assert!(matches!(error, StorageError::InvalidArgument { .. }));
        safe_chain(&error, &fixture.0);
    }

    #[test]
    fn real_permission_error_retains_access_denied_without_absolute_path() {
        let fixture = Fixture::new();
        let file = fixture.0.join("denied.bin");
        std::fs::write(&file, "synthetic").unwrap();
        let root = Root::open(&fixture.0).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o0)).unwrap();
        let result = root.open_read("denied.bin", StorageOperation::Get);
        // Restore fixture permissions before any assertion that can unwind.
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        if rustix::process::geteuid().is_root() {
            // Privileged execution cannot establish an actual EACCES denial.
            eprintln!("permission-denial evidence unavailable under root execution");
            return;
        }
        let error = result.unwrap_err();
        assert!(matches!(error, StorageError::AccessDenied { .. }));
        safe_chain(&error, &fixture.0);
    }

    #[test]
    fn exact_leaf_parent_scan_charges_work_ledger_and_enforces_entry_cap() {
        let fixture = Fixture::new();
        for idx in 0..5 {
            std::fs::create_dir(fixture.0.join(format!("sibling-{idx}"))).unwrap();
        }
        std::fs::create_dir(fixture.0.join("Target")).unwrap();
        let root = Root::open(&fixture.0).unwrap();
        let child = fs::openat(
            &root.directory,
            "Target",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .unwrap();
        let child_stat = fs::fstat(&child).unwrap();

        let mut small_ledger = crate::listing::Ledger::new(crate::listing::ListLimits {
            max_entries: 2,
            max_duration: std::time::Duration::from_secs(10),
        });
        let err =
            scan_parent_for_exact_leaf(&root.directory, &child_stat, "target", &mut small_ledger)
                .unwrap_err();
        assert!(matches!(
            err,
            StorageError::Inspection {
                kind: storageprims_core::InspectionKind::LimitExceeded,
                limit_dimension: Some(storageprims_core::InspectionLimitDimension::Entries),
                configured_limit: 2,
                consumed: 3,
                ..
            }
        ));

        #[cfg(not(target_vendor = "apple"))]
        {
            // On a case-sensitive Unix directory, ASCII prefix verification resolves in O(1)
            // without charging `readdir` entries when no case-folded alias exists.
            let mut fast_ledger = crate::listing::Ledger::new(crate::listing::ListLimits {
                max_entries: 2,
                max_duration: std::time::Duration::from_secs(10),
            });
            let is_case_folded = fs::statat(&root.directory, "target", AtFlags::SYMLINK_NOFOLLOW)
                .is_ok_and(|st| (st.st_dev, st.st_ino) == (child_stat.st_dev, child_stat.st_ino));
            if !is_case_folded {
                assert!(matches_exact_leaf_case(
                    &root.directory,
                    &child,
                    "Target",
                    &mut fast_ledger
                )
                .unwrap());
                assert_eq!(fast_ledger.examined(), 0);
            }
        }
    }
}
