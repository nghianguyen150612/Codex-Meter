use super::*;
use std::fs::File;
use std::io::{Read, Write};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, IntoRawHandle, OwnedHandle, RawHandle};
use std::path::{Component, Prefix};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{
    GetLastError, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_LOCK_VIOLATION,
    ERROR_PATH_NOT_FOUND, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Foundation::{LocalFree, GENERIC_ALL, GENERIC_WRITE};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
    ConvertStringSidToSidW, GetSecurityInfo, SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    EqualSid, GetAce, GetAclInformation, GetLengthSid, GetSecurityDescriptorControl,
    GetSecurityDescriptorDacl, GetTokenInformation, TokenUser, ACCESS_ALLOWED_ACE, ACE_HEADER, ACL,
    ACL_SIZE_INFORMATION, DACL_SECURITY_INFORMATION, INHERIT_ONLY_ACE, OWNER_SECURITY_INFORMATION,
    SECURITY_ATTRIBUTES, SE_DACL_PRESENT, SE_DACL_PROTECTED, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateDirectoryW, CreateFileW, DeleteFileW, FileAttributeTagInfo, FileStandardInfo,
    FlushFileBuffers, GetDriveTypeW, GetFileInformationByHandleEx, GetFileSizeEx, GetFileType,
    LockFileEx, MoveFileExW, ReplaceFileW, SetEndOfFile, SetFilePointerEx, UnlockFileEx,
    FILE_ALL_ACCESS, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_ATTRIBUTE_TAG_INFO, FILE_BEGIN, FILE_DELETE_CHILD, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_READ_DATA, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_STANDARD_INFO, FILE_TYPE_DISK, FILE_WRITE_ATTRIBUTES,
    FILE_WRITE_DATA, FILE_WRITE_EA, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
    MOVEFILE_WRITE_THROUGH, OPEN_ALWAYS, OPEN_EXISTING,
};
use windows_sys::Win32::System::SystemServices::{ACCESS_ALLOWED_ACE_TYPE, ACCESS_DENIED_ACE_TYPE};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows_sys::Win32::System::IO::OVERLAPPED;

const LOCK_TIMEOUT: Duration = Duration::from_secs(5);
const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(10);
const MAX_PATH_UNITS: usize = 32_767;
const PRIVATE_SECURITY_DESCRIPTOR_REVISION: u32 = 1;
const LOCK_FILE_LENGTH_LOW: u32 = 1;
const DRIVE_FIXED_TYPE: u32 = 3;
static JOURNAL_THREAD_LOCK: Mutex<()> = Mutex::new(());

pub struct Store {
    directory: PathBuf,
    _directory_handle: OwnedHandle,
    #[cfg(test)]
    remove_directory_on_drop: bool,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, RecoveryJournalError> {
        let security = SecurityContext::current()?;
        let directory_handle = ensure_private_directory(path, &security)?;
        Ok(Self {
            directory: path.to_path_buf(),
            _directory_handle: directory_handle,
            #[cfg(test)]
            remove_directory_on_drop: false,
        })
    }

