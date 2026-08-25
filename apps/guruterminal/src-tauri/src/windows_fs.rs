#![cfg(windows)]

use std::{
    ffi::c_void,
    fs::{File, Metadata, OpenOptions},
    io,
    os::windows::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    },
    path::{Component, Path, PathBuf},
};

use crate::domain::RootFilesystemIdentity;

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const FILE_READ_ATTRIBUTES: u32 = 0x0000_0080;
const FILE_SHARE_READ: u32 = 0x0000_0001;
const FILE_SHARE_WRITE: u32 = 0x0000_0002;
const MOVEFILE_WRITE_THROUGH: u32 = 0x0000_0008;

#[repr(C)]
#[derive(Clone, Copy)]
struct FileTime {
    low_date_time: u32,
    high_date_time: u32,
}

#[repr(C)]
struct ByHandleFileInformation {
    file_attributes: u32,
    creation_time: FileTime,
    last_access_time: FileTime,
    last_write_time: FileTime,
    volume_serial_number: u32,
    file_size_high: u32,
    file_size_low: u32,
    number_of_links: u32,
    file_index_high: u32,
    file_index_low: u32,
}

#[repr(C)]
#[cfg(not(debug_assertions))]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

#[repr(C)]
#[cfg(not(debug_assertions))]
struct WinTrustFileInfo {
    struct_size: u32,
    file_path: *const u16,
    file_handle: *mut c_void,
    known_subject: *const Guid,
}

#[repr(C)]
#[cfg(not(debug_assertions))]
struct WinTrustData {
    struct_size: u32,
    policy_callback_data: *mut c_void,
    sip_client_data: *mut c_void,
    ui_choice: u32,
    revocation_checks: u32,
    union_choice: u32,
    file_info: *mut WinTrustFileInfo,
    state_action: u32,
    state_data: *mut c_void,
    url_reference: *const u16,
    provider_flags: u32,
    ui_context: u32,
    signature_settings: *mut c_void,
}

#[repr(C)]
#[cfg(not(debug_assertions))]
struct CertContext {
    encoding_type: u32,
    encoded: *const u8,
    encoded_len: u32,
    cert_info: *mut c_void,
    cert_store: *mut c_void,
}

#[repr(C)]
#[cfg(not(debug_assertions))]
struct CryptProviderCert {
    struct_size: u32,
    cert: *mut CertContext,
}

#[repr(C)]
#[cfg(not(debug_assertions))]
struct CryptProviderSigner {
    struct_size: u32,
    verify_as_of: FileTime,
    cert_chain_count: u32,
    cert_chain: *mut CryptProviderCert,
}

#[link(name = "kernel32")]
extern "system" {
    fn GetFileInformationByHandle(
        file: *mut c_void,
        information: *mut ByHandleFileInformation,
    ) -> i32;
    fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    fn ReplaceFileW(
        replaced: *const u16,
        replacement: *const u16,
        backup: *const u16,
        flags: u32,
        exclude: *mut c_void,
        reserved: *mut c_void,
    ) -> i32;
    #[cfg(not(debug_assertions))]
    fn GetModuleHandleW(module_name: *const u16) -> *mut c_void;
    #[cfg(not(debug_assertions))]
    fn GetProcAddress(module: *mut c_void, procedure_name: *const u8) -> *mut c_void;
}

#[link(name = "wintrust")]
#[cfg(not(debug_assertions))]
extern "system" {
    fn WinVerifyTrust(window: *mut c_void, action: *const Guid, data: *mut c_void) -> i32;
}

pub(crate) fn metadata_is_reparse(metadata: &Metadata) -> bool {
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

pub(crate) fn ensure_no_reparse_points(path: &Path) -> io::Result<()> {
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path must be absolute",
        ));
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                current.push(component.as_os_str());
            }
            Component::CurDir | Component::ParentDir => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "path contains a non-ordinary component",
                ));
            }
        }
        // A Windows prefix alone is not an openable filesystem entry.
        if matches!(component, Component::Prefix(_)) {
            continue;
        }
        let metadata = std::fs::symlink_metadata(&current)?;
        if metadata.file_type().is_symlink() || metadata_is_reparse(&metadata) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "path contains a reparse point",
            ));
        }
    }
    Ok(())
}

