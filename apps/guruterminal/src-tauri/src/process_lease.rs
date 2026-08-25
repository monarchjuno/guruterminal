//! Durable ownership records for app-spawned process groups.
//!
//! A lease is written after `spawn` and before a sidecar is used. If Guru Terminal
//! itself is killed, the next instance can prove that a still-running process
//! group is the exact group it created before signaling it. The lease is not a
//! lock: the app-wide instance lock must already be held during recovery.

use serde::{Deserialize, Serialize};
#[cfg(unix)]
use sha2::{Digest, Sha256};
#[cfg(any(unix, windows))]
use std::time::{Duration, Instant};
#[cfg(unix)]
use std::{
    ffi::OsStr,
    io::{Read, Write},
    path::PathBuf,
};
use std::{fs, io, path::Path};
use thiserror::Error;
#[cfg(unix)]
use uuid::Uuid;

#[cfg(windows)]
use tokio::process::Child;
#[cfg(windows)]
use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError, ERROR_NO_MORE_FILES, HANDLE, INVALID_HANDLE_VALUE},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
        },
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAccountingInformation,
            JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
            TerminateJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        },
        Threading::{
            GetProcessIdOfThread, OpenThread, ResumeThread, CREATE_SUSPENDED,
            THREAD_QUERY_LIMITED_INFORMATION, THREAD_SUSPEND_RESUME,
        },
    },
};

#[cfg(unix)]
use rustix::{
    fd::OwnedFd,
    fs::{
        fchmod, fstat, fsync, open, openat, renameat, unlinkat, AtFlags, Dir, FileType, Mode,
        OFlags,
    },
    io::Errno,
};
#[cfg(unix)]
use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};

#[cfg(unix)]
const LEASE_SCHEMA: u32 = 1;
#[cfg(unix)]
const MAX_LEASE_BYTES: u64 = 16 * 1024;
#[cfg(unix)]
const MAX_LEASE_ENTRIES: usize = 64;
#[cfg(unix)]
const MAX_GROUP_MEMBERS: usize = 4096;
#[cfg(unix)]
const RECOVERY_TERM_GRACE: Duration = Duration::from_secs(1);
#[cfg(unix)]
const RECOVERY_KILL_GRACE: Duration = Duration::from_secs(2);
#[cfg(unix)]
const POLL_INTERVAL: Duration = Duration::from_millis(25);
#[cfg(windows)]
const WINDOWS_JOB_POLL_INTERVAL: Duration = Duration::from_millis(25);

#[derive(Debug, Error)]
pub enum ProcessLeaseError {
    // Keep the OS error visible: the message contains no pathname or secret,
    // and the errno is essential for distinguishing a disappearing process
    // group from a lease-filesystem failure.
    #[error("process lease I/O failed: {0}")]
    Io(#[source] io::Error),
    #[error("process lease directory or record is not private and regular")]
    UnsafeFilesystemEntry,
    #[error("process lease record is invalid")]
    InvalidRecord,
    #[error("process lease limit was exceeded")]
    LimitExceeded,
    #[error("leased process identity does not match the live process")]
    IdentityMismatch,
    #[error("a process group exists without a verifiable leased leader")]
    UnknownProcessGroup,
    #[error("leased process group did not stop")]
    StopTimeout,
    #[error("process identity is unsupported on this platform")]
    UnsupportedPlatform,
}

impl From<io::Error> for ProcessLeaseError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessKind {
    Pi,
    Finance,
    Compute,
    Mcp,
}

#[cfg(unix)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct StartIdentity {
    source: String,
    primary: u64,
    secondary: u64,
}

#[cfg(unix)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ExecutableIdentity {
    device: u64,
    inode: u64,
    path_sha256: String,
}

#[cfg(unix)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ProcessLeaseRecord {
    schema: u32,
    nonce: String,
    kind: ProcessKind,
    leader_pid: i32,
    process_group_id: i32,
    start: StartIdentity,
    executable: ExecutableIdentity,
}

#[cfg(unix)]
#[derive(Clone, Debug, Eq, PartialEq)]
struct ProcessIdentity {
    process_group_id: i32,
    start: StartIdentity,
    executable: ExecutableIdentity,
}

#[cfg(unix)]
struct ObservedProcess {
    identity: ProcessIdentity,
    zombie: bool,
}

/// A durable record for one child-led process group.
///
/// Dropping this value deliberately leaves the lease on disk. Call
/// [`ChildProcessLease::complete`] only after the whole group is confirmed
/// exited.
pub struct ChildProcessLease {
    #[cfg(unix)]
    directory: PathBuf,
    #[cfg(unix)]
    file_name: String,
    process_group_id: i32,
}

/// Owns a Windows child tree with `KILL_ON_JOB_CLOSE` semantics.
///
/// The handle is intentionally process-local. If Guru Terminal crashes, Windows
/// closes the handle and terminates every process assigned to the job, so no
/// durable recovery record is needed on this platform.
#[cfg(windows)]
pub struct ChildProcessJob {
    handle: HANDLE,
}