    #[cfg(test)]
    pub fn open_for_tests() -> Result<Self, RecoveryJournalError> {
        let local_data = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .ok_or(RecoveryJournalError::DirectoryUnavailable)?;
        let path = local_data.join(format!(
            "codex-meter-windows-unit-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let mut store = Self::open(&path)?;
        store.remove_directory_on_drop = true;
        Ok(store)
    }

    pub fn with_lock<T>(
        &self,
        action: impl FnOnce() -> Result<T, RecoveryJournalError>,
    ) -> Result<T, RecoveryJournalError> {
        if persistence_failure(PersistenceFailurePoint::LockAcquisition) {
            return Err(RecoveryJournalError::Io);
        }
        let _thread_guard = JOURNAL_THREAD_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let security = SecurityContext::current()?;
        let path = self.directory.join(".lock");
        let handle = open_secure_file(
            &path,
            FILE_READ_DATA
                | FILE_READ_ATTRIBUTES
                | FILE_WRITE_DATA
                | windows_sys::Win32::Storage::FileSystem::READ_CONTROL,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            OPEN_ALWAYS,
            &security,
        )?;
        let _lock = acquire_lock(handle.as_raw_handle().cast())?;
        action()
    }

    pub fn read(&self) -> Result<JournalData, RecoveryJournalError> {
        if persistence_failure(PersistenceFailurePoint::Recovery) {
            return Err(RecoveryJournalError::Io);
        }
        let security = SecurityContext::current()?;
        let path = self.directory.join("journal.json");
        let handle = match open_existing_file(
            &path,
            FILE_READ_DATA
                | FILE_READ_ATTRIBUTES
                | windows_sys::Win32::Storage::FileSystem::READ_CONTROL,
            &security,
        ) {
            Ok(Some(handle)) => handle,
            Ok(None) => return Ok(JournalData::default()),
            Err(error) => return Err(error),
        };
        let mut size = 0_i64;
        if unsafe { GetFileSizeEx(handle.as_raw_handle().cast(), &mut size) } == 0 || size < 0 {
            return Err(RecoveryJournalError::Io);
        }
        if size as usize > MAX_JOURNAL_BYTES {
            return Err(RecoveryJournalError::CorruptJournal);
        }
        let file = file_from_handle(handle);
        let mut bytes = Vec::with_capacity(size as usize);
        file.take((MAX_JOURNAL_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| RecoveryJournalError::Io)?;
        if bytes.len() > MAX_JOURNAL_BYTES {
            return Err(RecoveryJournalError::CorruptJournal);
        }
        decode(&bytes)
    }

    pub fn write(&self, data: &mut JournalData) -> Result<(), RecoveryJournalError> {
        data.generation = data.generation.saturating_add(1);
        let bytes = encode(data)?;
        if persistence_failure(PersistenceFailurePoint::BeforeWrite) {
            return Err(RecoveryJournalError::Io);
        }
        let security = SecurityContext::current()?;
        let temporary_path = self.directory.join(".journal.tmp");
        let target_path = self.directory.join("journal.json");
        let handle = open_secure_file(
            &temporary_path,
            FILE_READ_ATTRIBUTES
                | FILE_READ_DATA
                | FILE_WRITE_DATA
                | windows_sys::Win32::Storage::FileSystem::READ_CONTROL,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            OPEN_ALWAYS,
            &security,
        )?;
        if unsafe {
            SetFilePointerEx(
                handle.as_raw_handle().cast(),
                0,
                std::ptr::null_mut(),
                FILE_BEGIN,
            )
        } == 0
            || unsafe { SetEndOfFile(handle.as_raw_handle().cast()) } == 0
        {
            return Err(RecoveryJournalError::Io);
        }
        let mut file = file_from_handle(handle);
        let write_result = if persistence_failure(PersistenceFailurePoint::DuringWrite) {
            file.write_all(&bytes[..bytes.len() / 2])
                .and_then(|()| Err(std::io::Error::other("injected journal write failure")))
        } else {
            file.write_all(&bytes)
        };
        if write_result.is_err() {
            drop(file);
            cleanup_temporary(&temporary_path, &security);
            return Err(RecoveryJournalError::Io);
        }
        if persistence_failure(PersistenceFailurePoint::BeforeFlush) {
            drop(file);
            cleanup_temporary(&temporary_path, &security);
            return Err(RecoveryJournalError::Io);
        }
        if file.flush().is_err() {
            drop(file);
            cleanup_temporary(&temporary_path, &security);
            return Err(RecoveryJournalError::Io);
        }
        if unsafe { FlushFileBuffers(file.as_raw_handle().cast()) } == 0 {
            drop(file);
            cleanup_temporary(&temporary_path, &security);
            return Err(RecoveryJournalError::Io);
        }
        drop(file);
        if persistence_failure(PersistenceFailurePoint::AfterFlush)
            || persistence_failure(PersistenceFailurePoint::BeforeReplace)
        {
            cleanup_temporary(&temporary_path, &security);
            return Err(RecoveryJournalError::Io);
        }

        match open_existing_file(
            &target_path,
            FILE_READ_ATTRIBUTES | windows_sys::Win32::Storage::FileSystem::READ_CONTROL,
            &security,
        )? {
            Some(target) => {
                drop(target);
                let target = wide_path(&target_path)?;
                let replacement = wide_path(&temporary_path)?;
                if unsafe {
                    ReplaceFileW(
                        target.as_ptr(),
                        replacement.as_ptr(),
                        std::ptr::null(),
                        0,
                        std::ptr::null(),
                        std::ptr::null(),
                    )
                } == 0
                {
                    cleanup_temporary(&temporary_path, &security);
                    return Err(RecoveryJournalError::Io);
                }
            }
            None => {
                let temporary = wide_path(&temporary_path)?;
                let target = wide_path(&target_path)?;
                if unsafe {
                    MoveFileExW(temporary.as_ptr(), target.as_ptr(), MOVEFILE_WRITE_THROUGH)
                } == 0
                {
                    cleanup_temporary(&temporary_path, &security);
                    return Err(RecoveryJournalError::Io);
                }
            }
        }
        if persistence_failure(PersistenceFailurePoint::AfterReplace) {
            return Err(RecoveryJournalError::Io);
        }
        if let Some(target) = open_existing_file(
            &target_path,
            FILE_READ_ATTRIBUTES | windows_sys::Win32::Storage::FileSystem::READ_CONTROL,
            &security,
        )? {
            drop(target);
            Ok(())
        } else {
            Err(RecoveryJournalError::Io)
        }
    }
}

#[cfg(test)]
impl Drop for Store {
    fn drop(&mut self) {
        if self.remove_directory_on_drop {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }
}

struct SecurityContext {
    user_sid: Vec<u8>,
    trusted_sid_allowlist: Vec<Vec<u8>>,
    descriptor: windows_sys::Win32::Security::PSECURITY_DESCRIPTOR,
}

impl SecurityContext {
    fn current() -> Result<Self, RecoveryJournalError> {
        let mut token = std::ptr::null_mut();
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(RecoveryJournalError::UnsafeStorage);
        }
        let token = unsafe { OwnedHandle::from_raw_handle(token.cast::<std::ffi::c_void>()) };
        let mut required = 0_u32;
        unsafe {
            GetTokenInformation(
                token.as_raw_handle().cast(),
                TokenUser,
                std::ptr::null_mut(),
                0,
                &mut required,
            );
        }
        if required == 0 || required > 64 * 1024 {
            return Err(RecoveryJournalError::UnsafeStorage);
        }
        let mut token_data = vec![0_u8; required as usize];
        if unsafe {
            GetTokenInformation(
                token.as_raw_handle().cast(),
                TokenUser,
                token_data.as_mut_ptr().cast(),
                required,
                &mut required,
            )
        } == 0
        {
            return Err(RecoveryJournalError::UnsafeStorage);
        }
        let token_user = unsafe { &*(token_data.as_ptr().cast::<TOKEN_USER>()) };
        let user_sid_ptr = token_user.User.Sid;
        if user_sid_ptr.is_null() {
            return Err(RecoveryJournalError::UnsafeStorage);
        }
        let sid_len = unsafe { GetLengthSid(user_sid_ptr) } as usize;
        if sid_len == 0 || sid_len > 256 {
            return Err(RecoveryJournalError::UnsafeStorage);
        }
        let user_sid =
            unsafe { std::slice::from_raw_parts(user_sid_ptr.cast::<u8>(), sid_len) }.to_vec();
        let trusted_sid_allowlist = ["S-1-5-18", "S-1-5-32-544"]
            .into_iter()
            .map(sid_from_text)
            .collect::<Result<Vec<_>, _>>()?;
        let mut sid_string = std::ptr::null_mut();
        if unsafe { ConvertSidToStringSidW(user_sid_ptr, &mut sid_string) } == 0
            || sid_string.is_null()
        {
            return Err(RecoveryJournalError::UnsafeStorage);
        }
        let sid_text = unsafe { bounded_wide_string(sid_string, 256) };
        unsafe {
            LocalFree(sid_string.cast::<std::ffi::c_void>());
        }
        let sid_text = sid_text.ok_or(RecoveryJournalError::UnsafeStorage)?;
        let sddl = format!("O:{sid_text}D:P(A;;FA;;;{sid_text})");
        let sddl: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();
        let mut descriptor = std::ptr::null_mut();
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                PRIVATE_SECURITY_DESCRIPTOR_REVISION,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        } == 0
            || descriptor.is_null()
        {
            return Err(RecoveryJournalError::UnsafeStorage);
        }
        let mut dacl = std::ptr::null_mut();
        let mut present = 0;
        let mut defaulted = 0;
        if unsafe { GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted) }
            == 0
            || present == 0
            || defaulted != 0
            || dacl.is_null()
        {
            unsafe {
                LocalFree(descriptor.cast::<std::ffi::c_void>());
            }
            return Err(RecoveryJournalError::UnsafeStorage);
        }
        Ok(Self {
            user_sid,
            trusted_sid_allowlist,
            descriptor,
        })
    }

    fn security_attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.descriptor,
            bInheritHandle: 0,
        }
    }

    fn validate(&self, handle: windows_sys::Win32::Foundation::HANDLE) -> bool {
        let mut owner = std::ptr::null_mut();
        let mut dacl = std::ptr::null_mut();
        let mut descriptor = std::ptr::null_mut();
        let status = unsafe {
            GetSecurityInfo(
                handle,
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                std::ptr::null_mut(),
                &mut dacl,
                std::ptr::null_mut(),
                &mut descriptor,
            )
        };
        if status != 0 || descriptor.is_null() {
            #[cfg(test)]
            eprintln!("P038_TEST private ACL query failed: status={status}");
            return false;
        }
        let mut control = 0_u16;
        let mut revision = 0_u32;
        let mut present = 0;
        let mut defaulted = 0;
        let valid = unsafe {
            GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) != 0
                && control & (SE_DACL_PRESENT | SE_DACL_PROTECTED)
                    == (SE_DACL_PRESENT | SE_DACL_PROTECTED)
                && GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted)
                    != 0
                && present != 0
                && defaulted == 0
                && !owner.is_null()
                && EqualSid(owner, self.user_sid.as_ptr().cast_mut().cast()) != 0
                && self.exact_user_dacl(dacl)
        };
        #[cfg(test)]
        if !valid {
            eprintln!(
                "P038_TEST private ACL validation failed: control={control:#x} present={present} defaulted={defaulted} owner_null={}",
                owner.is_null()
            );
        }
        unsafe {
            LocalFree(descriptor.cast::<std::ffi::c_void>());
        }
        valid
    }

    fn safe_ancestor_dacl(&self, handle: windows_sys::Win32::Foundation::HANDLE) -> bool {
        let mut descriptor = std::ptr::null_mut();
        let status = unsafe {
            GetSecurityInfo(
                handle,
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut descriptor,
            )
        };
        if status != 0 || descriptor.is_null() {
            #[cfg(test)]
            eprintln!("P038_TEST ancestor ACL query failed: status={status}");
            return false;
        }
        let mut present = 0;
        let mut defaulted = 0;
        let mut actual_dacl = std::ptr::null_mut();
        let mut information = ACL_SIZE_INFORMATION::default();
        let valid = unsafe {
            GetSecurityDescriptorDacl(descriptor, &mut present, &mut actual_dacl, &mut defaulted)
                != 0
                && present != 0
                && defaulted == 0
                && !actual_dacl.is_null()
                && GetAclInformation(
                    actual_dacl,
                    (&mut information as *mut ACL_SIZE_INFORMATION).cast(),
                    std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
                    windows_sys::Win32::Security::AclSizeInformation,
                ) != 0
        };
        let mut safe = valid;
        #[cfg(test)]
        if !valid {
            eprintln!("P038_TEST ancestor ACL descriptor invalid");
        }
        if safe {
            for index in 0..information.AceCount {
                let mut ace_pointer = std::ptr::null_mut();
                if unsafe { GetAce(actual_dacl, index, &mut ace_pointer) } == 0
                    || ace_pointer.is_null()
                {
                    #[cfg(test)]
                    eprintln!("P038_TEST ancestor GetAce failed at index {index}");
                    safe = false;
                    break;
                }
                let header = unsafe { &*(ace_pointer.cast::<ACE_HEADER>()) };
                if header.AceType == ACCESS_DENIED_ACE_TYPE as u8 {
                    continue;
                }
                if header.AceType != ACCESS_ALLOWED_ACE_TYPE as u8 {
                    #[cfg(test)]
                    eprintln!("P038_TEST ancestor rejected ACE type {}", header.AceType);
                    safe = false;
                    break;
                }
                if header.AceFlags & INHERIT_ONLY_ACE as u8 != 0 {
                    continue;
                }
                if (header.AceSize as usize)
                    < std::mem::offset_of!(ACCESS_ALLOWED_ACE, SidStart) + 8
                {
                    #[cfg(test)]
                    eprintln!(
                        "P038_TEST ancestor rejected short ACE type {}",
                        header.AceType
                    );
                    safe = false;
                    break;
                }
                let ace = unsafe { &*(ace_pointer.cast::<ACCESS_ALLOWED_ACE>()) };
                let sid = (&ace.SidStart as *const u32).cast_mut().cast();
                let sid_length = unsafe { GetLengthSid(sid) } as usize;
                let sid_end =
                    std::mem::offset_of!(ACCESS_ALLOWED_ACE, SidStart).saturating_add(sid_length);
                if sid_length == 0 || sid_end > header.AceSize as usize {
                    #[cfg(test)]
                    eprintln!("P038_TEST ancestor rejected invalid SID size {sid_length}");
                    safe = false;
                    break;
                }
                let trusted = unsafe {
                    EqualSid(sid, self.user_sid.as_ptr().cast_mut().cast()) != 0
                        || self.trusted_sid_allowlist.iter().any(|trusted_sid| {
                            EqualSid(sid, trusted_sid.as_ptr().cast_mut().cast()) != 0
                        })
                };
                // `safe_ancestor_dacl` is called only for directories.
                // FILE_WRITE_DATA and FILE_APPEND_DATA mean add-file and
                // add-subdirectory there. Those rights cannot mutate an
                // existing protected child; DELETE_CHILD and ACL-changing
                // rights below are the replacement boundary.
                let unsafe_write_access = FILE_WRITE_EA
                    | FILE_WRITE_ATTRIBUTES
                    | FILE_DELETE_CHILD
                    | windows_sys::Win32::Storage::FileSystem::DELETE
                    | windows_sys::Win32::Storage::FileSystem::WRITE_DAC
                    | windows_sys::Win32::Storage::FileSystem::WRITE_OWNER
                    | GENERIC_WRITE
                    | GENERIC_ALL;
                if !trusted && ace.Mask & unsafe_write_access != 0 {
                    #[cfg(test)]
                    eprintln!("P038_TEST ancestor rejected non-owner mask {:#x}", ace.Mask);
                    safe = false;
                    break;
                }
            }
        }
        unsafe {
            LocalFree(descriptor.cast::<std::ffi::c_void>());
        }
        safe
    }

    fn exact_user_dacl(&self, dacl: *mut ACL) -> bool {
        if dacl.is_null() {
            return false;
        }
        let mut information = ACL_SIZE_INFORMATION::default();
        if unsafe {
            GetAclInformation(
                dacl,
                (&mut information as *mut ACL_SIZE_INFORMATION).cast(),
                std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
                windows_sys::Win32::Security::AclSizeInformation,
            )
        } == 0
            || information.AceCount != 1
        {
            return false;
        }
        let mut ace_pointer = std::ptr::null_mut();
        if unsafe { GetAce(dacl, 0, &mut ace_pointer) } == 0 || ace_pointer.is_null() {
            return false;
        }
        let ace = unsafe { &*(ace_pointer.cast::<ACCESS_ALLOWED_ACE>()) };
        let sid = (&ace.SidStart as *const u32).cast_mut().cast();
        ace.Header.AceType == ACCESS_ALLOWED_ACE_TYPE as u8
            && ace.Header.AceFlags == 0
            && ace.Mask == FILE_ALL_ACCESS
            && unsafe { EqualSid(sid, self.user_sid.as_ptr().cast_mut().cast()) != 0 }
    }
}

