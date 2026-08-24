#[cfg(unix)]
use rustix::{
    fd::OwnedFd,
    fs::{fstat, open, openat, Dir, FileType, Mode, OFlags},
    io::Errno,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[cfg(not(unix))]
use std::fs::OpenOptions;
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read},
    path::{Component, Path, PathBuf},
};
#[cfg(unix)]
use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
use thiserror::Error;

use crate::domain::CanonicalMemoryKind;
#[cfg(unix)]
use crate::pinned_root::{PinnedGuruRoot, PinnedRootError};
#[cfg(windows)]
use crate::windows_fs::{
    add_open_reparse_point_flag, ensure_no_reparse_points, metadata_is_reparse,
};

const MAX_MEMORY_FILE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_MEMORY_TREE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_MEMORY_ENTRIES: usize = 10_000;
const MAX_MEMORY_DEPTH: usize = 64;

#[derive(Debug, Error)]
pub enum SnapshotError {
    #[error("Guru Memory root is missing")]
    MissingMemory,
    #[error("memory snapshot contains an unsupported filesystem entry")]
    UnsupportedEntry,
    #[error("memory snapshot I/O failed: {0}")]
    Io(#[from] io::Error),
    #[cfg(unix)]
    #[error("pinned Guru root failed: {0}")]
    PinnedRoot(#[from] PinnedRootError),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotRecord {
    pub relative_path: String,
    pub content_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromotedTreeInspection {
    pub current_revision: String,
    pub reconstructed_base_revision: String,
    pub target_content_sha256: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromotedChangeSetInspection {
    pub current_revision: String,
    pub reconstructed_base_revision: String,
    pub target_content_sha256: BTreeMap<PathBuf, String>,
}

pub fn inspect_memory_tree(
    workspace: &Path,
) -> Result<(String, Vec<SnapshotRecord>), SnapshotError> {
    inspection_from_capture(capture_memory_tree(workspace)?)
}

#[cfg(unix)]
pub fn inspect_memory_tree_at(
    root: &PinnedGuruRoot,
) -> Result<(String, Vec<SnapshotRecord>), SnapshotError> {
    inspection_from_capture(capture_memory_tree_at(root)?)
}

fn inspection_from_capture(
    (revision, captured): (String, Vec<CapturedMemoryRecord>),
) -> Result<(String, Vec<SnapshotRecord>), SnapshotError> {
    Ok((
        revision,
        captured
            .into_iter()
            .map(|record| SnapshotRecord {
                relative_path: record.relative_text,
                content_sha256: hex::encode(Sha256::digest(&record.bytes)),
            })
            .collect(),
    ))
}

/// Reconstructs the tree digest as if one record still contained its pre-write
/// bytes (or did not exist). This lets crash recovery prove that the written
/// target is the only filesystem difference from the prepared base without
/// mutating canonical memory a second time.
pub fn inspect_memory_tree_with_record_override(
    workspace: &Path,
    target_relative_path: &Path,
    replacement: Option<&[u8]>,
) -> Result<String, SnapshotError> {
    if target_relative_path.is_absolute()
        || target_relative_path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || target_relative_path
            .extension()
            .and_then(|value| value.to_str())
            != Some("md")
    {
        return Err(SnapshotError::UnsupportedEntry);
    }
    let memory_relative = target_relative_path
        .strip_prefix("guruterminal")
        .map_err(|_| SnapshotError::UnsupportedEntry)?;
    if memory_relative.components().count() < 2 {
        return Err(SnapshotError::UnsupportedEntry);
    }
    inspect_memory_tree_inner(workspace, Some((memory_relative, replacement))).map(|value| value.0)
}

#[cfg(unix)]
pub fn inspect_memory_tree_with_record_override_at(
    root: &PinnedGuruRoot,
    target_relative_path: &Path,
    replacement: Option<&[u8]>,
) -> Result<String, SnapshotError> {
    let memory_relative = checked_memory_relative_path(target_relative_path)?;
    inspect_captured_memory_tree(
        capture_memory_tree_at(root)?.1,
        Some((&memory_relative, replacement)),
    )
    .map(|value| value.0)
}

/// Captures the promoted tree once and derives both its current revision and
/// the approved base revision from those exact bytes. The target digest is
/// returned from the same capture so a receipt cannot bind a candidate to a
/// different target that happened to be present during a later tree scan.
pub fn inspect_promoted_memory_tree(
    workspace: &Path,
    target_relative_path: &Path,
    base_replacement: Option<&[u8]>,
) -> Result<PromotedTreeInspection, SnapshotError> {
    let memory_relative = checked_memory_relative_path(target_relative_path)?;
    let (_, captured) = capture_memory_tree(workspace)?;

    inspect_promoted_capture(captured, &memory_relative, base_replacement)
}

#[cfg(unix)]
pub fn inspect_promoted_memory_tree_at(
    root: &PinnedGuruRoot,
    target_relative_path: &Path,
    base_replacement: Option<&[u8]>,
) -> Result<PromotedTreeInspection, SnapshotError> {
    let memory_relative = checked_memory_relative_path(target_relative_path)?;
    let (_, captured) = capture_memory_tree_at(root)?;
    inspect_promoted_capture(captured, &memory_relative, base_replacement)
}

fn inspect_promoted_capture(
    captured: Vec<CapturedMemoryRecord>,
    memory_relative: &Path,
    base_replacement: Option<&[u8]>,
) -> Result<PromotedTreeInspection, SnapshotError> {
    let inspection = inspect_promoted_replacements(
        captured,
        BTreeMap::from([(memory_relative.to_path_buf(), base_replacement)]),
    )?;
    Ok(PromotedTreeInspection {
        current_revision: inspection.current_revision,
        reconstructed_base_revision: inspection.reconstructed_base_revision,
        target_content_sha256: inspection
            .target_content_sha256
            .get(memory_relative)
            .cloned(),
    })
}

pub fn inspect_promoted_memory_change_set(
    workspace: &Path,
    targets: &[(&Path, Option<&[u8]>)],
) -> Result<PromotedChangeSetInspection, SnapshotError> {
    let (_, captured) = capture_memory_tree(workspace)?;
    inspect_promoted_change_set_capture(captured, targets)
}

#[cfg(unix)]
pub fn inspect_promoted_memory_change_set_at(
    root: &PinnedGuruRoot,
    targets: &[(&Path, Option<&[u8]>)],
) -> Result<PromotedChangeSetInspection, SnapshotError> {
    let (_, captured) = capture_memory_tree_at(root)?;
    inspect_promoted_change_set_capture(captured, targets)
}

fn inspect_promoted_change_set_capture(
    captured: Vec<CapturedMemoryRecord>,
    targets: &[(&Path, Option<&[u8]>)],
) -> Result<PromotedChangeSetInspection, SnapshotError> {
    if targets.is_empty() || targets.len() > 24 {
        return Err(SnapshotError::UnsupportedEntry);
    }
    let mut replacements = BTreeMap::new();
    for (path, replacement) in targets {
        let relative = checked_memory_relative_path(path)?;
        if replacements.insert(relative, *replacement).is_some() {
            return Err(SnapshotError::UnsupportedEntry);
        }
    }
    inspect_promoted_replacements(captured, replacements)
}

fn inspect_promoted_replacements(
    captured: Vec<CapturedMemoryRecord>,
    replacements: BTreeMap<PathBuf, Option<&[u8]>>,
) -> Result<PromotedChangeSetInspection, SnapshotError> {
    let mut current_tree = Sha256::new();
    let mut reconstructed = BTreeMap::new();
    let mut target_content_sha256 = BTreeMap::new();
    for record in captured {
        hash_tree_record(&mut current_tree, &record.relative_text, &record.bytes);
        match replacements.get(&record.relative_path) {
            Some(base_replacement) => {
                target_content_sha256.insert(
                    record.relative_path.clone(),
                    hex::encode(Sha256::digest(&record.bytes)),
                );
                if let Some(replacement) = base_replacement {
                    reconstructed.insert(
                        record.relative_path,
                        (record.relative_text, replacement.to_vec()),
                    );
                }
            }
            None => {
                reconstructed.insert(record.relative_path, (record.relative_text, record.bytes));
            }
        }
    }
    for (relative, replacement) in replacements {
        if target_content_sha256.contains_key(&relative) {
            continue;
        }
        let Some(bytes) = replacement else {
            return Err(SnapshotError::UnsupportedEntry);
        };
        reconstructed.insert(
            relative.clone(),
            (normalized_relative_text(&relative)?, bytes.to_vec()),
        );
    }

    let mut reconstructed_tree = Sha256::new();
    for (relative_text, bytes) in reconstructed.values() {
        hash_tree_record(&mut reconstructed_tree, relative_text, bytes);
    }
    Ok(PromotedChangeSetInspection {
        current_revision: hex::encode(current_tree.finalize()),
        reconstructed_base_revision: hex::encode(reconstructed_tree.finalize()),
        target_content_sha256,
    })
}

fn checked_memory_relative_path(target_relative_path: &Path) -> Result<PathBuf, SnapshotError> {
    if target_relative_path.is_absolute()
        || target_relative_path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || target_relative_path
            .extension()
            .and_then(|value| value.to_str())
            != Some("md")
    {
        return Err(SnapshotError::UnsupportedEntry);
    }
    let memory_relative = target_relative_path
        .strip_prefix("guruterminal")
        .map_err(|_| SnapshotError::UnsupportedEntry)?;
    if memory_relative.components().count() < 2 {
        return Err(SnapshotError::UnsupportedEntry);
    }
    Ok(memory_relative.to_path_buf())
}

fn inspect_memory_tree_inner(
    workspace: &Path,
    record_override: Option<(&Path, Option<&[u8]>)>,
) -> Result<(String, Vec<SnapshotRecord>), SnapshotError> {
    inspect_captured_memory_tree(capture_memory_tree(workspace)?.1, record_override)
}

fn inspect_captured_memory_tree(
    mut captured: Vec<CapturedMemoryRecord>,
    record_override: Option<(&Path, Option<&[u8]>)>,
) -> Result<(String, Vec<SnapshotRecord>), SnapshotError> {
    if let Some((relative, replacement)) = record_override {
        match replacement {
            Some(bytes) => {
                if let Some(record) = captured
                    .iter_mut()
                    .find(|record| record.relative_path == relative)
                {
                    record.bytes = bytes.to_vec();
                } else {
                    captured.push(CapturedMemoryRecord {
                        relative_path: relative.to_path_buf(),
                        relative_text: normalized_relative_text(relative)?,
                        bytes: bytes.to_vec(),
                    });
                }
            }
            None => captured.retain(|record| record.relative_path != relative),
        }
    }
    captured.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));

    let mut records = Vec::with_capacity(captured.len());
    let mut tree = Sha256::new();
    for record in captured {
        hash_tree_record(&mut tree, &record.relative_text, &record.bytes);
        records.push(SnapshotRecord {
            relative_path: record.relative_text,
            content_sha256: hex::encode(Sha256::digest(&record.bytes)),
        });
    }
    Ok((hex::encode(tree.finalize()), records))
}

fn normalized_relative_text(relative: &Path) -> Result<String, SnapshotError> {
    Ok(relative
        .to_str()
        .ok_or(SnapshotError::UnsupportedEntry)?
        .replace(std::path::MAIN_SEPARATOR, "/"))
}

fn hash_tree_record(tree: &mut Sha256, relative_text: &str, bytes: &[u8]) {
    tree.update((relative_text.len() as u64).to_be_bytes());
    tree.update(relative_text.as_bytes());
    tree.update((bytes.len() as u64).to_be_bytes());
    tree.update(bytes);
}

struct CapturedMemoryRecord {
    relative_path: PathBuf,
    relative_text: String,
    bytes: Vec<u8>,
}

#[derive(Default)]
struct CaptureBudget {
    bytes: u64,
    entries: usize,
}

fn capture_memory_tree(
    workspace: &Path,
) -> Result<(String, Vec<CapturedMemoryRecord>), SnapshotError> {
    #[cfg(unix)]
    let captured = capture_records_from_memory_root(open_memory_root(workspace)?)?;

    #[cfg(not(unix))]
    let captured = {
        let source_root = workspace.join("guruterminal");
        #[cfg(windows)]
        ensure_no_reparse_points(&source_root).map_err(|_| SnapshotError::UnsupportedEntry)?;
        if !source_root.is_dir() {
            return Err(SnapshotError::MissingMemory);
        }
        let mut budget = CaptureBudget::default();
        let mut records = Vec::new();
        collect_markdown(&source_root, &source_root, 0, &mut budget, &mut records)?;
        records
    };

    finalize_capture(captured)
}

#[cfg(unix)]
fn capture_memory_tree_at(
    root: &PinnedGuruRoot,
) -> Result<(String, Vec<CapturedMemoryRecord>), SnapshotError> {
    let source_root = root
        .open_directory(Path::new("guruterminal"))
        .map_err(memory_root_error)?;
    finalize_capture(capture_records_from_memory_root(source_root)?)
}

#[cfg(unix)]
fn capture_records_from_memory_root(
    source_root: OwnedFd,
) -> Result<Vec<CapturedMemoryRecord>, SnapshotError> {
    let mut records = Vec::new();
    let mut budget = CaptureBudget::default();
    capture_directory(&source_root, Path::new(""), 0, &mut budget, &mut records)?;
    Ok(records)
}

fn finalize_capture(
    mut captured: Vec<CapturedMemoryRecord>,
) -> Result<(String, Vec<CapturedMemoryRecord>), SnapshotError> {
    captured.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    let mut tree = Sha256::new();
    for record in &captured {
        hash_tree_record(&mut tree, &record.relative_text, &record.bytes);
    }
    Ok((hex::encode(tree.finalize()), captured))
}

pub fn read_memory_record(
    workspace: &Path,
    target_relative_path: &Path,
) -> Result<Option<Vec<u8>>, SnapshotError> {
    let memory_relative = checked_memory_relative_path(target_relative_path)?;

    #[cfg(unix)]
    {
        read_memory_record_from_root(open_memory_root(workspace)?, &memory_relative)
    }

    #[cfg(not(unix))]
    {
        let source_root = workspace.join("guruterminal");
        let target = source_root.join(&memory_relative);
        let parent = target.parent().ok_or(SnapshotError::UnsupportedEntry)?;
        #[cfg(windows)]
        let _directory_guards = windows_target_directory_guards(workspace, &memory_relative)?;
        #[cfg(not(windows))]
        let canonical_root = source_root
            .canonicalize()
            .map_err(|_| SnapshotError::MissingMemory)?;
        #[cfg(not(windows))]
        let canonical_parent = match parent.canonicalize() {
            Ok(parent) => parent,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        #[cfg(not(windows))]
        if !canonical_parent.starts_with(&canonical_root) {
            return Err(SnapshotError::UnsupportedEntry);
        }
        match fs::symlink_metadata(&target) {
            Ok(_) => {
                let mut budget = CaptureBudget::default();
                read_snapshot_file(&target, &mut budget).map(Some)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}

#[cfg(unix)]
pub fn read_memory_record_at(
    root: &PinnedGuruRoot,
    target_relative_path: &Path,
) -> Result<Option<Vec<u8>>, SnapshotError> {
    let memory_relative = checked_memory_relative_path(target_relative_path)?;
    let memory_root = root
        .open_directory(Path::new("guruterminal"))
        .map_err(memory_root_error)?;
    read_memory_record_from_root(memory_root, &memory_relative)
}

#[cfg(unix)]
fn read_memory_record_from_root(
    mut directory: OwnedFd,
    memory_relative: &Path,
) -> Result<Option<Vec<u8>>, SnapshotError> {
    let components = memory_relative
        .components()
        .map(|component| component.as_os_str().to_owned())
        .collect::<Vec<_>>();
    if components.len() > MAX_MEMORY_DEPTH + 1 {
        return Err(SnapshotError::UnsupportedEntry);
    }
    let Some((file_name, parents)) = components.split_last() else {
        return Err(SnapshotError::UnsupportedEntry);
    };
    for parent in parents {
        directory = match openat(&directory, parent, directory_open_flags(), Mode::empty()) {
            Ok(descriptor) => descriptor,
            Err(Errno::NOENT) => return Ok(None),
            Err(error) => return Err(snapshot_open_error(error)),
        };
        require_file_type(&directory, FileType::Directory)?;
    }
    let descriptor = match openat(&directory, file_name, entry_open_flags(), Mode::empty()) {
        Ok(descriptor) => descriptor,
        Err(Errno::NOENT) => return Ok(None),
        Err(error) => return Err(snapshot_open_error(error)),
    };
    let mut budget = CaptureBudget::default();
    read_bounded_record(descriptor, &mut budget).map(Some)
}

#[cfg(unix)]
fn open_memory_root(workspace: &Path) -> Result<OwnedFd, SnapshotError> {
    let workspace =
        open(workspace, directory_open_flags(), Mode::empty()).map_err(snapshot_open_error)?;
    require_file_type(&workspace, FileType::Directory)?;
    let memory = openat(
        &workspace,
        OsStr::new("guruterminal"),
        directory_open_flags(),
        Mode::empty(),
    )
    .map_err(|error| {
        if error == Errno::NOENT {
            SnapshotError::MissingMemory
        } else {
            snapshot_open_error(error)
        }
    })?;
    require_file_type(&memory, FileType::Directory)?;
    Ok(memory)
}

#[cfg(unix)]
fn capture_directory(
    directory: &OwnedFd,
    relative_parent: &Path,
    depth: usize,
    budget: &mut CaptureBudget,
    output: &mut Vec<CapturedMemoryRecord>,
) -> Result<(), SnapshotError> {
    if depth > MAX_MEMORY_DEPTH {
        return Err(SnapshotError::UnsupportedEntry);
    }
    let mut entries = Dir::read_from(directory).map_err(snapshot_open_error)?;
    for entry in &mut entries {
        let entry = entry.map_err(snapshot_open_error)?;
        let name_bytes = entry.file_name().to_bytes();
        if name_bytes == b"." || name_bytes == b".." {
            continue;
        }
        if budget.entries >= MAX_MEMORY_ENTRIES {
            return Err(SnapshotError::UnsupportedEntry);
        }
        budget.entries += 1;
        let name = OsStr::from_bytes(name_bytes);
        if name.to_str().is_none() {
            return Err(SnapshotError::UnsupportedEntry);
        }
        let descriptor = openat(directory, name, entry_open_flags(), Mode::empty())
            .map_err(snapshot_open_error)?;
        let metadata = fstat(&descriptor).map_err(snapshot_open_error)?;
        let file_type = FileType::from_raw_mode(metadata.st_mode);
        let relative_path = relative_parent.join(name);
        if file_type == FileType::Directory {
            if depth == 0
                && name
                    .to_str()
                    .and_then(CanonicalMemoryKind::from_slug)
                    .is_none()
            {
                return Err(SnapshotError::UnsupportedEntry);
            }
            capture_directory(&descriptor, &relative_path, depth + 1, budget, output)?;
        } else if file_type == FileType::RegularFile
            && relative_path.extension().and_then(|value| value.to_str()) == Some("md")
        {
            let bytes = read_bounded_record(descriptor, budget)?;
            output.push(CapturedMemoryRecord {
                relative_text: normalized_relative_text(&relative_path)?,
                relative_path,
                bytes,
            });
        } else {
            return Err(SnapshotError::UnsupportedEntry);
        }
    }
    Ok(())
}

#[cfg(unix)]
fn read_bounded_record(
    descriptor: OwnedFd,
    budget: &mut CaptureBudget,
) -> Result<Vec<u8>, SnapshotError> {
    let metadata = fstat(&descriptor).map_err(snapshot_open_error)?;
    if FileType::from_raw_mode(metadata.st_mode) != FileType::RegularFile
        || metadata.st_size < 0
        || metadata.st_size as u64 > MAX_MEMORY_FILE_BYTES
        || budget.bytes.saturating_add(metadata.st_size as u64) > MAX_MEMORY_TREE_BYTES
    {
        return Err(SnapshotError::UnsupportedEntry);
    }
    let advertised_size = metadata.st_size as usize;
    let mut file = fs::File::from(descriptor);
    let mut bytes = Vec::with_capacity(advertised_size);
    (&mut file)
        .take(MAX_MEMORY_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    let after = fstat(&file).map_err(snapshot_open_error)?;
    if bytes.len() > MAX_MEMORY_FILE_BYTES as usize
        || bytes.len() != advertised_size
        || after.st_size != metadata.st_size
    {
        return Err(SnapshotError::UnsupportedEntry);
    }
    budget.bytes += bytes.len() as u64;
    Ok(bytes)
}

#[cfg(unix)]
fn require_file_type(descriptor: &OwnedFd, expected: FileType) -> Result<(), SnapshotError> {
    let metadata = fstat(descriptor).map_err(snapshot_open_error)?;
    if FileType::from_raw_mode(metadata.st_mode) != expected {
        return Err(SnapshotError::UnsupportedEntry);
    }
    Ok(())
}

#[cfg(unix)]
fn directory_open_flags() -> OFlags {
    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC
}

#[cfg(unix)]
fn entry_open_flags() -> OFlags {
    OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC
}

#[cfg(unix)]
fn snapshot_open_error(error: Errno) -> SnapshotError {
    match error {
        Errno::LOOP | Errno::NOTDIR => SnapshotError::UnsupportedEntry,
        _ => SnapshotError::Io(io::Error::from(error)),
    }
}

#[cfg(unix)]
fn memory_root_error(error: PinnedRootError) -> SnapshotError {
    match error {
        PinnedRootError::NotDirectory => SnapshotError::MissingMemory,
        PinnedRootError::InvalidPath => SnapshotError::UnsupportedEntry,
        PinnedRootError::Io(error)
            if error
                .raw_os_error()
                .is_some_and(|code| matches!(code, libc::ENOENT | libc::ENOTDIR)) =>
        {
            SnapshotError::MissingMemory
        }
        PinnedRootError::Io(error) if error.raw_os_error() == Some(libc::ELOOP) => {
            SnapshotError::UnsupportedEntry
        }
        other => SnapshotError::PinnedRoot(other),
    }
}

#[cfg(not(unix))]
fn read_snapshot_file(path: &Path, budget: &mut CaptureBudget) -> Result<Vec<u8>, SnapshotError> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || windows_metadata_is_reparse(&metadata)
        || !metadata.is_file()
        || metadata.len() > MAX_MEMORY_FILE_BYTES
        || budget.bytes.saturating_add(metadata.len()) > MAX_MEMORY_TREE_BYTES
    {
        return Err(SnapshotError::UnsupportedEntry);
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    add_open_reparse_point_flag(&mut options);
    let mut file = options.open(path)?;
    let opened_metadata = file.metadata()?;
    if !opened_metadata.is_file() || windows_metadata_is_reparse(&opened_metadata) {
        return Err(SnapshotError::UnsupportedEntry);
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    (&mut file)
        .take(MAX_MEMORY_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_MEMORY_FILE_BYTES as usize || bytes.len() as u64 != metadata.len() {
        return Err(SnapshotError::UnsupportedEntry);
    }
    budget.bytes += bytes.len() as u64;
    Ok(bytes)
}

#[cfg(not(unix))]
fn collect_markdown(
    root: &Path,
    directory: &Path,
    depth: usize,
    budget: &mut CaptureBudget,
    output: &mut Vec<CapturedMemoryRecord>,
) -> Result<(), SnapshotError> {
    if depth > MAX_MEMORY_DEPTH {
        return Err(SnapshotError::UnsupportedEntry);
    }
    #[cfg(windows)]
    let _directory_guard = crate::windows_fs::open_directory_no_reparse(directory)
        .map_err(|_| SnapshotError::UnsupportedEntry)?;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if budget.entries >= MAX_MEMORY_ENTRIES {
            return Err(SnapshotError::UnsupportedEntry);
        }
        budget.entries += 1;
        let file_type = entry.file_type()?;
        let metadata = fs::symlink_metadata(entry.path())?;
        if file_type.is_symlink() || windows_metadata_is_reparse(&metadata) {
            return Err(SnapshotError::UnsupportedEntry);
        }
        if file_type.is_dir() {
            if depth == 0
                && entry
                    .file_name()
                    .to_str()
                    .and_then(CanonicalMemoryKind::from_slug)
                    .is_none()
            {
                return Err(SnapshotError::UnsupportedEntry);
            }
            collect_markdown(root, &entry.path(), depth + 1, budget, output)?;
        } else if file_type.is_file()
            && entry.path().extension().and_then(|value| value.to_str()) == Some("md")
        {
            let relative_path = entry
                .path()
                .strip_prefix(root)
                .map_err(|_| SnapshotError::UnsupportedEntry)?
                .to_path_buf();
            let bytes = read_snapshot_file(&entry.path(), budget)?;
            output.push(CapturedMemoryRecord {
                relative_text: normalized_relative_text(&relative_path)?,
                relative_path,
                bytes,
            });
        } else {
            return Err(SnapshotError::UnsupportedEntry);
        }
    }
    Ok(())
}

#[cfg(windows)]
fn windows_target_directory_guards(
    workspace: &Path,
    memory_relative: &Path,
) -> Result<Vec<fs::File>, SnapshotError> {
    let mut guards = vec![crate::windows_fs::open_directory_no_reparse(workspace)
        .map_err(|_| SnapshotError::UnsupportedEntry)?];
    let mut directory = workspace.join("guruterminal");
    guards.push(
        crate::windows_fs::open_directory_no_reparse(&directory)
            .map_err(|_| SnapshotError::UnsupportedEntry)?,
    );
    let parent_count = memory_relative.components().count().saturating_sub(1);
    for component in memory_relative.components().take(parent_count) {
        directory.push(component.as_os_str());
        guards.push(
            crate::windows_fs::open_directory_no_reparse(&directory)
                .map_err(|_| SnapshotError::UnsupportedEntry)?,
        );
    }
    Ok(guards)
}

#[cfg(windows)]
fn windows_metadata_is_reparse(metadata: &fs::Metadata) -> bool {
    metadata_is_reparse(metadata)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    #[cfg(unix)]
    #[test]
    fn pinned_reads_and_snapshot_stay_in_a_after_path_is_replaced_by_b() {
        let temporary = tempfile::tempdir().unwrap();
        let root_a = temporary.path().join("guru-a");
        let root_b = temporary.path().join("guru-b");
        let moved_a = temporary.path().join("guru-a-original");
        fs::create_dir_all(root_a.join("guruterminal/wiki")).unwrap();
        fs::create_dir_all(root_b.join("guruterminal/wiki")).unwrap();
        fs::write(root_a.join("guruterminal/wiki/identity.md"), b"A").unwrap();
        fs::write(root_a.join("guruterminal/wiki/a-only.md"), b"A-only").unwrap();
        fs::write(root_b.join("guruterminal/wiki/identity.md"), b"B").unwrap();
        fs::write(root_b.join("guruterminal/wiki/b-only.md"), b"B-only").unwrap();
        let pinned = PinnedGuruRoot::open_unbound(&root_a).unwrap();

        fs::rename(&root_a, &moved_a).unwrap();
        fs::rename(&root_b, &root_a).unwrap();

        assert_eq!(
            read_memory_record_at(&pinned, Path::new("guruterminal/wiki/identity.md"))
                .unwrap()
                .as_deref(),
            Some(b"A".as_slice())
        );
        let (pinned_revision, pinned_records) = inspect_memory_tree_at(&pinned).unwrap();
        assert_eq!(pinned_revision, inspect_memory_tree(&moved_a).unwrap().0);
        assert_ne!(pinned_revision, inspect_memory_tree(&root_a).unwrap().0);
        assert!(pinned_records
            .iter()
            .any(|record| record.relative_path == "wiki/a-only.md"));
        assert!(!pinned_records
            .iter()
            .any(|record| record.relative_path == "wiki/b-only.md"));

        assert_eq!(
            inspect_memory_tree_with_record_override_at(
                &pinned,
                Path::new("guruterminal/wiki/identity.md"),
                Some(b"replacement"),
            )
            .unwrap(),
            inspect_memory_tree_with_record_override(
                &moved_a,
                Path::new("guruterminal/wiki/identity.md"),
                Some(b"replacement"),
            )
            .unwrap()
        );
        let promoted = inspect_promoted_memory_tree_at(
            &pinned,
            Path::new("guruterminal/wiki/identity.md"),
            Some(b"before"),
        )
        .unwrap();
        assert_eq!(
            promoted.target_content_sha256.as_deref(),
            Some(hex::encode(Sha256::digest(b"A")).as_str())
        );

        assert_eq!(
            fs::read(root_a.join("guruterminal/wiki/identity.md")).unwrap(),
            b"B"
        );
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_rejects_symlinks() {
        use std::os::unix::fs::symlink;
        let workspace = tempfile::tempdir().unwrap();
        let source = workspace.path().join("guruterminal/wiki");
        fs::create_dir_all(&source).unwrap();
        let outside = workspace.path().join("outside.md");
        fs::write(&outside, b"outside").unwrap();
        symlink(&outside, source.join("linked.md")).unwrap();
        assert!(matches!(
            inspect_memory_tree(workspace.path()),
            Err(SnapshotError::UnsupportedEntry)
        ));
    }

    #[cfg(windows)]
    #[test]
    fn target_directory_guards_block_parent_replacement_through_read() {
        let workspace = tempfile::tempdir().unwrap();
        let wiki = workspace.path().join("guruterminal/wiki");
        fs::create_dir_all(&wiki).unwrap();
        fs::write(wiki.join("guarded.md"), b"guarded").unwrap();
        let guards =
            windows_target_directory_guards(workspace.path(), Path::new("wiki/guarded.md"))
                .unwrap();
        assert!(fs::rename(&wiki, workspace.path().join("wiki-moved")).is_err());
        assert_eq!(
            read_memory_record(workspace.path(), Path::new("guruterminal/wiki/guarded.md"))
                .unwrap()
                .as_deref(),
            Some(b"guarded".as_slice())
        );
        drop(guards);
    }

    #[cfg(unix)]
    #[test]
    fn target_reads_do_not_follow_a_symlinked_parent() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        fs::create_dir_all(workspace.path().join("guruterminal")).unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.md"), b"outside").unwrap();
        symlink(outside.path(), workspace.path().join("guruterminal/wiki")).unwrap();

        assert!(matches!(
            read_memory_record(workspace.path(), Path::new("guruterminal/wiki/secret.md")),
            Err(SnapshotError::UnsupportedEntry)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_and_target_reads_reject_fifo_and_oversized_records() {
        let workspace = tempfile::tempdir().unwrap();
        let source = workspace.path().join("guruterminal/wiki");
        fs::create_dir_all(&source).unwrap();

        let oversized = source.join("oversized.md");
        fs::write(&oversized, vec![b'x'; MAX_MEMORY_FILE_BYTES as usize + 1]).unwrap();
        assert!(matches!(
            inspect_memory_tree(workspace.path()),
            Err(SnapshotError::UnsupportedEntry)
        ));
        assert!(matches!(
            read_memory_record(
                workspace.path(),
                Path::new("guruterminal/wiki/oversized.md")
            ),
            Err(SnapshotError::UnsupportedEntry)
        ));
        fs::remove_file(oversized).unwrap();

        let fifo = source.join("blocked.md");
        let fifo_path = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: `fifo_path` is a NUL-terminated path owned for this call.
        assert_eq!(unsafe { libc::mkfifo(fifo_path.as_ptr(), 0o600) }, 0);
        assert!(matches!(
            inspect_memory_tree(workspace.path()),
            Err(SnapshotError::UnsupportedEntry)
        ));
        assert!(
            read_memory_record(workspace.path(), Path::new("guruterminal/wiki/blocked.md"))
                .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_rejects_more_than_the_bounded_entry_count() {
        let workspace = tempfile::tempdir().unwrap();
        let memory = workspace.path().join("guruterminal/wiki");
        fs::create_dir_all(&memory).unwrap();
        for index in 0..=MAX_MEMORY_ENTRIES {
            fs::create_dir(memory.join(format!("empty-{index:05}"))).unwrap();
        }

        assert!(matches!(
            inspect_memory_tree(workspace.path()),
            Err(SnapshotError::UnsupportedEntry)
        ));
    }

    #[test]
    fn snapshot_rejects_removed_memory_kinds() {
        let workspace = tempfile::tempdir().unwrap();
        fs::create_dir_all(workspace.path().join("guruterminal/method")).unwrap();

        assert!(matches!(
            inspect_memory_tree(workspace.path()),
            Err(SnapshotError::UnsupportedEntry)
        ));
    }

    #[test]
    fn record_override_reconstructs_the_exact_base_tree() {
        let workspace = tempfile::tempdir().unwrap();
        let lens = workspace.path().join("guruterminal/lens");
        let wiki = workspace.path().join("guruterminal/wiki");
        fs::create_dir_all(&lens).unwrap();
        fs::create_dir_all(&wiki).unwrap();
        fs::write(wiki.join("base.md"), b"base").unwrap();
        let original = inspect_memory_tree(workspace.path()).unwrap().0;

        fs::write(lens.join("trained.md"), b"proposed").unwrap();
        let reconstructed = inspect_memory_tree_with_record_override(
            workspace.path(),
            Path::new("guruterminal/lens/trained.md"),
            None,
        )
        .unwrap();
        assert_eq!(reconstructed, original);

        fs::write(wiki.join("base.md"), b"unrelated edit").unwrap();
        let conflicted = inspect_memory_tree_with_record_override(
            workspace.path(),
            Path::new("guruterminal/lens/trained.md"),
            None,
        )
        .unwrap();
        assert_ne!(conflicted, original);
    }

    #[test]
    fn promoted_inspection_binds_target_and_both_revisions_to_one_capture() {
        let workspace = tempfile::tempdir().unwrap();
        let lens = workspace.path().join("guruterminal/lens");
        let wiki = workspace.path().join("guruterminal/wiki");
        fs::create_dir_all(&lens).unwrap();
        fs::create_dir_all(&wiki).unwrap();
        fs::write(lens.join("trained.md"), b"before").unwrap();
        fs::write(wiki.join("base.md"), b"base").unwrap();
        let base_revision = inspect_memory_tree(workspace.path()).unwrap().0;

        fs::write(lens.join("trained.md"), b"proposed").unwrap();
        let inspection = inspect_promoted_memory_tree(
            workspace.path(),
            Path::new("guruterminal/lens/trained.md"),
            Some(b"before"),
        )
        .unwrap();

        assert_eq!(inspection.reconstructed_base_revision, base_revision);
        assert_eq!(
            inspection.target_content_sha256.as_deref(),
            Some(hex::encode(Sha256::digest(b"proposed")).as_str())
        );
        assert_eq!(
            inspection.current_revision,
            inspect_memory_tree(workspace.path()).unwrap().0
        );
    }

    #[test]
    fn promoted_inspection_reconstructs_a_deleted_target() {
        let workspace = tempfile::tempdir().unwrap();
        let wiki = workspace.path().join("guruterminal/wiki");
        fs::create_dir_all(&wiki).unwrap();
        fs::create_dir_all(workspace.path().join("guruterminal/lens")).unwrap();
        fs::write(wiki.join("kept.md"), b"kept").unwrap();
        fs::write(wiki.join("removed.md"), b"before").unwrap();
        let base_revision = inspect_memory_tree(workspace.path()).unwrap().0;

        fs::remove_file(wiki.join("removed.md")).unwrap();
        let inspection = inspect_promoted_memory_change_set(
            workspace.path(),
            &[(
                Path::new("guruterminal/wiki/removed.md"),
                Some(b"before".as_slice()),
            )],
        )
        .unwrap();
        let single = inspect_promoted_memory_tree(
            workspace.path(),
            Path::new("guruterminal/wiki/removed.md"),
            Some(b"before"),
        )
        .unwrap();

        assert_eq!(inspection.reconstructed_base_revision, base_revision);
        assert_eq!(single.reconstructed_base_revision, base_revision);
        assert!(!inspection
            .target_content_sha256
            .contains_key(Path::new("wiki/removed.md")));
        assert_eq!(single.target_content_sha256, None);
        assert_eq!(
            inspection.current_revision,
            inspect_memory_tree(workspace.path()).unwrap().0
        );
    }
}