#[cfg(windows)]
// Windows kernel handles can be closed or used from any thread. This value is
// the unique owner and exposes only thread-safe Job Object operations.
unsafe impl Send for ChildProcessJob {}
#[cfg(windows)]
unsafe impl Sync for ChildProcessJob {}

#[cfg(windows)]
impl ChildProcessJob {
    /// Suspends the new child before user code can create descendants. `assign`
    /// resumes its primary thread only after Job Object ownership is installed.
    pub fn configure_command(command: &mut tokio::process::Command) {
        command.creation_flags(CREATE_SUSPENDED);
    }

    pub fn assign(child: &Child) -> Result<Self, ProcessLeaseError> {
        let process_id = child.id().ok_or(ProcessLeaseError::IdentityMismatch)?;
        let process = child
            .raw_handle()
            .ok_or(ProcessLeaseError::IdentityMismatch)? as HANDLE;
        // SAFETY: null security attributes and name request a private,
        // unnamed job object owned solely by this process.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error().into());
        }

        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: `limits` has the exact layout and size required by the
        // JobObjectExtendedLimitInformation information class.
        let configured = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if configured == 0 {
            let error = io::Error::last_os_error();
            // SAFETY: `handle` is live and owned by this function.
            unsafe { CloseHandle(handle) };
            return Err(error.into());
        }

        // SAFETY: both handles are live. The job handle remains owned by the
        // returned value for at least as long as the child tree is needed.
        if unsafe { AssignProcessToJobObject(handle, process) } == 0 {
            let error = io::Error::last_os_error();
            unsafe { CloseHandle(handle) };
            return Err(error.into());
        }
        let job = Self { handle };
        resume_primary_thread(process_id)?;
        Ok(job)
    }

    /// Terminates all live processes in the owned tree. The handle remains
    /// valid so the caller can wait for the leader before dropping ownership.
    pub fn terminate(&self) -> Result<(), ProcessLeaseError> {
        // SAFETY: the job handle is live for the lifetime of `self`.
        if unsafe { TerminateJobObject(self.handle, 1) } == 0 {
            Err(io::Error::last_os_error().into())
        } else {
            Ok(())
        }
    }

    /// Terminates the full job tree and does not return until Windows reports
    /// that no process remains active in the job.
    pub async fn terminate_and_wait(&self, deadline: Duration) -> Result<(), ProcessLeaseError> {
        self.terminate()?;
        let started = Instant::now();
        loop {
            if self.active_processes()? == 0 {
                return Ok(());
            }
            if started.elapsed() >= deadline {
                return Err(ProcessLeaseError::StopTimeout);
            }
            tokio::time::sleep(WINDOWS_JOB_POLL_INTERVAL).await;
        }
    }

    fn active_processes(&self) -> Result<u32, ProcessLeaseError> {
        let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        // SAFETY: `accounting` has the exact layout and size required by the
        // JobObjectBasicAccountingInformation information class.
        if unsafe {
            QueryInformationJobObject(
                self.handle,
                JobObjectBasicAccountingInformation,
                (&mut accounting as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                std::ptr::null_mut(),
            )
        } == 0
        {
            Err(io::Error::last_os_error().into())
        } else {
            Ok(accounting.ActiveProcesses)
        }
    }
}

#[cfg(windows)]
fn resume_primary_thread(process_id: u32) -> Result<(), ProcessLeaseError> {
    // SAFETY: this creates an owned snapshot handle with no input pointers.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error().into());
    }
    let result = (|| {
        let mut entry = THREADENTRY32 {
            dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        // SAFETY: `entry` is initialized with the required structure size.
        if unsafe { Thread32First(snapshot, &mut entry) } == 0 {
            return Err(ProcessLeaseError::Io(io::Error::last_os_error()));
        }
        let mut primary_thread_id = None;
        loop {
            if entry.th32OwnerProcessID == process_id {
                // CREATE_SUSPENDED creates exactly one initial thread. Refuse
                // ambiguous state instead of resuming an arbitrary injected or
                // instrumented thread.
                if primary_thread_id.replace(entry.th32ThreadID).is_some() {
                    return Err(ProcessLeaseError::IdentityMismatch);
                }
            }
            // SAFETY: `entry` remains valid for the next snapshot record.
            if unsafe { Thread32Next(snapshot, &mut entry) } == 0 {
                // SAFETY: this observes the error from the immediately
                // preceding Thread32Next call.
                if unsafe { GetLastError() } != ERROR_NO_MORE_FILES {
                    return Err(ProcessLeaseError::Io(io::Error::last_os_error()));
                }
                break;
            }
        }
        let primary_thread_id = primary_thread_id.ok_or(ProcessLeaseError::IdentityMismatch)?;
        // SAFETY: the id came from the snapshot and only resume/query rights are
        // requested. Ownership is rechecked on the opened handle below.
        let thread = unsafe {
            OpenThread(
                THREAD_SUSPEND_RESUME | THREAD_QUERY_LIMITED_INFORMATION,
                0,
                primary_thread_id,
            )
        };
        if thread.is_null() {
            return Err(ProcessLeaseError::Io(io::Error::last_os_error()));
        }
        let owner_process_id = unsafe { GetProcessIdOfThread(thread) };
        if owner_process_id == 0 {
            let error = io::Error::last_os_error();
            unsafe { CloseHandle(thread) };
            return Err(ProcessLeaseError::Io(error));
        }
        if owner_process_id != process_id {
            unsafe { CloseHandle(thread) };
            return Err(ProcessLeaseError::IdentityMismatch);
        }
        let resumed = unsafe { ResumeThread(thread) };
        unsafe { CloseHandle(thread) };
        if resumed == u32::MAX {
            Err(ProcessLeaseError::Io(io::Error::last_os_error()))
        } else if resumed == 0 {
            Err(ProcessLeaseError::IdentityMismatch)
        } else {
            Ok(())
        }
    })();
    // SAFETY: `snapshot` is owned by this function.
    unsafe { CloseHandle(snapshot) };
    result
}

