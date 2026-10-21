//! Explicitly user-owned prompt drafts, isolated from telemetry storage.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use super::composer::{ModelSelection, ReasoningSelection, TargetKind, MAX_PROMPT_BYTES};

const FORMAT_VERSION: u32 = 1;
const MAX_TITLE_BYTES: usize = 384;
const MAX_DRAFT_FILE_BYTES: usize = 128 * 1024;
const MAX_DRAFTS: usize = 100;
const MAX_TOTAL_BYTES: u64 = 10 * 1024 * 1024;
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DraftError {
    #[cfg(not(unix))]
    UnsupportedPlatform,
    DirectoryUnavailable,
    UnsafeDirectory,
    InvalidDraft,
    MissingDraft,
    CorruptDraft,
    TooManyDrafts,
    StorageLimit,
    Conflict,
    Io,
}

impl DraftError {
    pub const fn message(self) -> &'static str {
        match self {
            #[cfg(not(unix))]
            Self::UnsupportedPlatform => {
                "Private local draft storage is unavailable on this platform. Use an encrypted user filesystem and keep the prompt in memory."
            }
            Self::DirectoryUnavailable => {
                "Could not open the private draft directory; the prompt remains in memory."
            }
            Self::UnsafeDirectory => {
                "Draft storage contains a symbolic link or unsafe file; no draft was changed."
            }
            Self::InvalidDraft => "The draft title or contents exceed the supported limits.",
            Self::MissingDraft => "That saved draft is no longer available.",
            Self::CorruptDraft => "A saved draft is malformed and was skipped.",
            Self::TooManyDrafts => "The saved draft limit has been reached; delete a draft first.",
            Self::StorageLimit => "The private draft storage limit has been reached.",
            Self::Conflict => {
                "This draft changed or was deleted elsewhere; reload it before saving again."
            }
            Self::Io => "Draft storage could not complete the operation; the prompt remains in memory.",
        }
    }
}

pub struct DraftContents {
    pub title: String,
    pub prompt: String,
    pub target_kind: TargetKind,
    pub target_thread_id: Option<String>,
    pub workspace_selected: bool,
    pub model_selection: ModelSelection,
    pub reasoning_selection: ReasoningSelection,
}

pub struct LoadedDraft {
    pub id: String,
    pub digest: String,
    pub saved_at: String,
    pub contents: DraftContents,
}

#[derive(Clone, Eq, PartialEq)]
pub struct DraftSummary {
    pub id: String,
    pub title: String,
    pub saved_at: String,
    pub file_bytes: usize,
}

pub struct DraftListing {
    pub drafts: Vec<DraftSummary>,
    pub skipped_entries: usize,
    pub total_bytes: u64,
    pub overflowed: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DraftFile {
    version: u32,
    id: String,
    title: String,
    saved_at: String,
    prompt: String,
    target_kind: TargetKind,
    target_thread_id: Option<String>,
    workspace_selected: bool,
    model_selection: ModelSelection,
    reasoning_selection: ReasoningSelection,
}

fn validate_contents(contents: &DraftContents) -> Result<(), DraftError> {
    if contents.title.trim().is_empty()
        || contents.title.len() > MAX_TITLE_BYTES
        || contents.title.chars().any(char::is_control)
        || contents.prompt.len() > MAX_PROMPT_BYTES
        || contents
            .prompt
            .chars()
            .any(|character| character.is_control() && character != '\n' && character != '\t')
        || (contents.target_kind == TargetKind::ExistingThread
            && contents
                .target_thread_id
                .as_ref()
                .is_none_or(|thread_id| thread_id.is_empty() || thread_id.len() > 128))
        || (contents.target_kind == TargetKind::NewThread && contents.target_thread_id.is_some())
    {
        return Err(DraftError::InvalidDraft);
    }
    Ok(())
}

fn now_string() -> Result<String, DraftError> {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|_| DraftError::Io)
}

