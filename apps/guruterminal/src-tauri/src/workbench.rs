#[cfg(unix)]
use std::fs::File;
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

#[cfg(all(test, unix))]
use std::cell::Cell;

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

use serde_json::{json, Value};
use uuid::Uuid;

use crate::{broker::BrokerError, hashing::sha256};

#[cfg(windows)]
use crate::windows_fs::{move_file_no_replace, replace_file_with_backup};

pub(crate) const MAX_WORKBENCH_FILE_BYTES: usize = 512 * 1024;
pub(crate) const MAX_TOOL_OUTPUT_BYTES: usize = 50 * 1024;
pub(crate) const MAX_READ_LINES: u32 = 2_000;

static GURU_MUTATION_LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();

#[cfg(all(test, unix))]
thread_local! {
    static FORCE_DIRECTORY_FSYNC_ERROR: Cell<bool> = const { Cell::new(false) };
}

#[derive(Clone, Debug)]
pub(crate) struct WorkbenchStore {
    guru_id: String,
    root: PathBuf,
}

#[derive(Debug)]
enum WorkbenchError {
    Message(&'static str),
    Io(io::Error),
}

impl From<io::Error> for WorkbenchError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<WorkbenchError> for BrokerError {
    fn from(error: WorkbenchError) -> Self {
        match error {
            WorkbenchError::Message(message) => BrokerError::Execution(message.into()),
            WorkbenchError::Io(error) => BrokerError::Execution(error.to_string()),
        }
    }
}

impl WorkbenchStore {
    pub(crate) fn open(guru_id: impl Into<String>, root: PathBuf) -> Result<Self, BrokerError> {
        let metadata = fs::symlink_metadata(&root).map_err(WorkbenchError::from)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(WorkbenchError::Message("Workbench is not a directory").into());
        }
        let canonical = root.canonicalize().map_err(WorkbenchError::from)?;
        Ok(Self {
            guru_id: guru_id.into(),
            root: canonical,
        })
    }

    pub(crate) fn read(
        &self,
        input_path: &str,
        offset: Option<u32>,
        limit: Option<u32>,
    ) -> Result<Value, BrokerError> {
        let path = resolve_workbench_path(&self.root, input_path, false)?;
        let bytes = read_bounded_file(&path)?;
        let relative = canonical_relative(&self.root, &path)?;
        let revision = revision_token(&relative, &bytes);
        let text = String::from_utf8(bytes)
            .map_err(|_| WorkbenchError::Message("Workbench file is not a bounded regular file"))?;
        let lines: Vec<&str> = text.split('\n').collect();
        let start = offset.unwrap_or(1).saturating_sub(1) as usize;
        let take = limit.unwrap_or(MAX_READ_LINES) as usize;
        let selected = lines
            .iter()
            .skip(start)
            .take(take)
            .copied()
            .collect::<Vec<_>>()
            .join("\n");
        Ok(json!({
            "path": relative,
            "content": bounded_text(&selected),
            "total_lines": lines.len(),
            "revision": revision,
        }))
    }