fn sid_from_text(value: &str) -> Result<Vec<u8>, RecoveryJournalError> {
    let value = value
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut sid = std::ptr::null_mut();
    if unsafe { ConvertStringSidToSidW(value.as_ptr(), &mut sid) } == 0 || sid.is_null() {
        return Err(RecoveryJournalError::UnsafeStorage);
    }
    let sid_length = unsafe { GetLengthSid(sid) } as usize;
    let owned_sid = if sid_length == 0 || sid_length > 256 {
        Err(RecoveryJournalError::UnsafeStorage)
    } else {
        Ok(unsafe { std::slice::from_raw_parts(sid.cast::<u8>(), sid_length) }.to_vec())
    };
    unsafe { LocalFree(sid.cast::<std::ffi::c_void>()) };
    owned_sid
}

impl Drop for SecurityContext {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.descriptor.cast::<std::ffi::c_void>());
        }
    }
}

fn ensure_private_directory(
    path: &Path,
    security: &SecurityContext,
) -> Result<OwnedHandle, RecoveryJournalError> {
    if !path.is_absolute() {
        return Err(RecoveryJournalError::DirectoryUnavailable);
    }
    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return Err(RecoveryJournalError::DirectoryUnavailable);
    };
    let disk = match prefix.kind() {
        Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => {
            std::ffi::OsString::from(format!("{}:", drive as char))
        }
        _ => return Err(RecoveryJournalError::DirectoryUnavailable),
    };
    if components.next() != Some(Component::RootDir) {
        return Err(RecoveryJournalError::DirectoryUnavailable);
    }
    let mut root = PathBuf::new();
    // `canonicalize()` returns `\\?\C:\...` on Windows. Normalize that
    // local drive prefix before passing paths to APIs that expect `C:\...`.
    root.push(disk);
    root.push("\\");
    let root_wide = wide_path(&root)?;
    let drive_type = unsafe { GetDriveTypeW(root_wide.as_ptr()) };
    if drive_type != DRIVE_FIXED_TYPE {
        #[cfg(test)]
        eprintln!("P038_TEST journal root drive type {drive_type}");
        return Err(RecoveryJournalError::UnsafeStorage);
    }
    let root_handle = open_directory(&root)?;
    if !security.safe_ancestor_dacl(root_handle.as_raw_handle().cast()) {
        #[cfg(test)]
        eprintln!("P038_TEST journal root ACL unsafe");
        return Err(RecoveryJournalError::UnsafeStorage);
    }
    let mut current = root;
    let remaining: Vec<_> = components.collect();
    if remaining.is_empty() {
        return Err(RecoveryJournalError::DirectoryUnavailable);
    }
    for (index, component) in remaining.iter().enumerate() {
        let Component::Normal(name) = component else {
            return Err(RecoveryJournalError::DirectoryUnavailable);
        };
        let name_text = name.to_string_lossy();
        if name_text.is_empty()
            || name_text.ends_with(['.', ' '])
            || name_text.contains(':')
            || name_text.contains('\0')
        {
            return Err(RecoveryJournalError::DirectoryUnavailable);
        }
        current.push(name);
        let directory = match open_directory(&current) {
            Ok(handle) => handle,
            Err(RecoveryJournalError::DirectoryUnavailable) => {
                let current_wide = wide_path(&current)?;
                let attributes = security.security_attributes();
                let created = unsafe { CreateDirectoryW(current_wide.as_ptr(), &attributes) };
                if created == 0 && unsafe { GetLastError() } != ERROR_ALREADY_EXISTS {
                    return Err(RecoveryJournalError::DirectoryUnavailable);
                }
                open_directory(&current)?
            }
            Err(error) => return Err(error),
        };
        let is_private_component =
            index + 1 == remaining.len() || name_text.eq_ignore_ascii_case("codex-meter");
        if is_private_component {
            if !security.validate(directory.as_raw_handle().cast()) {
                #[cfg(test)]
                eprintln!("P038_TEST private directory component {index} ACL unsafe");
                return Err(RecoveryJournalError::UnsafeStorage);
            }
        } else if !security.safe_ancestor_dacl(directory.as_raw_handle().cast()) {
            #[cfg(test)]
            eprintln!("P038_TEST ancestor directory component {index} ACL unsafe");
            return Err(RecoveryJournalError::UnsafeStorage);
        }
        if index + 1 == remaining.len() {
            return Ok(directory);
        }
    }
    Err(RecoveryJournalError::DirectoryUnavailable)
}