fn digest(bytes: &[u8]) -> String {
    let bytes = Sha256::digest(bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn next_id() -> String {
    let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let material = format!(
        "{}:{}:{}:{}",
        std::process::id(),
        sequence,
        OffsetDateTime::now_utc().unix_timestamp_nanos(),
        std::thread::current().name().unwrap_or("draft")
    );
    digest(material.as_bytes())
}

#[cfg(unix)]
mod platform {
    use super::*;
    use std::ffi::{CStr, CString, OsStr};
    use std::fs::File;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;

    const PRIVATE_MODE: libc::mode_t = 0o700;
    const FILE_MODE: libc::mode_t = 0o600;

    pub struct DraftStore {
        directory: OwnedFd,
    }

    impl DraftStore {
        pub fn open_default() -> Result<Self, DraftError> {
            let data_directory = std::env::var_os("XDG_DATA_HOME")
                .map(std::path::PathBuf::from)
                .filter(|path| path.is_absolute())
                .or_else(|| {
                    std::env::var_os("HOME")
                        .map(std::path::PathBuf::from)
                        .filter(|path| path.is_absolute())
                        .map(|home| home.join(".local").join("share"))
                })
                .ok_or(DraftError::DirectoryUnavailable)?;
            Self::open_at(&data_directory.join("codex-meter").join("drafts"))
        }

        pub fn open_at(path: &Path) -> Result<Self, DraftError> {
            if !path.is_absolute() {
                return Err(DraftError::UnsafeDirectory);
            }
            let root = CString::new("/").map_err(|_| DraftError::DirectoryUnavailable)?;
            // SAFETY: `root` is a valid NUL-terminated path and open returns a new descriptor.
            let root_fd = unsafe {
                libc::open(
                    root.as_ptr(),
                    libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW,
                )
            };
            if root_fd < 0 {
                return Err(DraftError::DirectoryUnavailable);
            }
            // SAFETY: `root_fd` was returned by open and is now owned here.
            let mut current = unsafe { OwnedFd::from_raw_fd(root_fd) };
            let components = path
                .components()
                .filter_map(|component| match component {
                    std::path::Component::RootDir => None,
                    std::path::Component::Normal(value) => Some(Ok(value)),
                    _ => Some(Err(DraftError::UnsafeDirectory)),
                })
                .collect::<Result<Vec<_>, _>>()?;
            if components.is_empty() {
                return Err(DraftError::UnsafeDirectory);
            }
            let uid = unsafe { libc::getuid() };
            for (index, component) in components.iter().enumerate() {
                let name = cstring(component)?;
                let mut next_fd = unsafe {
                    libc::openat(
                        current.as_raw_fd(),
                        name.as_ptr(),
                        libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW,
                    )
                };
                if next_fd < 0
                    && std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound
                {
                    let mode = if index + 1 == components.len() {
                        PRIVATE_MODE
                    } else {
                        0o755
                    };
                    let created =
                        unsafe { libc::mkdirat(current.as_raw_fd(), name.as_ptr(), mode) };
                    if created != 0
                        && std::io::Error::last_os_error().kind()
                            != std::io::ErrorKind::AlreadyExists
                    {
                        return Err(DraftError::DirectoryUnavailable);
                    }
                    next_fd = unsafe {
                        libc::openat(
                            current.as_raw_fd(),
                            name.as_ptr(),
                            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW,
                        )
                    };
                }
                if next_fd < 0 {
                    return Err(DraftError::UnsafeDirectory);
                }
                let next = unsafe { OwnedFd::from_raw_fd(next_fd) };
                let mut metadata = unsafe { std::mem::zeroed::<libc::stat>() };
                if unsafe { libc::fstat(next.as_raw_fd(), &mut metadata) } != 0
                    || metadata.st_mode & libc::S_IFMT != libc::S_IFDIR
                    || (index + 1 == components.len() && metadata.st_uid != uid)
                {
                    return Err(DraftError::UnsafeDirectory);
                }
                if index + 1 == components.len()
                    && unsafe { libc::fchmod(next.as_raw_fd(), PRIVATE_MODE) } != 0
                {
                    return Err(DraftError::DirectoryUnavailable);
                }
                current = next;
            }
            Ok(Self { directory: current })
        }

        pub fn list(&self) -> Result<DraftListing, DraftError> {
            self.with_lock(|| self.list_unlocked())
        }

        pub fn load(&self, id: &str) -> Result<LoadedDraft, DraftError> {
            validate_id(id)?;
            self.read_unlocked(id)
        }

        pub fn save_new(&self, contents: DraftContents) -> Result<LoadedDraft, DraftError> {
            validate_contents(&contents)?;
            self.with_lock(|| {
                let listing = self.list_unlocked()?;
                if listing.overflowed {
                    return Err(DraftError::TooManyDrafts);
                }
                let count = listing.drafts.len() + listing.skipped_entries;
                if count >= MAX_DRAFTS {
                    return Err(DraftError::TooManyDrafts);
                }
                let total = listing.total_bytes;
                if total >= MAX_TOTAL_BYTES {
                    return Err(DraftError::StorageLimit);
                }
                self.write_new_unlocked(contents)
            })
        }

        pub fn replace(
            &self,
            id: &str,
            expected_digest: &str,
            contents: DraftContents,
        ) -> Result<LoadedDraft, DraftError> {
            validate_id(id)?;
            validate_contents(&contents)?;
            self.with_lock(|| {
                let current = self.read_unlocked(id).map_err(|error| match error {
                    DraftError::MissingDraft => DraftError::Conflict,
                    other => other,
                })?;
                if current.digest != expected_digest {
                    return Err(DraftError::Conflict);
                }
                let listing = self.list_unlocked()?;
                if listing.overflowed {
                    return Err(DraftError::TooManyDrafts);
                }
                let existing = listing
                    .drafts
                    .iter()
                    .find(|draft| draft.id == id)
                    .map_or(0, |draft| draft.file_bytes as u64);
                let total = listing.total_bytes;
                if total.saturating_sub(existing) >= MAX_TOTAL_BYTES {
                    return Err(DraftError::StorageLimit);
                }
                self.write_unlocked(id, contents)
            })
        }

        pub fn delete(&self, id: &str, expected_digest: &str) -> Result<(), DraftError> {
            validate_id(id)?;
            self.with_lock(|| {
                let current = self.read_unlocked(id).map_err(|error| match error {
                    DraftError::MissingDraft => DraftError::Conflict,
                    other => other,
                })?;
                if current.digest != expected_digest {
                    return Err(DraftError::Conflict);
                }
                let name = cstring(format!("{id}.json"))?;
                let result =
                    unsafe { libc::unlinkat(self.directory.as_raw_fd(), name.as_ptr(), 0) };
                if result != 0 {
                    return Err(DraftError::Io);
                }
                self.sync_directory()
            })
        }

        fn write_new_unlocked(&self, contents: DraftContents) -> Result<LoadedDraft, DraftError> {
            for _ in 0..4 {
                let id = next_id();
                let target = format!("{id}.json");
                if self.file_exists(&target)? {
                    continue;
                }
                return self.write_unlocked(&id, contents);
            }
            Err(DraftError::Io)
        }

        fn write_unlocked(
            &self,
            id: &str,
            contents: DraftContents,
        ) -> Result<LoadedDraft, DraftError> {
            let file = DraftFile {
                version: FORMAT_VERSION,
                id: id.to_owned(),
                title: contents.title.trim().to_owned(),
                saved_at: now_string()?,
                prompt: contents.prompt,
                target_kind: contents.target_kind,
                target_thread_id: contents.target_thread_id,
                workspace_selected: contents.workspace_selected,
                model_selection: contents.model_selection,
                reasoning_selection: contents.reasoning_selection,
            };
            let bytes = serde_json::to_vec(&file).map_err(|_| DraftError::InvalidDraft)?;
            if bytes.len() > MAX_DRAFT_FILE_BYTES {
                return Err(DraftError::InvalidDraft);
            }
            let listing = self.list_unlocked()?;
            let current_size = listing
                .drafts
                .iter()
                .find(|draft| draft.id == id)
                .map_or(0, |draft| draft.file_bytes as u64);
            let current_total = listing.total_bytes;
            if current_total
                .saturating_sub(current_size)
                .saturating_add(bytes.len() as u64)
                > MAX_TOTAL_BYTES
            {
                return Err(DraftError::StorageLimit);
            }
            let temporary = format!(".tmp-{}", next_id());
            let temporary_c = cstring(&temporary)?;
            let fd = unsafe {
                libc::openat(
                    self.directory.as_raw_fd(),
                    temporary_c.as_ptr(),
                    libc::O_WRONLY
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_CLOEXEC
                        | libc::O_NOFOLLOW,
                    FILE_MODE,
                )
            };
            if fd < 0 {
                return Err(DraftError::Io);
            }
            let owned = unsafe { OwnedFd::from_raw_fd(fd) };
            let mut output = File::from(owned);
            let write_result = output.write_all(&bytes).and_then(|()| output.sync_all());
            if write_result.is_err() {
                let _ = self.unlink(&temporary);
                return Err(DraftError::Io);
            }
            let source = cstring(&temporary)?;
            let target = cstring(format!("{id}.json"))?;
            let renamed = unsafe {
                libc::renameat(
                    self.directory.as_raw_fd(),
                    source.as_ptr(),
                    self.directory.as_raw_fd(),
                    target.as_ptr(),
                )
            };
            if renamed != 0 {
                let _ = self.unlink(&temporary);
                return Err(DraftError::Io);
            }
            self.sync_directory()?;
            let digest = digest(&bytes);
            Ok(LoadedDraft {
                id: file.id,
                digest,
                saved_at: file.saved_at,
                contents: DraftContents {
                    title: file.title,
                    prompt: file.prompt,
                    target_kind: file.target_kind,
                    target_thread_id: file.target_thread_id,
                    workspace_selected: file.workspace_selected,
                    model_selection: file.model_selection,
                    reasoning_selection: file.reasoning_selection,
                },
            })
        }

        fn read_unlocked(&self, id: &str) -> Result<LoadedDraft, DraftError> {
            let name = format!("{id}.json");
            let fd = self.open_regular(&name)?;
            let metadata = File::from(fd.try_clone().map_err(|_| DraftError::Io)?)
                .metadata()
                .map_err(|_| DraftError::Io)?;
            if metadata.len() as usize > MAX_DRAFT_FILE_BYTES {
                return Err(DraftError::CorruptDraft);
            }
            let mut bytes = Vec::with_capacity(metadata.len() as usize);
            File::from(fd)
                .take((MAX_DRAFT_FILE_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
                .map_err(|_| DraftError::Io)?;
            if bytes.len() > MAX_DRAFT_FILE_BYTES {
                return Err(DraftError::CorruptDraft);
            }
            let file: DraftFile =
                serde_json::from_slice(&bytes).map_err(|_| DraftError::CorruptDraft)?;
            if file.version != FORMAT_VERSION
                || file.id != id
                || file.prompt.len() > MAX_PROMPT_BYTES
            {
                return Err(DraftError::CorruptDraft);
            }
            let contents = DraftContents {
                title: file.title,
                prompt: file.prompt,
                target_kind: file.target_kind,
                target_thread_id: file.target_thread_id,
                workspace_selected: file.workspace_selected,
                model_selection: file.model_selection,
                reasoning_selection: file.reasoning_selection,
            };
            validate_contents(&contents).map_err(|_| DraftError::CorruptDraft)?;
            Ok(LoadedDraft {
                id: id.to_owned(),
                digest: digest(&bytes),
                saved_at: file.saved_at,
                contents,
            })
        }

        fn list_unlocked(&self) -> Result<DraftListing, DraftError> {
            let current = cstring(".")?;
            let directory_fd = unsafe {
                libc::openat(
                    self.directory.as_raw_fd(),
                    current.as_ptr(),
                    libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW,
                )
            };
            if directory_fd < 0 {
                return Err(DraftError::Io);
            }
            let directory = unsafe { libc::fdopendir(directory_fd) };
            if directory.is_null() {
                unsafe { libc::close(directory_fd) };
                return Err(DraftError::Io);
            }
            let mut names = Vec::new();
            let mut overflowed = false;
            loop {
                let entry = unsafe { libc::readdir(directory) };
                if entry.is_null() {
                    break;
                }
                let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
                if name == b"." || name == b".." {
                    continue;
                }
                if name.starts_with(b".tmp-") {
                    if let Ok(temporary) = CString::new(name) {
                        let _ = unsafe {
                            libc::unlinkat(self.directory.as_raw_fd(), temporary.as_ptr(), 0)
                        };
                    }
                    continue;
                }
                if !name.ends_with(b".json") {
                    continue;
                }
                if names.len() == MAX_DRAFTS {
                    overflowed = true;
                    break;
                }
                names.push(name.to_vec());
            }
            unsafe { libc::closedir(directory) };
            names.sort();
            let mut drafts = Vec::new();
            let mut skipped_entries = 0;
            let mut total_bytes = 0_u64;
            for name in names {
                let Ok(name_text) = std::str::from_utf8(&name) else {
                    skipped_entries += 1;
                    continue;
                };
                let file_bytes = self.file_size(name_text).unwrap_or(0);
                total_bytes = total_bytes.saturating_add(file_bytes as u64);
                let Some(id) = name_text.strip_suffix(".json") else {
                    skipped_entries += 1;
                    continue;
                };
                if validate_id(id).is_err() {
                    skipped_entries += 1;
                    continue;
                }
                match self.read_unlocked(id) {
                    Ok(draft) => drafts.push(DraftSummary {
                        id: draft.id,
                        title: draft.contents.title,
                        saved_at: draft.saved_at,
                        file_bytes,
                    }),
                    Err(_) => skipped_entries += 1,
                }
            }
            Ok(DraftListing {
                drafts,
                skipped_entries,
                total_bytes,
                overflowed,
            })
        }

        fn file_exists(&self, name: &str) -> Result<bool, DraftError> {
            let name = cstring(name)?;
            let mut metadata = unsafe { std::mem::zeroed::<libc::stat>() };
            let result = unsafe {
                libc::fstatat(
                    self.directory.as_raw_fd(),
                    name.as_ptr(),
                    &mut metadata,
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            };
            if result == 0 {
                Ok(true)
            } else if std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
                Ok(false)
            } else {
                Err(DraftError::Io)
            }
        }

        fn file_size(&self, name: &str) -> Result<usize, DraftError> {
            let fd = self.open_regular(name)?;
            let file = File::from(fd);
            let metadata = file.metadata().map_err(|_| DraftError::Io)?;
            Ok(metadata.len().min(usize::MAX as u64) as usize)
        }

        fn open_regular(&self, name: &str) -> Result<OwnedFd, DraftError> {
            let name = cstring(name)?;
            let fd = unsafe {
                libc::openat(
                    self.directory.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                )
            };
            if fd < 0 {
                return match std::io::Error::last_os_error().kind() {
                    std::io::ErrorKind::NotFound => Err(DraftError::MissingDraft),
                    std::io::ErrorKind::PermissionDenied => Err(DraftError::UnsafeDirectory),
                    _ => Err(DraftError::UnsafeDirectory),
                };
            }
            let owned = unsafe { OwnedFd::from_raw_fd(fd) };
            let mut metadata = unsafe { std::mem::zeroed::<libc::stat>() };
            if unsafe { libc::fstat(owned.as_raw_fd(), &mut metadata) } != 0
                || metadata.st_mode & libc::S_IFMT != libc::S_IFREG
                || metadata.st_uid != unsafe { libc::getuid() }
            {
                return Err(DraftError::UnsafeDirectory);
            }
            if unsafe { libc::fchmod(owned.as_raw_fd(), FILE_MODE) } != 0 {
                return Err(DraftError::UnsafeDirectory);
            }
            Ok(owned)
        }

        fn unlink(&self, name: &str) -> Result<(), DraftError> {
            let name = cstring(name)?;
            if unsafe { libc::unlinkat(self.directory.as_raw_fd(), name.as_ptr(), 0) } != 0 {
                return Err(DraftError::Io);
            }
            Ok(())
        }

        fn sync_directory(&self) -> Result<(), DraftError> {
            if unsafe { libc::fsync(self.directory.as_raw_fd()) } != 0 {
                return Err(DraftError::Io);
            }
            Ok(())
        }

        fn with_lock<T>(
            &self,
            action: impl FnOnce() -> Result<T, DraftError>,
        ) -> Result<T, DraftError> {
            let name = cstring(".lock")?;
            let fd = unsafe {
                libc::openat(
                    self.directory.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDWR | libc::O_CREAT | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                    FILE_MODE,
                )
            };
            if fd < 0 {
                return Err(DraftError::UnsafeDirectory);
            }
            let lock = unsafe { OwnedFd::from_raw_fd(fd) };
            let mut metadata = unsafe { std::mem::zeroed::<libc::stat>() };
            if unsafe { libc::fstat(lock.as_raw_fd(), &mut metadata) } != 0
                || metadata.st_mode & libc::S_IFMT != libc::S_IFREG
                || metadata.st_uid != unsafe { libc::getuid() }
                || unsafe { libc::fchmod(lock.as_raw_fd(), FILE_MODE) } != 0
            {
                return Err(DraftError::UnsafeDirectory);
            }
            if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
                return Err(DraftError::Io);
            }
            let result = action();
            unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_UN) };
            result
        }
    }

    fn cstring(value: impl AsRef<OsStr>) -> Result<CString, DraftError> {
        CString::new(value.as_ref().as_bytes()).map_err(|_| DraftError::UnsafeDirectory)
    }

    fn validate_id(id: &str) -> Result<(), DraftError> {
        if id.len() != 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(DraftError::MissingDraft);
        }
        Ok(())
    }
}