#[cfg(windows)]
impl Drop for ChildProcessJob {
    fn drop(&mut self) {
        // Closing the final handle enforces JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE.
        // SAFETY: this value uniquely owns the handle.
        unsafe { CloseHandle(self.handle) };
    }
}

impl ChildProcessLease {
    pub fn register(
        directory: &Path,
        kind: ProcessKind,
        leader_pid: i32,
        process_group_id: i32,
        expected_executable: &Path,
    ) -> Result<Self, ProcessLeaseError> {
        #[cfg(unix)]
        {
            if leader_pid <= 1 || process_group_id != leader_pid {
                return Err(ProcessLeaseError::InvalidRecord);
            }
            prepare_lease_directory(directory)?;
            let observed = query_process(leader_pid)?
                .filter(|process| !process.zombie)
                .ok_or(ProcessLeaseError::IdentityMismatch)?;
            if observed.identity.process_group_id != process_group_id {
                return Err(ProcessLeaseError::IdentityMismatch);
            }
            require_expected_executable(expected_executable, &observed.identity.executable)?;

            let nonce = Uuid::new_v4().simple().to_string();
            let record = ProcessLeaseRecord {
                schema: LEASE_SCHEMA,
                nonce: nonce.clone(),
                kind,
                leader_pid,
                process_group_id,
                start: observed.identity.start,
                executable: observed.identity.executable,
            };
            let file_name = lease_file_name(&nonce);
            write_record_atomically(directory, &file_name, &record)?;
            Ok(Self {
                directory: directory.to_path_buf(),
                file_name,
                process_group_id,
            })
        }

        #[cfg(not(unix))]
        {
            let _ = (
                directory,
                kind,
                leader_pid,
                process_group_id,
                expected_executable,
            );
            Err(ProcessLeaseError::UnsupportedPlatform)
        }
    }

    /// Removes the durable lease only if no live member remains in the group.
    pub fn complete(self) -> Result<(), ProcessLeaseError> {
        #[cfg(unix)]
        {
            if process_group_state(self.process_group_id)? == GroupState::Live {
                return Err(ProcessLeaseError::StopTimeout);
            }
            remove_record(&self.directory, &self.file_name)
        }

        #[cfg(not(unix))]
        {
            Err(ProcessLeaseError::UnsupportedPlatform)
        }
    }
}

/// Creates and tightens the app-owned lease directory. This must run after the
/// single-instance lock is acquired.
pub fn prepare_lease_directory(directory: &Path) -> Result<(), ProcessLeaseError> {
    fs::create_dir_all(directory)?;
    #[cfg(unix)]
    {
        let descriptor = open_lease_directory(directory)?;
        let metadata = fstat(&descriptor).map_err(errno_io)?;
        if FileType::from_raw_mode(metadata.st_mode) != FileType::Directory
            || metadata.st_uid != effective_uid()
        {
            return Err(ProcessLeaseError::UnsafeFilesystemEntry);
        }
        fchmod(&descriptor, Mode::RWXU).map_err(errno_io)?;
        fsync(&descriptor).map_err(errno_io)?;
    }
    Ok(())
}