pub(crate) fn open_directory_no_reparse(path: &Path) -> io::Result<File> {
    ensure_no_reparse_points(path)?;
    let mut options = OpenOptions::new();
    options
        .access_mode(FILE_READ_ATTRIBUTES)
        // Deliberately omit FILE_SHARE_DELETE. A retained Guru root handle then
        // prevents rename/delete/rebind for its lifetime on Windows.
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_dir() || metadata_is_reparse(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "path is not a non-reparse directory",
        ));
    }
    Ok(file)
}

pub(crate) fn add_open_reparse_point_flag(options: &mut OpenOptions) {
    options
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
}

/// Configures a short-lived mutable state-file handle. Read/write sharing lets
/// SQLite retain its own read/write handle while the app revalidates a path.
/// Deliberately omit `FILE_SHARE_DELETE`: the validation handle still blocks
/// rename, deletion, and path rebinding for its lifetime.
pub(crate) fn add_open_reparse_point_flag_with_read_write_share(options: &mut OpenOptions) {
    options
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
}

pub(crate) fn open_regular_no_reparse(path: &Path) -> io::Result<File> {
    ensure_no_reparse_points(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    add_open_reparse_point_flag(&mut options);
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata_is_reparse(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "path is not a non-reparse regular file",
        ));
    }
    Ok(file)
}

/// Reopens a regular file only long enough to compare its filesystem identity
/// with an already-trusted handle.
pub(crate) fn reopen_regular_no_reparse_for_identity(path: &Path) -> io::Result<File> {
    ensure_no_reparse_points(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    add_open_reparse_point_flag_with_read_write_share(&mut options);
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata_is_reparse(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "path is not a non-reparse regular file",
        ));
    }
    Ok(file)
}

pub(crate) fn open_parent_directories_no_reparse(path: &Path) -> io::Result<Vec<File>> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "file path has no parent"))?;
    let mut ancestors = parent
        .ancestors()
        .filter(|ancestor| ancestor.is_absolute())
        .collect::<Vec<_>>();
    ancestors.reverse();
    ancestors
        .into_iter()
        .map(open_directory_no_reparse)
        .collect()
}

pub(crate) fn filesystem_identity(file: &File) -> io::Result<RootFilesystemIdentity> {
    let mut information = std::mem::MaybeUninit::<ByHandleFileInformation>::uninit();
    // SAFETY: `file` owns a valid Windows handle and the output pointer refers
    // to writable storage of the exact structure required by kernel32.
    let succeeded = unsafe {
        GetFileInformationByHandle(file.as_raw_handle().cast(), information.as_mut_ptr())
    };
    if succeeded == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a successful call initialized the complete structure.
    let information = unsafe { information.assume_init() };
    Ok(RootFilesystemIdentity {
        // On Windows these cross-platform fields carry the volume serial and
        // the 64-bit file index respectively.
        device: information.volume_serial_number as u64,
        inode: ((information.file_index_high as u64) << 32) | information.file_index_low as u64,
    })
}

pub(crate) fn move_file_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
    let source = wide_path(source);
    let destination = wide_path(destination);
    // SAFETY: both buffers are NUL-terminated and live for the duration of the call.
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub(crate) fn replace_file_with_backup(
    target_path: &Path,
    replacement_path: &Path,
    backup_path: &Path,
) -> io::Result<()> {
    let target = wide_path(target_path);
    let replacement = wide_path(replacement_path);
    let backup = wide_path(backup_path);
    // SAFETY: all buffers are NUL-terminated and optional pointer arguments
    // are intentionally null as required by ReplaceFileW.
    if unsafe {
        ReplaceFileW(
            target.as_ptr(),
            replacement.as_ptr(),
            backup.as_ptr(),
            // REPLACEFILE_WRITE_THROUGH is explicitly unsupported by Windows.
            // Flush both resulting files below while retaining the backup as
            // the crash-recovery journal until the caller validates it.
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } == 0
    {
        Err(io::Error::last_os_error())
    } else {
        flush_regular_file(target_path)?;
        flush_regular_file(backup_path)?;
        Ok(())
    }
}

fn flush_regular_file(path: &Path) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    add_open_reparse_point_flag(&mut options);
    options.open(path)?.sync_all()
}