#[cfg(not(unix))]
mod platform {
    use super::*;

    pub struct DraftStore;

    impl DraftStore {
        pub fn open_default() -> Result<Self, DraftError> {
            Err(DraftError::UnsupportedPlatform)
        }

        pub fn open_at(_path: &Path) -> Result<Self, DraftError> {
            Err(DraftError::UnsupportedPlatform)
        }

        pub fn list(&self) -> Result<DraftListing, DraftError> {
            Err(DraftError::UnsupportedPlatform)
        }

        pub fn load(&self, _id: &str) -> Result<LoadedDraft, DraftError> {
            Err(DraftError::UnsupportedPlatform)
        }

        pub fn save_new(&self, _contents: DraftContents) -> Result<LoadedDraft, DraftError> {
            Err(DraftError::UnsupportedPlatform)
        }

        pub fn replace(
            &self,
            _id: &str,
            _expected_digest: &str,
            _contents: DraftContents,
        ) -> Result<LoadedDraft, DraftError> {
            Err(DraftError::UnsupportedPlatform)
        }

        pub fn delete(&self, _id: &str, _expected_digest: &str) -> Result<(), DraftError> {
            Err(DraftError::UnsupportedPlatform)
        }
    }
}

pub use platform::DraftStore;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_TEST_PATH: AtomicUsize = AtomicUsize::new(1);

    fn test_dir() -> PathBuf {
        std::env::temp_dir().join(format!(
            "codex-meter-drafts-test-{}-{}",
            std::process::id(),
            NEXT_TEST_PATH.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn contents(prompt: &str) -> DraftContents {
        DraftContents {
            title: "Review prompt".into(),
            prompt: prompt.into(),
            target_kind: TargetKind::NewThread,
            target_thread_id: None,
            workspace_selected: false,
            model_selection: ModelSelection::Inherit,
            reasoning_selection: ReasoningSelection::Inherit,
        }
    }

    #[cfg(unix)]
    #[test]
    fn drafts_are_saved_only_explicitly_listed_loaded_replaced_and_deleted() {
        let path = test_dir();
        let store = DraftStore::open_at(&path).unwrap();
        let saved = store.save_new(contents("private prompt canary")).unwrap();
        assert_eq!(saved.contents.prompt, "private prompt canary");
        std::fs::write(path.join(".tmp-interrupted-write"), b"partial prompt").unwrap();
        assert_eq!(store.list().unwrap().drafts.len(), 1);
        assert!(!path.join(".tmp-interrupted-write").exists());
        let loaded = store.load(&saved.id).unwrap();
        assert_eq!(loaded.contents.prompt, "private prompt canary");

        let updated = store
            .replace(&saved.id, &loaded.digest, contents("updated prompt"))
            .unwrap();
        assert_eq!(
            store.load(&updated.id).unwrap().contents.prompt,
            "updated prompt"
        );
        assert!(matches!(
            store.replace(&saved.id, &loaded.digest, contents("stale update")),
            Err(DraftError::Conflict)
        ));
        store.delete(&updated.id, &updated.digest).unwrap();
        assert!(store.list().unwrap().drafts.is_empty());
        std::fs::remove_dir_all(path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn concurrent_replacements_of_the_same_saved_version_conflict() {
        let path = test_dir();
        let first_store = DraftStore::open_at(&path).unwrap();
        let second_store = DraftStore::open_at(&path).unwrap();
        let saved = first_store.save_new(contents("initial")).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let first_barrier = barrier.clone();
        let first_id = saved.id.clone();
        let first_digest = saved.digest.clone();
        let first = std::thread::spawn(move || {
            first_barrier.wait();
            first_store.replace(&first_id, &first_digest, contents("first writer"))
        });
        let second_barrier = barrier.clone();
        let second_id = saved.id.clone();
        let second_digest = saved.digest.clone();
        let second = std::thread::spawn(move || {
            second_barrier.wait();
            second_store.replace(&second_id, &second_digest, contents("second writer"))
        });
        let first = first.join().unwrap();
        let second = second.join().unwrap();
        assert_ne!(first.is_ok(), second.is_ok());
        assert!(
            matches!(first, Err(DraftError::Conflict))
                || matches!(second, Err(DraftError::Conflict))
        );
        assert_eq!(
            DraftStore::open_at(&path)
                .unwrap()
                .list()
                .unwrap()
                .drafts
                .len(),
            1
        );
        std::fs::remove_dir_all(path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn draft_count_is_bounded_and_oversized_directories_are_reported() {
        let path = test_dir();
        let store = DraftStore::open_at(&path).unwrap();
        for index in 0..MAX_DRAFTS {
            let mut draft = contents("");
            draft.title = format!("Draft {index}");
            store.save_new(draft).unwrap();
        }
        assert!(matches!(
            store.save_new(contents("another")),
            Err(DraftError::TooManyDrafts)
        ));
        std::fs::remove_dir_all(&path).unwrap();

        let overflow_path = test_dir();
        let overflow_store = DraftStore::open_at(&overflow_path).unwrap();
        for index in 0..=MAX_DRAFTS {
            let id = format!("{index:064x}");
            std::fs::write(overflow_path.join(format!("{id}.json")), b"{}").unwrap();
        }
        let listing = overflow_store.list().unwrap();
        assert!(listing.overflowed);
        assert!(matches!(
            overflow_store.save_new(contents("overflow")),
            Err(DraftError::TooManyDrafts)
        ));
        std::fs::remove_dir_all(overflow_path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn draft_file_names_are_opaque_and_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let path = test_dir();
        let store = DraftStore::open_at(&path).unwrap();
        let saved = store.save_new(contents("filename canary")).unwrap();
        assert!(!saved.id.contains("filename canary"));
        let file = path.join(format!("{}.json", saved.id));
        let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        let directory_mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(directory_mode, 0o700);
        let bytes = std::fs::read(&file).unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("filename canary"));
        std::fs::remove_dir_all(path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlink_directory_and_draft_file_are_rejected_without_following() {
        use std::os::unix::fs::symlink;
        let parent = test_dir();
        let target = test_dir();
        std::fs::create_dir_all(&parent).unwrap();
        std::fs::create_dir_all(&target).unwrap();
        let link = parent.join("link");
        symlink(&target, &link).unwrap();
        assert!(matches!(
            DraftStore::open_at(&link),
            Err(DraftError::UnsafeDirectory)
        ));

        let store = DraftStore::open_at(&parent.join("drafts")).unwrap();
        let id = "a".repeat(64);
        symlink(
            target.join("victim"),
            parent.join("drafts").join(format!("{id}.json")),
        )
        .unwrap();
        assert!(matches!(store.load(&id), Err(DraftError::UnsafeDirectory)));
        assert!(matches!(
            store.delete(&id, "x"),
            Err(DraftError::UnsafeDirectory)
        ));
        assert!(!target.join("victim").exists());
        std::fs::remove_dir_all(parent).unwrap();
        std::fs::remove_dir_all(target).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn malformed_drafts_are_skipped_and_do_not_crash_library_listing() {
        let path = test_dir();
        let store = DraftStore::open_at(&path).unwrap();
        let id = "b".repeat(64);
        let malformed = DraftFile {
            version: FORMAT_VERSION,
            id: id.clone(),
            title: "Malformed prompt".into(),
            saved_at: "2026-01-01T00:00:00Z".into(),
            prompt: "unsupported\u{1}".into(),
            target_kind: TargetKind::NewThread,
            target_thread_id: None,
            workspace_selected: false,
            model_selection: ModelSelection::Inherit,
            reasoning_selection: ReasoningSelection::Inherit,
        };
        std::fs::write(
            path.join(format!("{id}.json")),
            serde_json::to_vec(&malformed).unwrap(),
        )
        .unwrap();
        let listing = store.list().unwrap();
        assert!(listing.drafts.is_empty());
        assert_eq!(listing.skipped_entries, 1);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn stored_text_never_enters_telemetry_database() {
        let path = test_dir();
        let database = path.with_extension("sqlite3");
        let store = DraftStore::open_at(&path).unwrap();
        store.save_new(contents("privacy canary 92831")).unwrap();
        assert!(!database.exists());
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn draft_limits_reject_invalid_titles_and_large_prompts() {
        let mut value = contents(&"x".repeat(MAX_PROMPT_BYTES + 1));
        assert_eq!(validate_contents(&value), Err(DraftError::InvalidDraft));
        value.prompt = "valid".into();
        value.title = "\n".into();
        assert_eq!(validate_contents(&value), Err(DraftError::InvalidDraft));
    }
}