    pub(crate) fn write(
        &self,
        input_path: &str,
        content: &str,
        expected_revision: Option<&str>,
    ) -> Result<Value, BrokerError> {
        let bytes = content.as_bytes();
        if bytes.len() > MAX_WORKBENCH_FILE_BYTES {
            return Err(WorkbenchError::Message("Workbench file is too large").into());
        }
        if let Some(expected) = expected_revision {
            validate_revision(expected)?;
        }
        let lock = mutation_lock(&self.guru_id);
        let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let path = resolve_workbench_path(&self.root, input_path, true)?;
        ensure_parent_directories(&self.root, &path)?;
        let path = resolve_workbench_path(&self.root, input_path, true)?;
        let relative = canonical_relative(&self.root, &path)?;
        let existed = match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(WorkbenchError::Message(
                        "Workbench tools do not follow symbolic links",
                    )
                    .into());
                }
                if !metadata.is_file() {
                    return Err(WorkbenchError::Message(
                        "Workbench file is not a bounded regular file",
                    )
                    .into());
                }
                true
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => return Err(WorkbenchError::Io(error).into()),
        };
        match (existed, expected_revision) {
            (false, Some(_)) => {
                return Err(WorkbenchError::Message("Workbench path does not exist").into());
            }
            (true, None) => {
                return conflict_result(&path, &relative);
            }
            (true, Some(expected)) => {
                let live = read_bounded_file(&path)?;
                if revision_token(&relative, &live) != expected {
                    return conflict_from_bytes(&relative, live);
                }
                atomic_replace(&self.root, &path, bytes, true)?;
            }
            (false, None) => match atomic_replace(&self.root, &path, bytes, false) {
                Ok(()) => {}
                Err(error @ WorkbenchError::Io(_)) => return Err(error.into()),
                Err(_) if path.is_file() => return conflict_result(&path, &relative),
                Err(error) => return Err(error.into()),
            },
        }
        Ok(write_ok(&relative, bytes))
    }

    pub(crate) fn edit(
        &self,
        input_path: &str,
        old_text: &str,
        new_text: &str,
        expected_revision: &str,
    ) -> Result<Value, BrokerError> {
        if old_text.is_empty() {
            return Err(BrokerError::Malformed);
        }
        validate_revision(expected_revision)?;
        let lock = mutation_lock(&self.guru_id);
        let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let path = resolve_workbench_path(&self.root, input_path, true)?;
        let relative = canonical_relative(&self.root, &path)?;
        let live = read_bounded_file(&path)?;
        if revision_token(&relative, &live) != expected_revision {
            return conflict_from_bytes(&relative, live);
        }
        let current = String::from_utf8(live)
            .map_err(|_| WorkbenchError::Message("Workbench file is not a bounded regular file"))?;
        let first = current
            .find(old_text)
            .ok_or(WorkbenchError::Message("old_text must match exactly once"))?;
        if current[first + old_text.len()..].contains(old_text) {
            return Err(WorkbenchError::Message("old_text must match exactly once").into());
        }
        let next = format!(
            "{}{}{}",
            &current[..first],
            new_text,
            &current[first + old_text.len()..]
        );
        if next.len() > MAX_WORKBENCH_FILE_BYTES {
            return Err(WorkbenchError::Message("Edited workbench file is too large").into());
        }
        atomic_replace(&self.root, &path, next.as_bytes(), true)?;
        Ok(write_ok(&relative, next.as_bytes()))
    }
}

fn mutation_lock(guru_id: &str) -> Arc<Mutex<()>> {
    let mut locks = GURU_MUTATION_LOCKS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    locks
        .entry(guru_id.to_owned())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

fn write_ok(relative: &str, bytes: &[u8]) -> Value {
    json!({
        "status": "ok",
        "path": relative,
        "bytes": bytes.len(),
        "revision": revision_token(relative, bytes),
    })
}

fn conflict_result(path: &Path, relative: &str) -> Result<Value, BrokerError> {
    let live = read_bounded_file(path)?;
    conflict_from_bytes(relative, live)
}

fn conflict_from_bytes(relative: &str, live: Vec<u8>) -> Result<Value, BrokerError> {
    Ok(json!({
        "status": "conflict",
        "path": relative,
        "revision": revision_token(relative, &live),
    }))
}

pub(crate) fn revision_token(relative: &str, bytes: &[u8]) -> String {
    let mut material = Vec::with_capacity(relative.len() + 1 + bytes.len());
    material.extend_from_slice(relative.as_bytes());
    material.push(0);
    material.extend_from_slice(bytes);
    sha256(&material)
}

fn validate_revision(value: &str) -> Result<(), BrokerError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        Ok(())
    } else {
        Err(BrokerError::Malformed)
    }
}

fn bounded_text(text: &str) -> String {
    let bytes = text.as_bytes();
    if bytes.len() <= MAX_TOOL_OUTPUT_BYTES {
        return text.to_owned();
    }
    let mut end = MAX_TOOL_OUTPUT_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n\n[Output truncated at 50KB]", &text[..end])
}

fn is_within(root: &Path, path: &Path) -> bool {
    path.starts_with(root)
}

fn is_attachment_path(root: &Path, path: &Path) -> bool {
    let attachments = root.join("attachments");
    path == attachments || path.starts_with(&attachments)
}

fn normalize_path(path: PathBuf) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => normalized.push(component),
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = normalized.pop();
            }
            Component::Normal(_) => normalized.push(component),
        }
    }
    normalized
}

fn resolve_candidate(root: &Path, input: &str) -> Result<PathBuf, WorkbenchError> {
    if input.is_empty() || input.contains('\0') {
        return Err(WorkbenchError::Message("Workbench path is invalid"));
    }
    let requested = Path::new(input);
    if requested.is_absolute() {
        return Err(WorkbenchError::Message("Workbench path must be relative"));
    }
    Ok(normalize_path(root.join(requested)))
}

