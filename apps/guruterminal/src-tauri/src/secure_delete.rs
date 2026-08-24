use std::{
    fs,
    path::{Component, Path, PathBuf},
};

#[cfg(unix)]
use std::{ffi::OsStr, os::unix::ffi::OsStrExt};

#[cfg(unix)]
use rustix::{
    fd::OwnedFd,
    fs::{
        chmodat, fchmod, fstat, fsync, mkdirat, open, openat, renameat_with, statat, unlinkat,
        AtFlags, Dir, FileType, Mode, OFlags, RenameFlags,
    },
    io::fcntl_dupfd_cloexec,
};

use crate::{app::CommandError, domain::RootFilesystemIdentity};

/// A retained capability for destructive operations below the app-data root.
///
/// Callers pass validated relative paths only. Unix operations descend from the
/// retained root descriptor with `openat(O_NOFOLLOW)`; Windows retains the root
/// and every ancestor without delete sharing before using a pathname API. This
/// keeps deletion independent from mutable ancestors of the app-data pathname.
#[derive(Debug)]
pub struct SecureDeletionRoot {
    path: PathBuf,
    #[cfg(unix)]
    descriptor: OwnedFd,
    #[cfg(unix)]
    device: u64,
    #[cfg(windows)]
    _handle: fs::File,
}

#[derive(Debug)]
pub struct PrivateDirectoryGuard {
    file: fs::File,
    #[cfg(windows)]
    _ancestors: Vec<fs::File>,
}

impl PrivateDirectoryGuard {
    pub fn file(&self) -> &fs::File {
        &self.file
    }
}