fn open_directory(path: &Path) -> Result<OwnedHandle, RecoveryJournalError> {
    let path = wide_path(path)?;
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            FILE_READ_ATTRIBUTES | windows_sys::Win32::Storage::FileSystem::READ_CONTROL,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        return if matches!(
            unsafe { GetLastError() },
            ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND
        ) {
            Err(RecoveryJournalError::DirectoryUnavailable)
        } else {
            Err(RecoveryJournalError::UnsafeStorage)
        };
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(handle.cast::<std::ffi::c_void>()) };
    let mut information = FILE_ATTRIBUTE_TAG_INFO::default();
    if unsafe {
        GetFileInformationByHandleEx(
            handle.as_raw_handle().cast(),
            FileAttributeTagInfo,
            (&mut information as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
            std::mem::size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    } == 0
        || information.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || information.FileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
    {
        return Err(RecoveryJournalError::UnsafeStorage);
    }
    Ok(handle)
}

fn open_secure_file(
    path: &Path,
    access: u32,
    share_mode: u32,
    disposition: u32,
    security: &SecurityContext,
) -> Result<OwnedHandle, RecoveryJournalError> {
    let path_wide = wide_path(path)?;
    let attributes = security.security_attributes();
    let handle = unsafe {
        CreateFileW(
            path_wide.as_ptr(),
            access,
            share_mode,
            &attributes,
            disposition,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        return Err(RecoveryJournalError::Io);
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(handle.cast::<std::ffi::c_void>()) };
    validate_regular_file(handle.as_raw_handle().cast(), security)?;
    Ok(handle)
}

fn open_existing_file(
    path: &Path,
    access: u32,
    security: &SecurityContext,
) -> Result<Option<OwnedHandle>, RecoveryJournalError> {
    let path_wide = wide_path(path)?;
    let handle = unsafe {
        CreateFileW(
            path_wide.as_ptr(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        return if unsafe { GetLastError() } == ERROR_FILE_NOT_FOUND {
            Ok(None)
        } else {
            Err(RecoveryJournalError::UnsafeStorage)
        };
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(handle.cast::<std::ffi::c_void>()) };
    validate_regular_file(handle.as_raw_handle().cast(), security)?;
    Ok(Some(handle))
}

fn validate_regular_file(
    handle: windows_sys::Win32::Foundation::HANDLE,
    security: &SecurityContext,
) -> Result<(), RecoveryJournalError> {
    let mut information = FILE_ATTRIBUTE_TAG_INFO::default();
    let mut standard_information = FILE_STANDARD_INFO::default();
    if unsafe { GetFileType(handle) } != FILE_TYPE_DISK
        || unsafe {
            GetFileInformationByHandleEx(
                handle,
                FileAttributeTagInfo,
                (&mut information as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
                std::mem::size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
            )
        } == 0
        || unsafe {
            GetFileInformationByHandleEx(
                handle,
                FileStandardInfo,
                (&mut standard_information as *mut FILE_STANDARD_INFO).cast(),
                std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
            )
        } == 0
        || information.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0
        || information.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || standard_information.NumberOfLinks != 1
        || standard_information.DeletePending
        || !security.validate(handle)
    {
        return Err(RecoveryJournalError::UnsafeStorage);
    }
    Ok(())
}

fn cleanup_temporary(path: &Path, security: &SecurityContext) {
    if persistence_failure(PersistenceFailurePoint::Cleanup) {
        return;
    }
    if let Ok(Some(handle)) = open_existing_file(
        path,
        FILE_READ_ATTRIBUTES | windows_sys::Win32::Storage::FileSystem::READ_CONTROL,
        security,
    ) {
        drop(handle);
        if let Ok(path) = wide_path(path) {
            unsafe {
                DeleteFileW(path.as_ptr());
            }
        }
    }
}

fn acquire_lock(
    handle: windows_sys::Win32::Foundation::HANDLE,
) -> Result<FileLock, RecoveryJournalError> {
    let deadline = Instant::now() + LOCK_TIMEOUT;
    let mut overlapped = OVERLAPPED::default();
    loop {
        if unsafe {
            LockFileEx(
                handle,
                LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
                0,
                LOCK_FILE_LENGTH_LOW,
                0,
                &mut overlapped,
            )
        } != 0
        {
            return Ok(FileLock { handle, overlapped });
        }
        if unsafe { GetLastError() } != ERROR_LOCK_VIOLATION || Instant::now() >= deadline {
            return Err(RecoveryJournalError::Io);
        }
        thread::sleep(LOCK_POLL_INTERVAL);
    }
}

struct FileLock {
    handle: windows_sys::Win32::Foundation::HANDLE,
    overlapped: OVERLAPPED,
}

impl Drop for FileLock {
    fn drop(&mut self) {
        unsafe {
            UnlockFileEx(
                self.handle,
                0,
                LOCK_FILE_LENGTH_LOW,
                0,
                &mut self.overlapped,
            );
        }
    }
}

fn file_from_handle(handle: OwnedHandle) -> File {
    unsafe { File::from_raw_handle(handle.into_raw_handle() as RawHandle) }
}

fn wide_path(path: &Path) -> Result<Vec<u16>, RecoveryJournalError> {
    let mut path: Vec<u16> = path.as_os_str().encode_wide().collect();
    if path.is_empty() || path.len() >= MAX_PATH_UNITS || path.contains(&0) {
        return Err(RecoveryJournalError::DirectoryUnavailable);
    }
    path.push(0);
    Ok(path)
}

unsafe fn bounded_wide_string(value: *const u16, maximum: usize) -> Option<String> {
    let mut length = 0;
    while length < maximum && *value.add(length) != 0 {
        length += 1;
    }
    if length == maximum {
        return None;
    }
    String::from_utf16(std::slice::from_raw_parts(value, length)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::sync::{Arc, Barrier};

    fn temp_path(label: &str) -> PathBuf {
        let local_data = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .expect("the Windows test runner provides a per-user local data directory");
        local_data.join(format!(
            "codex-meter-windows-{label}-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn private_acl_atomic_replacement_and_revision_conflicts() {
        let path = temp_path("journal");
        let journal = RecoveryJournal::open_at(&path).unwrap();
        let first = journal.create(ExecutionOrigin::NewThread, None).unwrap();
        let initial_bytes = std::fs::read(path.join("journal.json")).unwrap();
        assert!(!path.join(".journal.tmp").exists());

        let security = SecurityContext::current().unwrap();
        let directory = open_directory(&path).unwrap();
        assert!(security.validate(directory.as_raw_handle().cast()));
        for private_file in [path.join(".lock"), path.join("journal.json")] {
            let handle = open_existing_file(
                &private_file,
                FILE_READ_ATTRIBUTES | windows_sys::Win32::Storage::FileSystem::READ_CONTROL,
                &security,
            )
            .unwrap()
            .unwrap();
            assert!(security.validate(handle.as_raw_handle().cast()));
        }

        inject_persistence_failures(&[
            PersistenceFailurePoint::DuringWrite,
            PersistenceFailurePoint::Cleanup,
        ]);
        assert_eq!(
            journal.create(ExecutionOrigin::NewThread, None),
            Err(RecoveryJournalError::Io)
        );
        let stale_temporary = open_existing_file(
            &path.join(".journal.tmp"),
            FILE_READ_ATTRIBUTES | windows_sys::Win32::Storage::FileSystem::READ_CONTROL,
            &security,
        )
        .unwrap()
        .unwrap();
        assert!(security.validate(stale_temporary.as_raw_handle().cast()));
        drop(stale_temporary);
        inject_persistence_failures(&[]);

        let mut current = first.clone();
        current.execution_phase = RecoveryPhase::Failed;
        let current = journal.update(&current).unwrap();
        let replaced_bytes = std::fs::read(path.join("journal.json")).unwrap();
        assert_ne!(initial_bytes, replaced_bytes);
        assert!(!path.join(".journal.tmp").exists());
        let mut stale_update = first;
        stale_update.execution_phase = RecoveryPhase::Cancelled;
        assert_eq!(
            journal.update(&stale_update),
            Err(RecoveryJournalError::ConcurrentUpdate)
        );
        assert_eq!(journal.get(&current.execution_id).unwrap(), current);
        drop(journal);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn concurrent_process_updates_keep_all_records() {
        let path = temp_path("concurrent");
        let journal = RecoveryJournal::open_at(&path).unwrap();
        let executable = std::env::current_exe().unwrap();
        let execution_ids = (0..4)
            .map(|_| {
                let mut record = journal.create(ExecutionOrigin::NewThread, None).unwrap();
                record.execution_phase = RecoveryPhase::Running;
                record.mutation_intent = MutationIntent::TurnStart;
                journal.update(&record).unwrap().execution_id
            })
            .collect::<Vec<_>>();
        let barrier = Arc::new(Barrier::new(5));
        let mut workers = Vec::new();
        for execution_id in &execution_ids {
            let executable = executable.clone();
            let path = path.clone();
            let execution_id = execution_id.clone();
            let barrier = Arc::clone(&barrier);
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                Command::new(executable)
                    .arg("--exact")
                    .arg("codex_app_server::execution_recovery::platform::tests::journal_writer_subprocess_helper")
                    .env("CODEX_METER_TEST_JOURNAL_PATH", path)
                    .env("CODEX_METER_TEST_JOURNAL_MODE", "complete")
                    .env("CODEX_METER_TEST_JOURNAL_EXECUTION_ID", execution_id)
                    .output()
                    .unwrap()
            }));
        }
        barrier.wait();
        for worker in workers {
            let output = worker.join().unwrap();
            assert!(
                output.status.success(),
                "journal writer subprocess failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let completed_records = journal.snapshot().unwrap();
        assert_eq!(completed_records.len(), 4);
        assert!(completed_records.iter().all(|record| {
            execution_ids.contains(&record.execution_id)
                && record.execution_phase == RecoveryPhase::Completed
                && record.revision() == 2
        }));

        let crash = Command::new(executable)
            .arg("--exact")
            .arg("codex_app_server::execution_recovery::platform::tests::journal_writer_subprocess_helper")
            .env("CODEX_METER_TEST_JOURNAL_PATH", &path)
            .env("CODEX_METER_TEST_JOURNAL_MODE", "abort-while-locked")
            .output()
            .unwrap();
        assert!(
            !crash.status.success(),
            "the child must terminate while holding the lock"
        );
        assert_eq!(journal.snapshot().unwrap().len(), 4);
        drop(journal);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn journal_writer_subprocess_helper() {
        let (Some(path), Some(mode)) = (
            std::env::var_os("CODEX_METER_TEST_JOURNAL_PATH"),
            std::env::var_os("CODEX_METER_TEST_JOURNAL_MODE"),
        ) else {
            return;
        };
        let journal = RecoveryJournal::open_at(Path::new(&path)).unwrap();
        if mode.to_string_lossy() == "abort-while-locked" {
            let _: Result<(), RecoveryJournalError> =
                journal.store.with_lock(|| std::process::abort());
            unreachable!("the child process must abort inside the lock");
        }
        let execution_id = std::env::var("CODEX_METER_TEST_JOURNAL_EXECUTION_ID").unwrap();
        let mut record = journal.get(&execution_id).unwrap();
        record.execution_phase = RecoveryPhase::Completed;
        record.last_authoritative_status = Some(AuthoritativeStatus::Completed);
        journal.update(&record).unwrap();
    }

    #[test]
    fn journal_rejects_an_acl_grant_to_other_local_principals() {
        let path = temp_path("unsafe-acl");
        std::fs::create_dir_all(&path).unwrap();
        let security = SecurityContext::current().unwrap();
        let mut sid_string = std::ptr::null_mut();
        let token_sid = security.user_sid.as_ptr().cast_mut().cast();
        assert_ne!(
            unsafe { ConvertSidToStringSidW(token_sid, &mut sid_string) },
            0
        );
        let sid_text = unsafe { bounded_wide_string(sid_string, 256) }.unwrap();
        unsafe { LocalFree(sid_string.cast()) };
        let descriptor_text = format!("O:{sid_text}D:P(A;;FA;;;{sid_text})(A;;FA;;;WD)");
        let descriptor_text = descriptor_text
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let mut descriptor = std::ptr::null_mut();
        assert_ne!(
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    descriptor_text.as_ptr(),
                    PRIVATE_SECURITY_DESCRIPTOR_REVISION,
                    &mut descriptor,
                    std::ptr::null_mut(),
                )
            },
            0
        );
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let broad_directory = path.join("codex-meter");
        let broad_path = wide_path(&broad_directory).unwrap();
        assert_ne!(
            unsafe { CreateDirectoryW(broad_path.as_ptr(), &attributes) },
            0
        );
        unsafe { LocalFree(descriptor.cast()) };
        let broad_handle = open_directory(&broad_directory).unwrap();
        assert!(!security.safe_ancestor_dacl(broad_handle.as_raw_handle().cast()));
        drop(broad_handle);

        let append_descriptor_text = format!("O:{sid_text}D:P(A;;FA;;;{sid_text})(A;;0x4;;;WD)");
        let append_descriptor_text = append_descriptor_text
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let mut append_descriptor = std::ptr::null_mut();
        assert_ne!(
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    append_descriptor_text.as_ptr(),
                    PRIVATE_SECURITY_DESCRIPTOR_REVISION,
                    &mut append_descriptor,
                    std::ptr::null_mut(),
                )
            },
            0
        );
        let append_attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: append_descriptor,
            bInheritHandle: 0,
        };
        let append_directory = path.join("append-only");
        let append_path = wide_path(&append_directory).unwrap();
        assert_ne!(
            unsafe { CreateDirectoryW(append_path.as_ptr(), &append_attributes) },
            0
        );
        unsafe { LocalFree(append_descriptor.cast()) };
        let append_handle = open_directory(&append_directory).unwrap();
        assert!(security.safe_ancestor_dacl(append_handle.as_raw_handle().cast()));
        drop(append_handle);

        assert!(matches!(
            RecoveryJournal::open_at(&path.join("codex-meter/recovery")),
            Err(RecoveryJournalError::UnsafeStorage)
        ));
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn journal_rejects_hard_linked_records() {
        let path = temp_path("hard-link");
        let journal = RecoveryJournal::open_at(&path).unwrap();
        journal.create(ExecutionOrigin::NewThread, None).unwrap();
        let external_link = path.join("external-link.json");
        std::fs::hard_link(path.join("journal.json"), &external_link).unwrap();
        assert_eq!(journal.snapshot(), Err(RecoveryJournalError::UnsafeStorage));
        std::fs::remove_file(external_link).unwrap();
        assert_eq!(journal.snapshot().unwrap().len(), 1);
        drop(journal);
        std::fs::remove_dir_all(path).unwrap();
    }
}