fn nearest_existing(path: &Path) -> Result<PathBuf, WorkbenchError> {
    let mut current = path.to_path_buf();
    loop {
        match fs::symlink_metadata(&current) {
            Ok(_) => return Ok(current),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if !current.pop() {
                    return Err(WorkbenchError::Message("Workbench path is invalid"));
                }
            }
            Err(error) => return Err(WorkbenchError::Io(error)),
        }
    }
}

fn resolve_workbench_path(
    root: &Path,
    input: &str,
    write: bool,
) -> Result<PathBuf, WorkbenchError> {
    let candidate = resolve_candidate(root, input)?;
    if !is_within(root, &candidate) {
        return Err(WorkbenchError::Message(
            "Path is outside this Guru's workbench",
        ));
    }
    if write && is_attachment_path(root, &candidate) {
        return Err(WorkbenchError::Message(
            "App-owned attachment snapshots are read-only",
        ));
    }
    let existing = nearest_existing(&candidate)?;
    let canonical_existing = fs::canonicalize(&existing).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            WorkbenchError::Message("Workbench path does not exist")
        } else {
            WorkbenchError::Io(error)
        }
    })?;
    if !is_within(root, &canonical_existing) {
        return Err(WorkbenchError::Message(
            "Path escapes this Guru's workbench through a symlink",
        ));
    }
    if write && is_attachment_path(root, &canonical_existing) {
        return Err(WorkbenchError::Message(
            "App-owned attachment snapshots are read-only",
        ));
    }
    if !write && existing != candidate {
        return Err(WorkbenchError::Message("Workbench path does not exist"));
    }
    if existing == candidate {
        let metadata = fs::symlink_metadata(&existing)?;
        if metadata.file_type().is_symlink() {
            return Err(WorkbenchError::Message(
                "Workbench tools do not follow symbolic links",
            ));
        }
    }
    Ok(candidate)
}

fn canonical_relative(root: &Path, path: &Path) -> Result<String, WorkbenchError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| WorkbenchError::Message("Path is outside this Guru's workbench"))?;
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(name) => {
                let name = name
                    .to_str()
                    .ok_or(WorkbenchError::Message("Workbench path is invalid"))?;
                if name.is_empty() || name.contains('\0') {
                    return Err(WorkbenchError::Message("Workbench path is invalid"));
                }
                parts.push(name);
            }
            _ => return Err(WorkbenchError::Message("Workbench path is invalid")),
        }
    }
    if parts.is_empty() {
        return Err(WorkbenchError::Message("Workbench path is invalid"));
    }
    Ok(parts.join("/"))
}

fn read_bounded_file(path: &Path) -> Result<Vec<u8>, BrokerError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            WorkbenchError::Message("Workbench path does not exist")
        } else {
            WorkbenchError::Io(error)
        }
    })?;
    if metadata.file_type().is_symlink() {
        return Err(WorkbenchError::Message("Workbench tools do not follow symbolic links").into());
    }
    if !metadata.is_file() || metadata.len() > MAX_WORKBENCH_FILE_BYTES as u64 {
        return Err(WorkbenchError::Message("Workbench file is not a bounded regular file").into());
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000);
    }
    let mut file = options.open(path).map_err(WorkbenchError::from)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(WorkbenchError::from)?;
    if bytes.len() > MAX_WORKBENCH_FILE_BYTES {
        return Err(WorkbenchError::Message("Workbench file is not a bounded regular file").into());
    }
    Ok(bytes)
}

fn ensure_parent_directories(root: &Path, file: &Path) -> Result<(), WorkbenchError> {
    let relative = file
        .strip_prefix(root)
        .map_err(|_| WorkbenchError::Message("Path is outside this Guru's workbench"))?;
    let mut current = root.to_path_buf();
    let components: Vec<_> = relative.components().collect();
    if components.len() < 2 {
        return Ok(());
    }
    for component in &components[..components.len() - 1] {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(WorkbenchError::Message(
                    "Workbench tools do not follow symbolic links",
                ));
            }
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                return Err(WorkbenchError::Message(
                    "Workbench file is not a bounded regular file",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                #[cfg(unix)]
                fs::DirBuilder::new().mode(0o700).create(&current)?;
                #[cfg(not(unix))]
                fs::create_dir(&current)?;
            }
            Err(error) => return Err(WorkbenchError::Io(error)),
        }
    }
    if !is_within(root, &fs::canonicalize(&current)?) {
        return Err(WorkbenchError::Message(
            "Path escapes this Guru's workbench through a symlink",
        ));
    }
    Ok(())
}