/// Uses the native Authenticode policy provider. Callers intentionally enable
/// this only in release builds so unpackaged development sidecars remain usable.
#[cfg(not(debug_assertions))]
pub(crate) fn authenticode_signer_certificate(path: &Path, opened: &File) -> Option<Vec<u8>> {
    const ACTION_GENERIC_VERIFY_V2: Guid = Guid {
        data1: 0x00aa_c56b,
        data2: 0xcd44,
        data3: 0x11d0,
        data4: [0x8c, 0xc2, 0x00, 0xc0, 0x4f, 0xc2, 0x95, 0xee],
    };
    const WTD_UI_NONE: u32 = 2;
    const WTD_REVOKE_NONE: u32 = 0;
    const WTD_CHOICE_FILE: u32 = 1;
    const WTD_STATEACTION_VERIFY: u32 = 1;
    const WTD_STATEACTION_CLOSE: u32 = 2;
    const WTD_SAFER_FLAG: u32 = 0x0000_0100;
    const WTD_CACHE_ONLY_URL_RETRIEVAL: u32 = 0x0000_1000;

    let path = wide_path(path);
    let mut file_info = WinTrustFileInfo {
        struct_size: std::mem::size_of::<WinTrustFileInfo>() as u32,
        file_path: path.as_ptr(),
        file_handle: opened.as_raw_handle().cast(),
        known_subject: std::ptr::null(),
    };
    let mut trust_data = WinTrustData {
        struct_size: std::mem::size_of::<WinTrustData>() as u32,
        policy_callback_data: std::ptr::null_mut(),
        sip_client_data: std::ptr::null_mut(),
        ui_choice: WTD_UI_NONE,
        revocation_checks: WTD_REVOKE_NONE,
        union_choice: WTD_CHOICE_FILE,
        file_info: &mut file_info,
        state_action: WTD_STATEACTION_VERIFY,
        state_data: std::ptr::null_mut(),
        url_reference: std::ptr::null(),
        provider_flags: WTD_SAFER_FLAG | WTD_CACHE_ONLY_URL_RETRIEVAL,
        ui_context: 0,
        signature_settings: std::ptr::null_mut(),
    };
    // SAFETY: both structures follow the WinTrust ABI and retain their backing
    // path buffer until the policy provider state is explicitly closed.
    let status = unsafe {
        WinVerifyTrust(
            std::ptr::null_mut(),
            &ACTION_GENERIC_VERIFY_V2,
            (&mut trust_data as *mut WinTrustData).cast(),
        )
    };
    let signer_certificate = if status == 0 {
        // SAFETY: the provider owns these structures until CLOSE below. We only
        // copy the bounded leaf certificate DER while that state is live.
        unsafe { signer_certificate_from_state(trust_data.state_data) }
    } else {
        None
    };
    trust_data.state_action = WTD_STATEACTION_CLOSE;
    // SAFETY: closes only state returned in `trust_data` by the preceding call.
    unsafe {
        WinVerifyTrust(
            std::ptr::null_mut(),
            &ACTION_GENERIC_VERIFY_V2,
            (&mut trust_data as *mut WinTrustData).cast(),
        );
    }
    signer_certificate
}