/// Recovers groups from a previous app process. The caller must hold the
/// app-wide single-instance lock. Every record is parsed and validated before
/// any group is signaled, so an unknown record fails the whole pass closed.
pub fn recover_orphaned_processes(directory: &Path) -> Result<(), ProcessLeaseError> {
    #[cfg(unix)]
    {
        prepare_lease_directory(directory)?;
        let leases = read_all_records(directory)?;
        let mut actions = Vec::with_capacity(leases.len());
        let mut groups = std::collections::BTreeSet::new();
        for (file_name, record) in leases {
            if !groups.insert(record.process_group_id) {
                return Err(ProcessLeaseError::InvalidRecord);
            }
            actions.push(classify_recovery(file_name, record)?);
        }

        for action in actions {
            match action {
                RecoveryAction::Remove { file_name } => remove_record(directory, &file_name)?,
                RecoveryAction::Terminate { file_name, record } => {
                    // Narrow the PID-reuse window by validating the leader again
                    // immediately before signaling its process group.
                    match query_process(record.leader_pid)? {
                        Some(observed) if record_matches(&record, &observed.identity) => {}
                        Some(_) => return Err(ProcessLeaseError::IdentityMismatch),
                        None => match process_group_state(record.process_group_id)? {
                            GroupState::Gone | GroupState::ZombiesOnly => {
                                remove_record(directory, &file_name)?;
                                continue;
                            }
                            GroupState::Live => return Err(ProcessLeaseError::UnknownProcessGroup),
                        },
                    }

                    signal_process_group(record.process_group_id, libc::SIGTERM)?;
                    if !wait_for_group_exit_blocking(record.process_group_id, RECOVERY_TERM_GRACE)?
                    {
                        signal_process_group(record.process_group_id, libc::SIGKILL)?;
                        if !wait_for_group_exit_blocking(
                            record.process_group_id,
                            RECOVERY_KILL_GRACE,
                        )? {
                            return Err(ProcessLeaseError::StopTimeout);
                        }
                    }
                    remove_record(directory, &file_name)?;
                }
            }
        }
        Ok(())
    }

    #[cfg(windows)]
    {
        // Windows Job Objects terminate their assigned tree when the owning
        // app handle closes, including hard crashes. Startup only needs to
        // establish the private directory used by the cross-platform layout.
        prepare_lease_directory(directory)
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = directory;
        Err(ProcessLeaseError::UnsupportedPlatform)
    }
}

/// Signals a process group already owned and held by the current app process.
#[cfg(unix)]
pub fn signal_process_group(process_group_id: i32, signal: i32) -> Result<(), ProcessLeaseError> {
    if process_group_id <= 1 {
        return Err(ProcessLeaseError::InvalidRecord);
    }
    // SAFETY: callers only pass a positive, child-created group id. Negating
    // it scopes the signal to that group.
    let result = unsafe { libc::kill(-process_group_id, signal) };
    if result == 0 {
        Ok(())
    } else {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(error.into())
        }
    }
}

/// The result of a bounded app-owned process-group exit observation.
///
/// Only [`ProcessGroupTermination::Confirmed`] establishes that no group
/// member can still execute or mutate child-owned artifacts. Callers must
/// treat every other result as unsafe to reuse those artifacts.
#[cfg(unix)]
#[derive(Debug)]
#[must_use = "only Confirmed makes child-owned artifacts safe to reuse"]
pub(crate) enum ProcessGroupTermination {
    Confirmed,
    Unconfirmed,
}

#[cfg(unix)]
impl ProcessGroupTermination {
    pub(crate) const fn is_confirmed(&self) -> bool {
        matches!(self, Self::Confirmed)
    }
}