fn atomic_replace(
    root: &Path,
    target: &Path,
    bytes: &[u8],
    replace: bool,
) -> Result<(), WorkbenchError> {
    let parent = target
        .parent()
        .ok_or(WorkbenchError::Message("Workbench path is invalid"))?;
    let canonical_parent = fs::canonicalize(parent)?;
    if !is_within(root, &canonical_parent) {
        return Err(WorkbenchError::Message(
            "Path escapes this Guru's workbench through a symlink",
        ));
    }
    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty() && *name != "." && *name != "..")
        .ok_or(WorkbenchError::Message("Workbench path is invalid"))?;
    #[cfg(unix)]
    {
        unix_atomic_replace(&canonical_parent, file_name, bytes, replace)
    }
    #[cfg(windows)]
    {
        let canonical_target = canonical_parent.join(file_name);
        windows_atomic_replace(
            &canonical_parent,
            file_name,
            &canonical_target,
            bytes,
            replace,
        )
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (parent, file_name);
        let mut options = OpenOptions::new();
        options.write(true);
        if replace {
            options.create(true).truncate(true);
        } else {
            options.create_new(true);
        }
        let mut file = options.open(target)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    }
}

#[cfg(unix)]
fn unix_atomic_replace(
    parent: &Path,
    file_name: &str,
    bytes: &[u8],
    replace: bool,
) -> Result<(), WorkbenchError> {
    use rustix::{
        fs::{
            fsync, open, openat, renameat, renameat_with, unlinkat, AtFlags, Mode, OFlags,
            RenameFlags,
        },
        io::Errno,
    };

    let directory = open(
        parent,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(map_errno)?;
    let temporary_name = format!(".{file_name}.{:x}.tmp", Uuid::new_v4().as_u128());
    let descriptor = openat(
        &directory,
        temporary_name.as_str(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    )
    .map_err(map_errno)?;
    let persist = (|| -> Result<(), WorkbenchError> {
        let mut file = File::from(descriptor);
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        if replace {
            renameat(&directory, temporary_name.as_str(), &directory, file_name)
                .map_err(map_errno)?;
        } else {
            match renameat_with(
                &directory,
                temporary_name.as_str(),
                &directory,
                file_name,
                RenameFlags::NOREPLACE,
            ) {
                Ok(()) => {}
                Err(Errno::EXIST) => {
                    return Err(WorkbenchError::Message(
                        "Workbench file is not a bounded regular file",
                    ));
                }
                Err(error) => return Err(WorkbenchError::Io(map_errno(error))),
            }
        }
        sync_directory(&directory)?;
        Ok(())
    })();
    if persist.is_err() {
        let _ = unlinkat(&directory, temporary_name.as_str(), AtFlags::empty());
        let _ = fsync(&directory);
    }
    persist
}

#[cfg(unix)]
fn sync_directory(directory: impl rustix::fd::AsFd) -> Result<(), WorkbenchError> {
    #[cfg(test)]
    if FORCE_DIRECTORY_FSYNC_ERROR.with(Cell::get) {
        FORCE_DIRECTORY_FSYNC_ERROR.with(|flag| flag.set(false));
        return Err(WorkbenchError::Io(io::Error::from_raw_os_error(libc::EIO)));
    }
    rustix::fs::fsync(directory)
        .map_err(map_errno)
        .map_err(WorkbenchError::from)
}

#[cfg(unix)]
fn map_errno(error: rustix::io::Errno) -> io::Error {
    io::Error::from_raw_os_error(error.raw_os_error())
}

#[cfg(windows)]
fn windows_atomic_replace(
    parent: &Path,
    file_name: &str,
    target: &Path,
    bytes: &[u8],
    replace: bool,
) -> Result<(), WorkbenchError> {
    let temporary = parent.join(format!(".{file_name}.{:x}.tmp", Uuid::new_v4().as_u128()));
    let persist = (|| -> Result<(), WorkbenchError> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.custom_flags(0x0020_0000);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        if replace {
            let backup = parent.join(format!(".{file_name}.{:x}.bak", Uuid::new_v4().as_u128()));
            replace_file_with_backup(target, &temporary, &backup).map_err(WorkbenchError::Io)?;
            let _ = fs::remove_file(backup);
        } else {
            move_file_no_replace(&temporary, target).map_err(WorkbenchError::Io)?;
        }
        Ok(())
    })();
    if persist.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    persist
}

#[cfg(test)]
mod tests;