impl SecureDeletionRoot {
    pub fn open(path: &Path) -> Result<Self, CommandError> {
        validate_absolute_path(path)?;
        #[cfg(unix)]
        {
            let descriptor = open_absolute_directory_unix(path)?;
            let metadata = fstat(&descriptor).map_err(map_rustix)?;
            if FileType::from_raw_mode(metadata.st_mode) != FileType::Directory {
                return Err(CommandError::internal(
                    "app data deletion root is not a directory",
                ));
            }
            Ok(Self {
                path: path.to_path_buf(),
                descriptor,
                device: metadata.st_dev as u64,
            })
        }
        #[cfg(windows)]
        {
            let handle = crate::windows_fs::open_directory_no_reparse(path).map_err(map_io)?;
            Ok(Self {
                path: path.to_path_buf(),
                _handle: handle,
            })
        }
        #[cfg(not(any(unix, windows)))]
        Err(CommandError::internal(
            "secure deletion is unsupported on this platform",
        ))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn absolute_path(&self, relative: &Path) -> Result<PathBuf, CommandError> {
        validate_relative_path(relative)?;
        Ok(self.path.join(relative))
    }

    pub fn ensure_private_subdirectory(
        &self,
        relative: &Path,
    ) -> Result<PrivateDirectoryGuard, CommandError> {
        self.open_private_subdirectory_inner(relative, true)
    }

    pub fn open_private_subdirectory(
        &self,
        relative: &Path,
    ) -> Result<PrivateDirectoryGuard, CommandError> {
        self.open_private_subdirectory_inner(relative, false)
    }

    fn open_private_subdirectory_inner(
        &self,
        relative: &Path,
        create: bool,
    ) -> Result<PrivateDirectoryGuard, CommandError> {
        validate_relative_path(relative)?;
        #[cfg(unix)]
        {
            use rustix::io::Errno;

            let mut directory = fcntl_dupfd_cloexec(&self.descriptor, 3).map_err(map_rustix)?;
            for component in relative.components() {
                let Component::Normal(name) = component else {
                    return Err(CommandError::internal(
                        "private directory contains a non-ordinary component",
                    ));
                };
                directory = match openat(&directory, name, directory_flags_unix(), Mode::empty()) {
                    Ok(next) => next,
                    Err(Errno::NOENT) if create => {
                        match mkdirat(&directory, name, Mode::from_raw_mode(0o700)) {
                            Ok(()) | Err(Errno::EXIST) => {}
                            Err(error) => return Err(map_rustix(error)),
                        }
                        openat(&directory, name, directory_flags_unix(), Mode::empty())
                            .map_err(map_rustix)?
                    }
                    Err(error) => return Err(map_rustix(error)),
                };
                let metadata = fstat(&directory).map_err(map_rustix)?;
                if FileType::from_raw_mode(metadata.st_mode) != FileType::Directory
                    || metadata.st_dev as u64 != self.device
                {
                    return Err(CommandError::internal(
                        "private directory crosses an untrusted filesystem boundary",
                    ));
                }
                fchmod(&directory, Mode::from_raw_mode(0o700)).map_err(map_rustix)?;
            }
            Ok(PrivateDirectoryGuard {
                file: fs::File::from(directory),
            })
        }
        #[cfg(windows)]
        {
            let mut ancestors = Vec::new();
            let mut path = self.path.clone();
            for component in relative.components() {
                path.push(component.as_os_str());
                match crate::windows_fs::open_directory_no_reparse(&path) {
                    Ok(handle) => ancestors.push(handle),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound && create => {
                        fs::create_dir(&path).map_err(map_io)?;
                        ancestors.push(
                            crate::windows_fs::open_directory_no_reparse(&path).map_err(map_io)?,
                        );
                    }
                    Err(error) => return Err(map_io(error)),
                }
            }
            let file = ancestors
                .last()
                .ok_or_else(|| CommandError::internal("private directory is empty"))?
                .try_clone()
                .map_err(map_io)?;
            Ok(PrivateDirectoryGuard {
                file,
                _ancestors: ancestors,
            })
        }
        #[cfg(not(any(unix, windows)))]
        Err(CommandError::internal(
            "private directory pinning is unsupported on this platform",
        ))
    }

    pub fn rename_sibling(&self, source: &Path, destination: &Path) -> Result<bool, CommandError> {
        self.rename_sibling_expected(source, destination, None)
    }

    pub fn rename_sibling_expected(
        &self,
        source: &Path,
        destination: &Path,
        expected: Option<&RootFilesystemIdentity>,
    ) -> Result<bool, CommandError> {
        validate_relative_path(source)?;
        validate_relative_path(destination)?;
        if source.parent() != destination.parent() {
            return Err(CommandError::internal(
                "deletion rename must stay in one pinned parent",
            ));
        }
        let source_name = source
            .file_name()
            .ok_or_else(|| CommandError::internal("deletion source has no name"))?;
        let destination_name = destination
            .file_name()
            .ok_or_else(|| CommandError::internal("deletion destination has no name"))?;

        #[cfg(unix)]
        {
            use rustix::io::Errno;

            let Some(parent) = self.open_parent_unix(source)? else {
                return Ok(false);
            };
            let _source_guard = match expected {
                Some(expected) => {
                    match open_expected_directory_unix(&parent, source_name, self.device, expected)?
                    {
                        Some(guard) => Some(guard),
                        None => return Ok(false),
                    }
                }
                None => None,
            };
            match renameat_with(
                &parent,
                source_name,
                &parent,
                destination_name,
                RenameFlags::NOREPLACE,
            ) {
                Ok(()) => {
                    if let Some(expected) = expected {
                        verify_identity_unix(&parent, destination_name, expected)?;
                    }
                    fsync(&parent).map_err(map_rustix)?;
                    Ok(true)
                }
                Err(Errno::NOENT) => Ok(false),
                Err(error) => Err(map_rustix(error)),
            }
        }
        #[cfg(windows)]
        {
            let source = self.absolute_path(source)?;
            let destination = self.absolute_path(destination)?;
            let _ancestors = match crate::windows_fs::open_parent_directories_no_reparse(&source) {
                Ok(ancestors) => ancestors,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
                Err(error) => return Err(map_io(error)),
            };
            if let Some(expected) = expected {
                verify_directory_identity_windows(&source, expected)?;
            }
            match crate::windows_fs::move_file_no_replace(&source, &destination) {
                Ok(()) => {
                    if let Some(expected) = expected {
                        verify_directory_identity_windows(&destination, expected)?;
                    }
                    Ok(true)
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(error) => Err(map_io(error)),
            }
        }
        #[cfg(not(any(unix, windows)))]
        Err(CommandError::internal(
            "secure deletion rename is unsupported on this platform",
        ))
    }

    pub fn remove_tree(&self, relative: &Path) -> Result<(), CommandError> {
        self.remove_tree_expected(relative, None)
    }

    pub fn remove_tree_expected(
        &self,
        relative: &Path,
        expected: Option<&RootFilesystemIdentity>,
    ) -> Result<(), CommandError> {
        validate_relative_path(relative)?;
        let name = relative
            .file_name()
            .ok_or_else(|| CommandError::internal("deletion path has no name"))?;
        #[cfg(unix)]
        {
            let Some(parent) = self.open_parent_unix(relative)? else {
                return Ok(());
            };
            remove_entry_unix(&parent, name, self.device, expected)?;
            fsync(&parent).map_err(map_rustix)?;
            Ok(())
        }
        #[cfg(windows)]
        {
            let path = self.absolute_path(relative)?;
            let _ancestors = match crate::windows_fs::open_parent_directories_no_reparse(&path) {
                Ok(ancestors) => ancestors,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(map_io(error)),
            };
            if let Some(expected) = expected {
                verify_directory_identity_windows(&path, expected)?;
            }
            remove_entry_windows(&path)
        }
        #[cfg(not(any(unix, windows)))]
        Err(CommandError::internal(
            "secure tree deletion is unsupported on this platform",
        ))
    }

    pub fn entry_exists(&self, relative: &Path) -> Result<bool, CommandError> {
        validate_relative_path(relative)?;
        let name = relative
            .file_name()
            .ok_or_else(|| CommandError::internal("deletion path has no name"))?;
        #[cfg(unix)]
        {
            use rustix::io::Errno;

            let Some(parent) = self.open_parent_unix(relative)? else {
                return Ok(false);
            };
            match statat(&parent, name, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(_) => Ok(true),
                Err(Errno::NOENT) => Ok(false),
                Err(error) => Err(map_rustix(error)),
            }
        }
        #[cfg(windows)]
        {
            let path = self.absolute_path(relative)?;
            let _ancestors = match crate::windows_fs::open_parent_directories_no_reparse(&path) {
                Ok(ancestors) => ancestors,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
                Err(error) => return Err(map_io(error)),
            };
            match fs::symlink_metadata(path) {
                Ok(_) => Ok(true),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(error) => Err(map_io(error)),
            }
        }
        #[cfg(not(any(unix, windows)))]
        Err(CommandError::internal(
            "secure deletion lookup is unsupported on this platform",
        ))
    }

    pub fn directory_identity(
        &self,
        relative: &Path,
    ) -> Result<Option<RootFilesystemIdentity>, CommandError> {
        validate_relative_path(relative)?;
        let name = relative
            .file_name()
            .ok_or_else(|| CommandError::internal("private directory has no name"))?;
        #[cfg(unix)]
        {
            let Some(parent) = self.open_parent_unix(relative)? else {
                return Ok(None);
            };
            let Some(directory) = open_directory_unix(&parent, name, self.device)? else {
                return Ok(None);
            };
            let metadata = fstat(&directory).map_err(map_rustix)?;
            Ok(Some(RootFilesystemIdentity {
                device: metadata.st_dev as u64,
                inode: metadata.st_ino as u64,
            }))
        }
        #[cfg(windows)]
        {
            let path = self.absolute_path(relative)?;
            let _ancestors = match crate::windows_fs::open_parent_directories_no_reparse(&path) {
                Ok(ancestors) => ancestors,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(map_io(error)),
            };
            let handle = match crate::windows_fs::open_directory_no_reparse(&path) {
                Ok(handle) => handle,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(map_io(error)),
            };
            crate::windows_fs::filesystem_identity(&handle)
                .map(Some)
                .map_err(map_io)
        }
        #[cfg(not(any(unix, windows)))]
        Err(CommandError::internal(
            "private directory identity is unsupported on this platform",
        ))
    }

    #[cfg(unix)]
    fn open_parent_unix(&self, relative: &Path) -> Result<Option<OwnedFd>, CommandError> {
        use rustix::io::Errno;

        let parent = relative.parent().unwrap_or_else(|| Path::new(""));
        let mut directory = fcntl_dupfd_cloexec(&self.descriptor, 3).map_err(map_rustix)?;
        for component in parent.components() {
            let Component::Normal(name) = component else {
                return Err(CommandError::internal(
                    "deletion path contains a non-ordinary component",
                ));
            };
            directory = match openat(&directory, name, directory_flags_unix(), Mode::empty()) {
                Ok(directory) => directory,
                Err(Errno::NOENT) => return Ok(None),
                Err(error) => return Err(map_rustix(error)),
            };
            let metadata = fstat(&directory).map_err(map_rustix)?;
            if FileType::from_raw_mode(metadata.st_mode) != FileType::Directory
                || metadata.st_dev as u64 != self.device
            {
                return Err(CommandError::internal(
                    "deletion path crosses an untrusted filesystem boundary",
                ));
            }
        }
        Ok(Some(directory))
    }
}

fn validate_absolute_path(path: &Path) -> Result<(), CommandError> {
    if !path.is_absolute()
        || path.components().any(|component| {
            !matches!(
                component,
                Component::Prefix(_) | Component::RootDir | Component::Normal(_)
            )
        })
    {
        return Err(CommandError::internal(
            "app data deletion root is not an absolute ordinary path",
        ));
    }
    Ok(())
}

fn validate_relative_path(path: &Path) -> Result<(), CommandError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(CommandError::internal(
            "deletion path contains a non-ordinary component",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn directory_flags_unix() -> OFlags {
    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC
}

#[cfg(unix)]
fn open_absolute_directory_unix(path: &Path) -> Result<OwnedFd, CommandError> {
    let mut directory = open("/", directory_flags_unix(), Mode::empty()).map_err(map_rustix)?;
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                directory = openat(&directory, name, directory_flags_unix(), Mode::empty())
                    .map_err(map_rustix)?;
                let metadata = fstat(&directory).map_err(map_rustix)?;
                if FileType::from_raw_mode(metadata.st_mode) != FileType::Directory {
                    return Err(CommandError::internal(
                        "app data deletion root contains a non-directory component",
                    ));
                }
            }
            _ => {
                return Err(CommandError::internal(
                    "app data deletion root contains a non-ordinary component",
                ));
            }
        }
    }
    Ok(directory)
}

#[cfg(unix)]
fn open_directory_unix(
    parent: &OwnedFd,
    name: &OsStr,
    root_device: u64,
) -> Result<Option<OwnedFd>, CommandError> {
    use rustix::io::Errno;

    let before = match statat(parent, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(metadata) => metadata,
        Err(Errno::NOENT) => return Ok(None),
        Err(error) => return Err(map_rustix(error)),
    };
    if FileType::from_raw_mode(before.st_mode) != FileType::Directory
        || before.st_dev as u64 != root_device
    {
        return Err(CommandError::internal(
            "private directory crosses an untrusted filesystem boundary",
        ));
    }
    let descriptor = match openat(parent, name, directory_flags_unix(), Mode::empty()) {
        Ok(descriptor) => descriptor,
        Err(Errno::NOENT) => return Ok(None),
        Err(error) => return Err(map_rustix(error)),
    };
    let opened = fstat(&descriptor).map_err(map_rustix)?;
    if FileType::from_raw_mode(opened.st_mode) != FileType::Directory
        || opened.st_dev != before.st_dev
        || opened.st_ino != before.st_ino
        || opened.st_dev as u64 != root_device
    {
        return Err(CommandError::internal(
            "private directory changed across its pinned boundary",
        ));
    }
    Ok(Some(descriptor))
}

#[cfg(unix)]
fn open_expected_directory_unix(
    parent: &OwnedFd,
    name: &OsStr,
    root_device: u64,
    expected: &RootFilesystemIdentity,
) -> Result<Option<OwnedFd>, CommandError> {
    let Some(descriptor) = open_directory_unix(parent, name, root_device)? else {
        return Ok(None);
    };
    let metadata = fstat(&descriptor).map_err(map_rustix)?;
    if metadata.st_dev as u64 != expected.device || metadata.st_ino as u64 != expected.inode {
        return Err(CommandError::conflict(
            "private storage identity changed before deletion",
        ));
    }
    Ok(Some(descriptor))
}

#[cfg(unix)]
fn verify_identity_unix(
    parent: &OwnedFd,
    name: &OsStr,
    expected: &RootFilesystemIdentity,
) -> Result<(), CommandError> {
    let metadata = statat(parent, name, AtFlags::SYMLINK_NOFOLLOW).map_err(map_rustix)?;
    if metadata.st_dev as u64 != expected.device || metadata.st_ino as u64 != expected.inode {
        return Err(CommandError::conflict(
            "private storage identity changed during deletion",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn remove_entry_unix(
    parent: &OwnedFd,
    name: &OsStr,
    root_device: u64,
    expected: Option<&RootFilesystemIdentity>,
) -> Result<(), CommandError> {
    use rustix::io::Errno;

    let metadata = match statat(parent, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(metadata) => metadata,
        Err(Errno::NOENT) => return Ok(()),
        Err(error) => return Err(map_rustix(error)),
    };
    if metadata.st_dev as u64 != root_device {
        return Err(CommandError::internal(
            "deletion target crosses an untrusted filesystem boundary",
        ));
    }
    if let Some(expected) = expected {
        if metadata.st_dev as u64 != expected.device || metadata.st_ino as u64 != expected.inode {
            return Err(CommandError::conflict(
                "private storage identity changed before cleanup",
            ));
        }
    }
    let file_type = FileType::from_raw_mode(metadata.st_mode);
    if file_type == FileType::Directory {
        // An app-owned cache may have crashed after changing its mode. Restore
        // only directory traversal bits, without following a replacement link.
        chmodat(
            parent,
            name,
            Mode::from_raw_mode(0o700),
            AtFlags::SYMLINK_NOFOLLOW,
        )
        .map_err(map_rustix)?;
        let Some(descriptor) = open_directory_unix(parent, name, root_device)? else {
            return Ok(());
        };
        let opened = fstat(&descriptor).map_err(map_rustix)?;
        if FileType::from_raw_mode(opened.st_mode) != FileType::Directory
            || opened.st_dev != metadata.st_dev
            || opened.st_ino != metadata.st_ino
            || opened.st_dev as u64 != root_device
        {
            return Err(CommandError::internal(
                "deletion target changed across its pinned boundary",
            ));
        }
        let mut entries = Dir::read_from(&descriptor).map_err(map_rustix)?;
        for entry in &mut entries {
            let entry = entry.map_err(map_rustix)?;
            let bytes = entry.file_name().to_bytes();
            if bytes == b"." || bytes == b".." {
                continue;
            }
            remove_entry_unix(&descriptor, OsStr::from_bytes(bytes), root_device, None)?;
        }
        drop(entries);
        fsync(&descriptor).map_err(map_rustix)?;
        drop(descriptor);
        match unlinkat(parent, name, AtFlags::REMOVEDIR) {
            Ok(()) | Err(Errno::NOENT) => Ok(()),
            Err(error) => Err(map_rustix(error)),
        }
    } else {
        match unlinkat(parent, name, AtFlags::empty()) {
            Ok(()) | Err(Errno::NOENT) => Ok(()),
            Err(error) => Err(map_rustix(error)),
        }
    }
}

#[cfg(windows)]
fn verify_directory_identity_windows(
    path: &Path,
    expected: &RootFilesystemIdentity,
) -> Result<(), CommandError> {
    let handle = crate::windows_fs::open_directory_no_reparse(path).map_err(map_io)?;
    let observed = crate::windows_fs::filesystem_identity(&handle).map_err(map_io)?;
    if &observed != expected {
        return Err(CommandError::conflict(
            "private storage identity changed during deletion",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn remove_entry_windows(path: &Path) -> Result<(), CommandError> {
    let mut metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(map_io(error)),
    };
    if metadata.file_type().is_symlink() || crate::windows_fs::metadata_is_reparse(&metadata) {
        return if metadata.is_dir() {
            fs::remove_dir(path).map_err(map_io)
        } else {
            fs::remove_file(path).map_err(map_io)
        };
    }
    if metadata.permissions().readonly() {
        let mut permissions = metadata.permissions();
        permissions.set_readonly(false);
        fs::set_permissions(path, permissions).map_err(map_io)?;
        metadata = fs::symlink_metadata(path).map_err(map_io)?;
        if metadata.file_type().is_symlink() || crate::windows_fs::metadata_is_reparse(&metadata) {
            return Err(CommandError::internal(
                "private storage changed while clearing read-only state",
            ));
        }
    }
    if metadata.is_dir() {
        let guard = crate::windows_fs::open_directory_no_reparse(path).map_err(map_io)?;
        let mut entries = fs::read_dir(path)
            .map_err(map_io)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_io)?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            remove_entry_windows(&entry.path())?;
        }
        drop(guard);
        fs::remove_dir(path).map_err(map_io)
    } else {
        let guard = crate::windows_fs::open_regular_no_reparse(path).map_err(map_io)?;
        drop(guard);
        fs::remove_file(path).map_err(map_io)
    }
}

#[cfg(unix)]
fn map_rustix(error: rustix::io::Errno) -> CommandError {
    map_io(std::io::Error::from(error))
}

fn map_io(error: std::io::Error) -> CommandError {
    CommandError::internal(format!("secure deletion boundary failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn removes_large_and_deep_trees_from_the_pinned_root() {
        let temporary = tempfile::tempdir().unwrap();
        let root = SecureDeletionRoot::open(&temporary.path().canonicalize().unwrap()).unwrap();
        let target = temporary.path().join("large");
        fs::create_dir(&target).unwrap();
        for index in 0..300 {
            fs::write(target.join(format!("{index}.txt")), b"cache").unwrap();
        }
        let mut deep = target.join("deep");
        for _ in 0..80 {
            fs::create_dir_all(&deep).unwrap();
            deep = deep.join("d");
        }
        #[cfg(unix)]
        {
            use std::os::unix::{fs::PermissionsExt, net::UnixListener};

            let socket = target.join("socket");
            let _listener = UnixListener::bind(&socket).unwrap();
            fs::set_permissions(target.join("deep"), fs::Permissions::from_mode(0o000)).unwrap();
        }

        root.remove_tree(Path::new("large")).unwrap();
        assert!(!target.exists());
    }

    #[cfg(unix)]
    #[test]
    fn removes_a_link_without_following_it() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().unwrap();
        let root = SecureDeletionRoot::open(&temporary.path().canonicalize().unwrap()).unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("sentinel"), b"keep").unwrap();
        symlink(outside.path(), temporary.path().join("linked")).unwrap();

        root.remove_tree(Path::new("linked")).unwrap();
        assert_eq!(fs::read(outside.path().join("sentinel")).unwrap(), b"keep");
    }

    #[cfg(unix)]
    #[test]
    fn pinned_root_ignores_an_ancestor_path_replacement() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().unwrap();
        let original = temporary.path().join("app-data");
        let moved = temporary.path().join("app-data-original");
        let outside = temporary.path().join("outside");
        fs::create_dir(&original).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(original.join("inside"), b"delete").unwrap();
        fs::write(outside.join("inside"), b"keep").unwrap();
        let root = SecureDeletionRoot::open(&original.canonicalize().unwrap()).unwrap();
        fs::rename(&original, &moved).unwrap();
        symlink(&outside, &original).unwrap();

        root.remove_tree(Path::new("inside")).unwrap();
        assert!(!moved.join("inside").exists());
        assert_eq!(fs::read(outside.join("inside")).unwrap(), b"keep");
    }

    #[cfg(unix)]
    #[test]
    fn expected_identity_refuses_a_swapped_guru_directory() {
        let temporary = tempfile::tempdir().unwrap();
        let root = SecureDeletionRoot::open(&temporary.path().canonicalize().unwrap()).unwrap();
        fs::create_dir(temporary.path().join("guru-a")).unwrap();
        fs::create_dir(temporary.path().join("guru-b")).unwrap();
        fs::write(temporary.path().join("guru-b/sentinel"), b"keep").unwrap();
        let expected = root
            .directory_identity(Path::new("guru-a"))
            .unwrap()
            .unwrap();
        fs::rename(
            temporary.path().join("guru-a"),
            temporary.path().join("guru-a-original"),
        )
        .unwrap();
        fs::rename(
            temporary.path().join("guru-b"),
            temporary.path().join("guru-a"),
        )
        .unwrap();

        assert!(root
            .rename_sibling_expected(
                Path::new("guru-a"),
                Path::new(".deleting-guru-a"),
                Some(&expected),
            )
            .is_err());
        assert_eq!(
            fs::read(temporary.path().join("guru-a/sentinel")).unwrap(),
            b"keep"
        );
        assert!(!temporary.path().join(".deleting-guru-a").exists());
    }
}