/// Waits until a group has no live members. Zombies count as exited because
/// they cannot execute and may await an external init process to reap them.
#[cfg(unix)]
pub async fn wait_for_process_group_exit(process_group_id: i32) -> Result<(), ProcessLeaseError> {
    loop {
        if process_group_state(process_group_id)? != GroupState::Live {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Observes an owned process group for a bounded interval.
///
/// A timeout or an inability to prove the group state is deliberately
/// reported as `Unconfirmed`: callers must not reuse files the group could
/// still write until a later confirmed observation or next-start recovery.
#[cfg(unix)]
pub(crate) async fn confirm_process_group_exit(
    process_group_id: i32,
    deadline: Duration,
) -> ProcessGroupTermination {
    match tokio::time::timeout(deadline, wait_for_process_group_exit(process_group_id)).await {
        Ok(Ok(())) => ProcessGroupTermination::Confirmed,
        Ok(Err(_)) | Err(_) => ProcessGroupTermination::Unconfirmed,
    }
}

/// Drop-path fallback for a sidecar whose owning async operation was
/// cancelled. The durable lease is removed only after the whole process group
/// is observed exited; if the runtime is already gone or observation fails,
/// the lease deliberately remains for next-start recovery.
#[cfg(unix)]
pub fn terminate_and_reap_process_group(process_group_id: i32, lease: ChildProcessLease) {
    if signal_process_group(process_group_id, libc::SIGKILL).is_err() {
        return;
    }
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return;
    };
    runtime.spawn(async move {
        if confirm_process_group_exit(process_group_id, Duration::from_secs(2))
            .await
            .is_confirmed()
        {
            let _ = lease.complete();
        }
    });
}

#[cfg(unix)]
#[derive(Debug)]
enum RecoveryAction {
    Remove {
        file_name: String,
    },
    Terminate {
        file_name: String,
        record: ProcessLeaseRecord,
    },
}

#[cfg(unix)]
fn classify_recovery(
    file_name: String,
    record: ProcessLeaseRecord,
) -> Result<RecoveryAction, ProcessLeaseError> {
    match query_process(record.leader_pid)? {
        Some(observed) => {
            if !record_matches(&record, &observed.identity) {
                return Err(ProcessLeaseError::IdentityMismatch);
            }
            if observed.zombie && process_group_state(record.process_group_id)? != GroupState::Live
            {
                Ok(RecoveryAction::Remove { file_name })
            } else {
                Ok(RecoveryAction::Terminate { file_name, record })
            }
        }
        None => match process_group_state(record.process_group_id)? {
            GroupState::Gone | GroupState::ZombiesOnly => Ok(RecoveryAction::Remove { file_name }),
            GroupState::Live => Err(ProcessLeaseError::UnknownProcessGroup),
        },
    }
}

#[cfg(unix)]
fn record_matches(record: &ProcessLeaseRecord, identity: &ProcessIdentity) -> bool {
    record.process_group_id == identity.process_group_id
        && record.start == identity.start
        && record.executable == identity.executable
}

#[cfg(unix)]
fn require_expected_executable(
    expected: &Path,
    observed: &ExecutableIdentity,
) -> Result<(), ProcessLeaseError> {
    let metadata = fs::metadata(expected)?;
    if !metadata.is_file() || metadata.dev() != observed.device || metadata.ino() != observed.inode
    {
        return Err(ProcessLeaseError::IdentityMismatch);
    }
    Ok(())
}

#[cfg(unix)]
fn write_record_atomically(
    directory: &Path,
    file_name: &str,
    record: &ProcessLeaseRecord,
) -> Result<(), ProcessLeaseError> {
    let bytes = serde_json::to_vec(record).map_err(|_| ProcessLeaseError::InvalidRecord)?;
    if bytes.len() as u64 > MAX_LEASE_BYTES {
        return Err(ProcessLeaseError::LimitExceeded);
    }
    let directory_fd = open_lease_directory(directory)?;
    ensure_entry_budget(&directory_fd)?;
    let temporary_name = format!(".{file_name}.tmp");
    let descriptor = openat(
        &directory_fd,
        temporary_name.as_str(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    )
    .map_err(errno_io)?;

    let result = (|| -> Result<(), ProcessLeaseError> {
        let mut file = fs::File::from(descriptor);
        file.write_all(&bytes)?;
        file.sync_all()?;
        let metadata = fstat(&file).map_err(errno_io)?;
        require_private_regular_file(&metadata)?;
        drop(file);
        renameat(
            &directory_fd,
            temporary_name.as_str(),
            &directory_fd,
            file_name,
        )
        .map_err(errno_io)?;
        fsync(&directory_fd).map_err(errno_io)?;
        Ok(())
    })();

    if result.is_err() {
        let _ = unlinkat(&directory_fd, temporary_name.as_str(), AtFlags::empty());
        let _ = fsync(&directory_fd);
    }
    result
}

#[cfg(unix)]
fn read_all_records(
    directory: &Path,
) -> Result<Vec<(String, ProcessLeaseRecord)>, ProcessLeaseError> {
    let directory_fd = open_lease_directory(directory)?;
    let scan_fd = open_lease_directory(directory)?;
    let mut entries = Dir::read_from(scan_fd).map_err(errno_io)?;
    let mut records = Vec::new();
    for entry in &mut entries {
        let entry = entry.map_err(errno_io)?;
        let name_bytes = entry.file_name().to_bytes();
        if name_bytes == b"." || name_bytes == b".." {
            continue;
        }
        if records.len() >= MAX_LEASE_ENTRIES {
            return Err(ProcessLeaseError::LimitExceeded);
        }
        let name = std::str::from_utf8(name_bytes)
            .map_err(|_| ProcessLeaseError::UnsafeFilesystemEntry)?;
        let nonce = nonce_from_file_name(name).ok_or(ProcessLeaseError::UnsafeFilesystemEntry)?;
        let descriptor = openat(
            &directory_fd,
            OsStr::from_bytes(name_bytes),
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(safe_open_error)?;
        let record = read_record(descriptor)?;
        validate_record(&record, nonce)?;
        records.push((name.to_owned(), record));
    }
    records.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(records)
}

#[cfg(unix)]
fn read_record(descriptor: OwnedFd) -> Result<ProcessLeaseRecord, ProcessLeaseError> {
    let before = fstat(&descriptor).map_err(errno_io)?;
    require_private_regular_file(&before)?;
    if before.st_size < 0 || before.st_size as u64 > MAX_LEASE_BYTES {
        return Err(ProcessLeaseError::LimitExceeded);
    }
    let advertised_size = before.st_size as usize;
    let mut file = fs::File::from(descriptor);
    let mut bytes = Vec::with_capacity(advertised_size);
    (&mut file)
        .take(MAX_LEASE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    let after = fstat(&file).map_err(errno_io)?;
    if bytes.len() > MAX_LEASE_BYTES as usize
        || bytes.len() != advertised_size
        || after.st_size != before.st_size
        || after.st_ino != before.st_ino
        || after.st_dev != before.st_dev
    {
        return Err(ProcessLeaseError::UnsafeFilesystemEntry);
    }
    serde_json::from_slice(&bytes).map_err(|_| ProcessLeaseError::InvalidRecord)
}

#[cfg(unix)]
fn validate_record(record: &ProcessLeaseRecord, file_nonce: &str) -> Result<(), ProcessLeaseError> {
    let expected_source = if cfg!(target_os = "linux") {
        "linux_proc_stat_v1"
    } else if cfg!(target_os = "macos") {
        "macos_proc_bsdinfo_v1"
    } else {
        return Err(ProcessLeaseError::UnsupportedPlatform);
    };
    if record.schema != LEASE_SCHEMA
        || record.nonce != file_nonce
        || !is_nonce(file_nonce)
        || record.leader_pid <= 1
        || record.process_group_id != record.leader_pid
        || record.start.source != expected_source
        || record.executable.device == 0
        || record.executable.inode == 0
        || !is_sha256(&record.executable.path_sha256)
    {
        return Err(ProcessLeaseError::InvalidRecord);
    }
    Ok(())
}

#[cfg(unix)]
fn remove_record(directory: &Path, file_name: &str) -> Result<(), ProcessLeaseError> {
    if nonce_from_file_name(file_name).is_none() {
        return Err(ProcessLeaseError::InvalidRecord);
    }
    let directory_fd = open_lease_directory(directory)?;
    unlinkat(&directory_fd, file_name, AtFlags::empty()).map_err(errno_io)?;
    fsync(&directory_fd).map_err(errno_io)?;
    Ok(())
}

#[cfg(unix)]
fn ensure_entry_budget(directory: &OwnedFd) -> Result<(), ProcessLeaseError> {
    let mut entries = Dir::read_from(directory).map_err(errno_io)?;
    let mut count = 0_usize;
    for entry in &mut entries {
        let entry = entry.map_err(errno_io)?;
        let name = entry.file_name().to_bytes();
        if name == b"." || name == b".." {
            continue;
        }
        count += 1;
        if count >= MAX_LEASE_ENTRIES {
            return Err(ProcessLeaseError::LimitExceeded);
        }
    }
    Ok(())
}

#[cfg(unix)]
fn require_private_regular_file(metadata: &rustix::fs::Stat) -> Result<(), ProcessLeaseError> {
    if FileType::from_raw_mode(metadata.st_mode) != FileType::RegularFile
        || metadata.st_uid != effective_uid()
        || metadata.st_nlink != 1
        || metadata.st_mode & 0o077 != 0
    {
        return Err(ProcessLeaseError::UnsafeFilesystemEntry);
    }
    Ok(())
}

#[cfg(unix)]
fn open_lease_directory(directory: &Path) -> Result<OwnedFd, ProcessLeaseError> {
    open(
        directory,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(safe_open_error)
}

#[cfg(unix)]
fn safe_open_error(error: Errno) -> ProcessLeaseError {
    match error {
        Errno::LOOP | Errno::NOTDIR => ProcessLeaseError::UnsafeFilesystemEntry,
        _ => errno_io(error),
    }
}

#[cfg(unix)]
fn errno_io(error: Errno) -> ProcessLeaseError {
    ProcessLeaseError::Io(io::Error::from(error))
}

#[cfg(unix)]
fn effective_uid() -> u32 {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() }
}

#[cfg(unix)]
fn lease_file_name(nonce: &str) -> String {
    format!("lease-{nonce}.json")
}

#[cfg(unix)]
fn nonce_from_file_name(name: &str) -> Option<&str> {
    name.strip_prefix("lease-")
        .and_then(|value| value.strip_suffix(".json"))
        .filter(|value| is_nonce(value))
}

#[cfg(unix)]
fn is_nonce(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(unix)]
fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GroupState {
    Gone,
    ZombiesOnly,
    Live,
}

#[cfg(unix)]
fn wait_for_group_exit_blocking(
    process_group_id: i32,
    grace: Duration,
) -> Result<bool, ProcessLeaseError> {
    let deadline = Instant::now() + grace;
    loop {
        if process_group_state(process_group_id)? != GroupState::Live {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

#[cfg(unix)]
fn process_group_state(process_group_id: i32) -> Result<GroupState, ProcessLeaseError> {
    if process_group_id <= 1 {
        return Err(ProcessLeaseError::InvalidRecord);
    }
    // Kernel group visibility and platform process enumeration can briefly
    // disagree while a leader exits or becomes a zombie. Retry only that
    // explicit unknown state; a persistent unverifiable group still fails
    // closed and is never signalled.
    for attempt in 0..4 {
        if !kernel_process_group_exists(process_group_id)? {
            return Ok(GroupState::Gone);
        }
        let state = platform_process_group_state(process_group_id);
        match state {
            Err(ProcessLeaseError::UnknownProcessGroup) if attempt < 3 => {
                std::thread::sleep(POLL_INTERVAL);
            }
            result => return result,
        }
    }
    unreachable!("bounded process-group state retry always returns")
}

#[cfg(target_os = "linux")]
fn platform_process_group_state(process_group_id: i32) -> Result<GroupState, ProcessLeaseError> {
    linux_process_group_state(process_group_id)
}

#[cfg(target_os = "macos")]
fn platform_process_group_state(process_group_id: i32) -> Result<GroupState, ProcessLeaseError> {
    macos_process_group_state(process_group_id)
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn platform_process_group_state(_process_group_id: i32) -> Result<GroupState, ProcessLeaseError> {
    Err(ProcessLeaseError::UnsupportedPlatform)
}

#[cfg(unix)]
fn kernel_process_group_exists(process_group_id: i32) -> Result<bool, ProcessLeaseError> {
    // SAFETY: signal 0 does not mutate the target group.
    let result = unsafe { libc::kill(-process_group_id, 0) };
    if result == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ESRCH) => Ok(false),
        Some(libc::EPERM) => Ok(true),
        _ => Err(error.into()),
    }
}

#[cfg(target_os = "linux")]
fn linux_process_group_state(process_group_id: i32) -> Result<GroupState, ProcessLeaseError> {
    let mut members = 0_usize;
    let mut live = false;
    let mut scanned = 0_usize;
    for entry in fs::read_dir("/proc")? {
        let entry = entry?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<i32>().ok())
        else {
            continue;
        };
        scanned += 1;
        if scanned > 65_536 {
            return Err(ProcessLeaseError::LimitExceeded);
        }
        if let Some(basic) = linux_basic_process(pid)? {
            if basic.process_group_id == process_group_id {
                members += 1;
                if members > MAX_GROUP_MEMBERS {
                    return Err(ProcessLeaseError::LimitExceeded);
                }
                live |= basic.state != 'Z';
            }
        }
    }
    match (members, live) {
        (_, true) => Ok(GroupState::Live),
        (0, false) if !kernel_process_group_exists(process_group_id)? => Ok(GroupState::Gone),
        (0, false) => Err(ProcessLeaseError::UnknownProcessGroup),
        (_, false) => Ok(GroupState::ZombiesOnly),
    }
}

#[cfg(target_os = "macos")]
fn macos_process_group_state(process_group_id: i32) -> Result<GroupState, ProcessLeaseError> {
    let mut pids = vec![0 as libc::pid_t; MAX_GROUP_MEMBERS];
    // SAFETY: the buffer is writable for exactly the byte length passed.
    let bytes = unsafe {
        libc::proc_listpgrppids(
            process_group_id,
            pids.as_mut_ptr().cast(),
            (pids.len() * std::mem::size_of::<libc::pid_t>()) as i32,
        )
    };
    if bytes < 0 {
        return Err(io::Error::last_os_error().into());
    }
    // Unlike proc_listpids, proc_listpgrppids returns a PID count.
    if bytes as usize >= pids.len() {
        return Err(ProcessLeaseError::LimitExceeded);
    }
    let count = bytes as usize;
    if count == 0 {
        return Err(ProcessLeaseError::UnknownProcessGroup);
    }
    let mut members = 0_usize;
    let mut live = false;
    for pid in pids.into_iter().take(count).filter(|pid| *pid > 0) {
        if let Some(info) = macos_basic_process(pid)? {
            if info.process_group_id == process_group_id {
                members += 1;
                live |= !info.zombie;
            }
        }
    }
    match (members, live) {
        (_, true) => Ok(GroupState::Live),
        (0, false) if !kernel_process_group_exists(process_group_id)? => Ok(GroupState::Gone),
        (0, false) => Err(ProcessLeaseError::UnknownProcessGroup),
        (_, false) => Ok(GroupState::ZombiesOnly),
    }
}

#[cfg(target_os = "linux")]
struct LinuxBasicProcess {
    process_group_id: i32,
    start_ticks: u64,
    state: char,
}

#[cfg(target_os = "linux")]
fn linux_basic_process(pid: i32) -> Result<Option<LinuxBasicProcess>, ProcessLeaseError> {
    let path = PathBuf::from(format!("/proc/{pid}/stat"));
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut text = String::new();
    (&mut file).take(16 * 1024).read_to_string(&mut text)?;
    if text.len() >= 16 * 1024 {
        return Err(ProcessLeaseError::InvalidRecord);
    }
    let (_, fields) = text
        .rsplit_once(") ")
        .ok_or(ProcessLeaseError::InvalidRecord)?;
    let fields = fields.split_whitespace().collect::<Vec<_>>();
    let state = fields
        .first()
        .and_then(|value| value.chars().next())
        .ok_or(ProcessLeaseError::InvalidRecord)?;
    let process_group_id = fields
        .get(2)
        .ok_or(ProcessLeaseError::InvalidRecord)?
        .parse()
        .map_err(|_| ProcessLeaseError::InvalidRecord)?;
    let start_ticks = fields
        .get(19)
        .ok_or(ProcessLeaseError::InvalidRecord)?
        .parse()
        .map_err(|_| ProcessLeaseError::InvalidRecord)?;
    Ok(Some(LinuxBasicProcess {
        process_group_id,
        start_ticks,
        state,
    }))
}

#[cfg(target_os = "linux")]
fn query_process(pid: i32) -> Result<Option<ObservedProcess>, ProcessLeaseError> {
    let Some(before) = linux_basic_process(pid)? else {
        return Ok(None);
    };
    let executable_path = match fs::read_link(format!("/proc/{pid}/exe")) {
        Ok(path) => path,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let metadata = match fs::metadata(format!("/proc/{pid}/exe")) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let Some(after) = linux_basic_process(pid)? else {
        return Ok(None);
    };
    if before.process_group_id != after.process_group_id || before.start_ticks != after.start_ticks
    {
        return Err(ProcessLeaseError::IdentityMismatch);
    }
    Ok(Some(ObservedProcess {
        identity: ProcessIdentity {
            process_group_id: before.process_group_id,
            start: StartIdentity {
                source: "linux_proc_stat_v1".into(),
                primary: before.start_ticks,
                secondary: 0,
            },
            executable: ExecutableIdentity {
                device: metadata.dev(),
                inode: metadata.ino(),
                path_sha256: hash_path(&executable_path),
            },
        },
        zombie: before.state == 'Z',
    }))
}

#[cfg(target_os = "macos")]
struct MacosBasicProcess {
    process_group_id: i32,
    start_seconds: u64,
    start_microseconds: u64,
    zombie: bool,
}

#[cfg(target_os = "macos")]
fn macos_basic_process(pid: i32) -> Result<Option<MacosBasicProcess>, ProcessLeaseError> {
    // SAFETY: proc_bsdinfo is plain data and proc_pidinfo receives its exact
    // writable size.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let expected = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    let bytes = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            expected,
        )
    };
    if bytes == 0 {
        // proc_pidinfo is allowed to return zero without preserving errno.
        // SAFETY: signal 0 only checks existence and permission.
        let exists = unsafe { libc::kill(pid, 0) };
        if exists != 0 && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            return Ok(None);
        }
        return Err(ProcessLeaseError::UnknownProcessGroup);
    }
    if bytes != expected || info.pbi_pid != pid as u32 {
        return Err(ProcessLeaseError::IdentityMismatch);
    }
    Ok(Some(MacosBasicProcess {
        process_group_id: info.pbi_pgid as i32,
        start_seconds: info.pbi_start_tvsec,
        start_microseconds: info.pbi_start_tvusec,
        zombie: info.pbi_status == libc::SZOMB,
    }))
}

#[cfg(target_os = "macos")]
fn query_process(pid: i32) -> Result<Option<ObservedProcess>, ProcessLeaseError> {
    let Some(before) = macos_basic_process(pid)? else {
        return Ok(None);
    };
    let mut path_buffer = vec![0_u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: path_buffer is writable for the reported capacity.
    let path_length = unsafe {
        libc::proc_pidpath(
            pid,
            path_buffer.as_mut_ptr().cast(),
            path_buffer.len() as u32,
        )
    };
    if path_length <= 0 {
        if macos_basic_process(pid)?.is_none() {
            return Ok(None);
        }
        return Err(ProcessLeaseError::IdentityMismatch);
    }
    path_buffer.truncate(path_length as usize);
    let executable_path = PathBuf::from(OsStr::from_bytes(&path_buffer));
    let metadata = match fs::metadata(&executable_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(ProcessLeaseError::IdentityMismatch)
        }
        Err(error) => return Err(error.into()),
    };
    let Some(after) = macos_basic_process(pid)? else {
        return Ok(None);
    };
    if before.process_group_id != after.process_group_id
        || before.start_seconds != after.start_seconds
        || before.start_microseconds != after.start_microseconds
    {
        return Err(ProcessLeaseError::IdentityMismatch);
    }
    Ok(Some(ObservedProcess {
        identity: ProcessIdentity {
            process_group_id: before.process_group_id,
            start: StartIdentity {
                source: "macos_proc_bsdinfo_v1".into(),
                primary: before.start_seconds,
                secondary: before.start_microseconds,
            },
            executable: ExecutableIdentity {
                device: metadata.dev(),
                inode: metadata.ino(),
                path_sha256: hash_path(&executable_path),
            },
        },
        zombie: before.zombie,
    }))
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn query_process(_pid: i32) -> Result<Option<ObservedProcess>, ProcessLeaseError> {
    Err(ProcessLeaseError::UnsupportedPlatform)
}

#[cfg(unix)]
fn hash_path(path: &Path) -> String {
    hex::encode(Sha256::digest(path.as_os_str().as_bytes()))
}

#[cfg(test)]
mod tests;