#[cfg(not(debug_assertions))]
unsafe fn signer_certificate_from_state(state_data: *mut c_void) -> Option<Vec<u8>> {
    type ProviderFromState = unsafe extern "system" fn(*mut c_void) -> *mut c_void;
    type SignerFromChain =
        unsafe extern "system" fn(*mut c_void, u32, i32, u32) -> *mut CryptProviderSigner;

    let module_name = "wintrust.dll\0".encode_utf16().collect::<Vec<_>>();
    // SAFETY: WinVerifyTrust is statically imported, so its module is loaded;
    // the names below are fixed NUL-terminated ASCII exports from that module.
    let module = unsafe { GetModuleHandleW(module_name.as_ptr()) };
    if module.is_null() {
        return None;
    }
    let provider_proc =
        unsafe { GetProcAddress(module, b"WTHelperProvDataFromStateData\0".as_ptr()) };
    let signer_proc =
        unsafe { GetProcAddress(module, b"WTHelperGetProvSignerFromChain\0".as_ptr()) };
    if provider_proc.is_null() || signer_proc.is_null() {
        return None;
    }
    // SAFETY: the resolved symbols have the signatures documented by wintrust.h.
    let provider_from_state: ProviderFromState = unsafe { std::mem::transmute(provider_proc) };
    let signer_from_chain: SignerFromChain = unsafe { std::mem::transmute(signer_proc) };
    let provider = unsafe { provider_from_state(state_data) };
    if provider.is_null() {
        return None;
    }
    let signer = unsafe { signer_from_chain(provider, 0, 0, 0) };
    if signer.is_null()
        || unsafe { (*signer).cert_chain_count == 0 }
        || unsafe { (*signer).cert_chain.is_null() }
        || unsafe { (*(*signer).cert_chain).cert.is_null() }
    {
        return None;
    }
    let certificate = unsafe { &*(*(*signer).cert_chain).cert };
    let length = certificate.encoded_len as usize;
    if certificate.encoded.is_null() || length == 0 || length > 1024 * 1024 {
        None
    } else {
        // SAFETY: the provider owns this DER buffer until WTD_STATEACTION_CLOSE;
        // the caller invokes us before that close and we copy it immediately.
        Some(unsafe { std::slice::from_raw_parts(certificate.encoded, length) }.to_vec())
    }
}

fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_through_move_never_clobbers_an_existing_target() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("source");
        let target = temporary.path().join("target");
        std::fs::write(&source, b"new").unwrap();
        std::fs::write(&target, b"old").unwrap();

        assert!(move_file_no_replace(&source, &target).is_err());
        assert_eq!(std::fs::read(&source).unwrap(), b"new");
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
    }

    #[test]
    fn held_directory_handle_blocks_path_rename() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("guru");
        let moved = temporary.path().join("guru-moved");
        std::fs::create_dir(&root).unwrap();
        let handle = open_directory_no_reparse(&root).unwrap();
        let identity = filesystem_identity(&handle).unwrap();
        assert!(std::fs::rename(&root, &moved).is_err());
        assert_eq!(filesystem_identity(&handle).unwrap(), identity);
        assert_eq!(
            filesystem_identity(&open_directory_no_reparse(&root).unwrap()).unwrap(),
            identity
        );
    }

    #[test]
    fn directory_symlink_reparse_points_fail_closed() {
        use std::os::windows::fs::symlink_dir;

        let temporary = tempfile::tempdir().unwrap();
        let outside = temporary.path().join("outside");
        let link = temporary.path().join("linked");
        std::fs::create_dir(&outside).unwrap();
        if let Err(error) = symlink_dir(&outside, &link) {
            if error.kind() == io::ErrorKind::PermissionDenied {
                return;
            }
            panic!("failed to create test reparse point: {error}");
        }
        assert!(ensure_no_reparse_points(&link).is_err());
        assert!(open_directory_no_reparse(&link).is_err());
    }

    #[test]
    fn identity_reopen_allows_a_restrictive_writer_without_weakening_it() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("state");
        std::fs::write(&path, b"state").unwrap();

        let mut options = OpenOptions::new();
        options.read(true).write(true);
        add_open_reparse_point_flag(&mut options);
        let retained = options.open(&path).unwrap();

        let reopened = reopen_regular_no_reparse_for_identity(&path).unwrap();
        assert_eq!(
            filesystem_identity(&retained).unwrap(),
            filesystem_identity(&reopened).unwrap()
        );

        let error = OpenOptions::new().write(true).open(&path).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(32));
    }

    #[test]
    fn identity_reopen_rejects_reparse_points() {
        use std::os::windows::fs::symlink_file;

        let temporary = tempfile::tempdir().unwrap();
        let target = temporary.path().join("target");
        let link = temporary.path().join("link");
        std::fs::write(&target, b"state").unwrap();
        if let Err(error) = symlink_file(&target, &link) {
            if error.kind() == io::ErrorKind::PermissionDenied {
                return;
            }
            panic!("failed to create test reparse point: {error}");
        }

        assert!(ensure_no_reparse_points(&link).is_err());
        assert!(reopen_regular_no_reparse_for_identity(&link).is_err());
    }
}
