//! Bounded, symlink-aware discovery of plain Codex rollout JSONL files.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::codex_rollout::{parse_rollout_record, RolloutRecordKind, COMPATIBILITY_CLI_VERSION};
use super::identity;
use super::incremental_jsonl::SourceIdentity;

const MAX_DIRECTORY_ENTRIES: usize = 50_000;
const MAX_ROLLOUT_FILES: usize = 10_000;
const MAX_ACTIVE_DEPTH: usize = 6;
const MAX_METADATA_LINE_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, Default)]
pub(crate) struct DiscoveryCounts {
    pub compressed_sources_skipped: usize,
    pub incomplete_sources: usize,
    pub unsupported_sources: usize,
    pub unsupported_version_sources: usize,
    pub unreadable_sources: usize,
    pub unreadable_directories: usize,
    pub symlink_entries_skipped: usize,
    pub ambiguous_source_identities: usize,
    examined_entries: usize,
    pub scan_limited: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct DiscoveredRollout {
    pub path: PathBuf,
    pub source: SourceIdentity,
    pub cli_version: String,
    file_identity_sha256: Option<String>,
    priority: u8,
    file_size: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct DiscoveryResult {
    pub codex_home: Option<PathBuf>,
    pub explicit_home_invalid: bool,
    pub roots_present: bool,
    pub source_files_seen: usize,
    pub sessions_seen: usize,
    pub rollouts: Vec<DiscoveredRollout>,
    pub counts: DiscoveryCounts,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HostPlatform {
    Linux,
    MacOs,
    Windows,
    Other,
}

pub(crate) fn discover_local_rollouts(
    explicit_home: Option<PathBuf>,
    codex_home_env: Option<OsString>,
) -> DiscoveryResult {
    let mut result = DiscoveryResult {
        codex_home: None,
        explicit_home_invalid: false,
        roots_present: false,
        source_files_seen: 0,
        sessions_seen: 0,
        rollouts: Vec::new(),
        counts: DiscoveryCounts::default(),
    };

    let selected_home = if let Some(explicit) = explicit_home {
        match usable_directory(&explicit) {
            Some(true) => Some(explicit),
            _ => {
                result.explicit_home_invalid = true;
                return result;
            }
        }
    } else if let Some(environment_path) = codex_home_env
        .map(PathBuf::from)
        .filter(|path| usable_directory(path) == Some(true))
    {
        Some(environment_path)
    } else {
        default_codex_home().filter(|path| usable_directory(path) == Some(true))
    };

    let Some(home) = selected_home else {
        return result;
    };
    result.codex_home = Some(home.clone());

    let mut candidates = Vec::new();
    let sessions = home.join("sessions");
    if is_directory_without_following(&sessions, &mut result.counts) {
        result.roots_present = true;
        let years = read_directory(&sessions, &mut result.counts);
        for year in years {
            let Some(year_name) = year.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if year_name.len() != 4 || !year_name.bytes().all(|byte| byte.is_ascii_digit()) {
                continue;
            }
            let months = read_valid_child_directories(&year, 2, &mut result.counts);
            for month in months {
                let days = read_valid_child_directories(&month, 2, &mut result.counts);
                for day in days {
                    gather_rollout_files(
                        &day,
                        0,
                        MAX_ACTIVE_DEPTH,
                        0,
                        &mut candidates,
                        &mut result.counts,
                    );
                }
            }
        }
    }

    let archived = home.join("archived_sessions");
    if is_directory_without_following(&archived, &mut result.counts) {
        result.roots_present = true;
        for path in read_directory(&archived, &mut result.counts) {
            if candidates.len() >= MAX_ROLLOUT_FILES {
                result.counts.scan_limited = true;
                break;
            }
            if is_regular_file_without_following(&path, &mut result.counts) {
                if let Some(kind) = rollout_kind(&path) {
                    candidates.push((path, kind, 1));
                }
            }
        }
    }
    result.roots_present |= !candidates.is_empty();
    result.source_files_seen = candidates.len();

    let plain_paths: BTreeSet<PathBuf> = candidates
        .iter()
        .filter(|(_, kind, _)| *kind == RolloutFileKind::Plain)
        .map(|(path, _, _)| path.clone())
        .collect();
    let mut metadata: BTreeMap<(String, String), DiscoveredRollout> = BTreeMap::new();
    let mut ambiguous_keys = BTreeSet::new();
    let mut seen_sessions = BTreeSet::new();
    for (path, kind, priority) in candidates {
        match kind {
            RolloutFileKind::Compressed => {
                let plain_sibling = compressed_plain_sibling(&path);
                if !plain_paths.contains(&plain_sibling)
                    && is_regular_file_without_following(&plain_sibling, &mut result.counts)
                {
                    continue;
                }
                if !plain_paths.contains(&plain_sibling) {
                    result.counts.compressed_sources_skipped += 1;
                }
            }
            RolloutFileKind::Plain => {}
        }

        if result.counts.scan_limited {
            break;
        }
        if let Ok(Some(rollout)) =
            inspect_rollout(&path, priority, &mut result.counts, &mut seen_sessions)
        {
            let key = (
                rollout.source.rollout_id().to_owned(),
                rollout.source.source_generation().to_owned(),
            );
            if ambiguous_keys.contains(&key) {
                continue;
            }
            match metadata.get(&key) {
                Some(previous)
                    if previous.file_identity_sha256.is_none()
                        || rollout.file_identity_sha256.is_none()
                        || previous.file_identity_sha256 != rollout.file_identity_sha256 =>
                {
                    metadata.remove(&key);
                    ambiguous_keys.insert(key);
                    result.counts.ambiguous_source_identities += 1;
                }
                Some(previous)
                    if previous.file_size > rollout.file_size
                        || (previous.file_size == rollout.file_size
                            && previous.priority <= rollout.priority) => {}
                _ => {
                    metadata.insert(key, rollout);
                }
            }
        }
    }
    result.sessions_seen = seen_sessions.len();
    result.rollouts = metadata.into_values().collect();
    result.rollouts.sort_by(|left, right| {
        left.source
            .rollout_id()
            .cmp(right.source.rollout_id())
            .then_with(|| {
                left.source
                    .source_generation()
                    .cmp(right.source.source_generation())
            })
    });
    result
}

fn default_codex_home() -> Option<PathBuf> {
    let platform = if cfg!(target_os = "linux") {
        HostPlatform::Linux
    } else if cfg!(target_os = "macos") {
        HostPlatform::MacOs
    } else if cfg!(target_os = "windows") {
        HostPlatform::Windows
    } else {
        HostPlatform::Other
    };
    home_for_platform(
        platform,
        std::env::var_os("HOME").as_deref(),
        std::env::var_os("USERPROFILE").as_deref(),
    )
}

fn home_for_platform(
    platform: HostPlatform,
    home: Option<&std::ffi::OsStr>,
    userprofile: Option<&std::ffi::OsStr>,
) -> Option<PathBuf> {
    let base = match platform {
        HostPlatform::Windows => userprofile.or(home),
        HostPlatform::Linux | HostPlatform::MacOs | HostPlatform::Other => home,
    }?;
    Some(PathBuf::from(base).join(".codex"))
}

fn usable_directory(path: &Path) -> Option<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Some(metadata.file_type().is_dir()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(false),
        Err(_) => None,
    }
}

fn is_directory_without_following(path: &Path, counts: &mut DiscoveryCounts) -> bool {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => true,
        Ok(metadata) if metadata.file_type().is_symlink() => {
            counts.symlink_entries_skipped += 1;
            false
        }
        Ok(_) => false,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => {
            counts.unreadable_directories += 1;
            false
        }
    }
}

fn is_regular_file_without_following(path: &Path, counts: &mut DiscoveryCounts) -> bool {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => true,
        Ok(metadata) if metadata.file_type().is_symlink() => {
            counts.symlink_entries_skipped += 1;
            false
        }
        Ok(_) => false,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => {
            counts.unreadable_sources += 1;
            false
        }
    }
}

fn read_directory(path: &Path, counts: &mut DiscoveryCounts) -> Vec<PathBuf> {
    if counts.scan_limited || !is_directory_without_following(path, counts) {
        return Vec::new();
    }
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(_) => {
            counts.unreadable_directories += 1;
            return Vec::new();
        }
    };
    let mut paths = Vec::new();
    for entry in entries {
        if counts.scan_limited {
            break;
        }
        if counts_examined(counts) {
            break;
        }
        match entry {
            Ok(entry) => {
                let file_type = match entry.file_type() {
                    Ok(file_type) => file_type,
                    Err(_) => {
                        counts.unreadable_sources += 1;
                        continue;
                    }
                };
                if file_type.is_symlink() {
                    counts.symlink_entries_skipped += 1;
                } else {
                    paths.push(entry.path());
                }
            }
            Err(_) => counts.unreadable_sources += 1,
        }
    }
    paths.sort();
    paths
}

fn counts_examined(counts: &mut DiscoveryCounts) -> bool {
    counts.examined_entries += 1;
    if counts.examined_entries >= MAX_DIRECTORY_ENTRIES {
        counts.scan_limited = true;
    }
    counts.scan_limited
}

fn read_valid_child_directories(
    path: &Path,
    component_bytes: usize,
    counts: &mut DiscoveryCounts,
) -> Vec<PathBuf> {
    read_directory(path, counts)
        .into_iter()
        .filter(|child| {
            let Some(name) = child.file_name().and_then(|name| name.to_str()) else {
                return false;
            };
            name.len() == component_bytes
                && name.bytes().all(|byte| byte.is_ascii_digit())
                && is_directory_without_following(child, counts)
        })
        .collect()
}

fn gather_rollout_files(
    directory: &Path,
    depth: usize,
    maximum_depth: usize,
    priority: u8,
    candidates: &mut Vec<(PathBuf, RolloutFileKind, u8)>,
    counts: &mut DiscoveryCounts,
) {
    for path in read_directory(directory, counts) {
        if counts.scan_limited || candidates.len() >= MAX_ROLLOUT_FILES {
            counts.scan_limited = true;
            break;
        }
        if is_regular_file_without_following(&path, counts) {
            if let Some(kind) = rollout_kind(&path) {
                candidates.push((path, kind, priority));
            }
        } else if depth < maximum_depth && is_directory_without_following(&path, counts) {
            gather_rollout_files(
                &path,
                depth + 1,
                maximum_depth,
                priority,
                candidates,
                counts,
            );
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RolloutFileKind {
    Plain,
    Compressed,
}

fn rollout_kind(path: &Path) -> Option<RolloutFileKind> {
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();
    if name.ends_with(".jsonl.zst") {
        Some(RolloutFileKind::Compressed)
    } else if name.ends_with(".jsonl") {
        Some(RolloutFileKind::Plain)
    } else {
        None
    }
}

fn compressed_plain_sibling(path: &Path) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    let suffix = ".zst";
    let mut text = value.to_string_lossy().into_owned();
    if text.to_ascii_lowercase().ends_with(suffix) {
        text.truncate(text.len() - suffix.len());
        value = OsString::from(text);
    }
    PathBuf::from(value)
}

fn inspect_rollout(
    path: &Path,
    priority: u8,
    counts: &mut DiscoveryCounts,
    seen_sessions: &mut BTreeSet<String>,
) -> Result<Option<DiscoveredRollout>, ()> {
    if !is_regular_file_without_following(path, counts) {
        return Err(());
    }
    let mut file = open_regular_file_without_following(path).map_err(|_| {
        counts.unreadable_sources += 1;
    })?;
    let file_size = file
        .metadata()
        .map_err(|_| {
            counts.unreadable_sources += 1;
        })?
        .len();
    let mut bounded = BufReader::new(file.by_ref().take(MAX_METADATA_LINE_BYTES + 1));
    let mut line = Vec::new();
    let bytes_read = bounded.read_until(b'\n', &mut line).map_err(|_| {
        counts.unreadable_sources += 1;
    })?;
    if bytes_read == 0 {
        counts.incomplete_sources += 1;
        return Ok(None);
    }
    if line.len() as u64 > MAX_METADATA_LINE_BYTES {
        counts.unsupported_sources += 1;
        return Ok(None);
    }
    if line.last() != Some(&b'\n') {
        counts.incomplete_sources += 1;
        return Ok(None);
    }
    let record_text = std::str::from_utf8(&line[..line.len() - 1]).map_err(|_| {
        counts.unsupported_sources += 1;
    })?;
    let record = parse_rollout_record(record_text).map_err(|_| {
        counts.unsupported_sources += 1;
    })?;
    let RolloutRecordKind::SessionMeta(meta) = record.kind else {
        counts.unsupported_sources += 1;
        return Ok(None);
    };
    seen_sessions.insert(identity::session_id(&meta.session_id));
    if meta.cli_version != COMPATIBILITY_CLI_VERSION {
        counts.unsupported_version_sources += 1;
        return Ok(None);
    }
    let source = SourceIdentity::new(
        identity::rollout_identity(&meta.session_id),
        identity::source_generation_identity(
            &meta.session_id,
            &meta.thread_id,
            &meta.cli_version,
            &record.timestamp,
        ),
    )
    .map_err(|_| {
        counts.unsupported_sources += 1;
    })?;
    let file_identity_sha256 = file_identity_sha256(&file);
    Ok(Some(DiscoveredRollout {
        path: path.to_path_buf(),
        source,
        cli_version: meta.cli_version,
        file_identity_sha256,
        priority,
        file_size,
    }))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BoundaryCheck {
    Matches,
    MissingFingerprint,
    Changed,
    Truncated,
    Unreadable,
}

pub(crate) fn verify_committed_boundary(
    file: &mut File,
    offset: u64,
    expected_sha256: Option<&str>,
) -> BoundaryCheck {
    if offset == 0 {
        return if expected_sha256.is_none() {
            BoundaryCheck::Matches
        } else {
            BoundaryCheck::Changed
        };
    }
    let Some(expected_sha256) = expected_sha256 else {
        return BoundaryCheck::MissingFingerprint;
    };
    let Ok(metadata) = file.metadata() else {
        return BoundaryCheck::Unreadable;
    };
    if metadata.len() < offset {
        return BoundaryCheck::Truncated;
    }
    // Include one byte before the largest accepted line so a maximum-sized
    // line can still prove its start boundary after earlier source records.
    let search_start = offset.saturating_sub(MAX_METADATA_LINE_BYTES + 1);
    let Ok(window_size) = usize::try_from(offset - search_start) else {
        return BoundaryCheck::Unreadable;
    };
    let mut tail = vec![0_u8; window_size];
    if file.seek(SeekFrom::Start(search_start)).is_err() || file.read_exact(&mut tail).is_err() {
        return BoundaryCheck::Unreadable;
    }
    if tail.last() != Some(&b'\n') {
        return BoundaryCheck::Changed;
    }
    let start_in_window = tail[..tail.len() - 1]
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |position| position + 1);
    if start_in_window == 0 && search_start > 0 {
        return BoundaryCheck::Changed;
    }
    let actual = Sha256::digest(&tail[start_in_window..])
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if actual == expected_sha256 {
        BoundaryCheck::Matches
    } else {
        BoundaryCheck::Changed
    }
}

pub(crate) fn file_identity_sha256(file: &File) -> Option<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = file.metadata().ok()?;
        let identity = format!("{}:{}", metadata.dev(), metadata.ino());
        Some(
            Sha256::digest(identity.as_bytes())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
        )
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        let metadata = file.metadata().ok()?;
        let volume = metadata.volume_serial_number()?;
        let index = metadata.file_index()?;
        let identity = format!("{volume}:{index}");
        Some(
            Sha256::digest(identity.as_bytes())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
        )
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        None
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SourceOpenError {
    Unreadable,
    IdentityChanged,
}

pub(crate) fn open_verified_rollout(rollout: &DiscoveredRollout) -> Result<File, SourceOpenError> {
    let mut file = open_regular_file_without_following(&rollout.path)
        .map_err(|_| SourceOpenError::Unreadable)?;
    let mut bounded = BufReader::new(
        file.by_ref()
            .take(MAX_METADATA_LINE_BYTES.saturating_add(1)),
    );
    let mut line = Vec::new();
    let read = bounded
        .read_until(b'\n', &mut line)
        .map_err(|_| SourceOpenError::Unreadable)?;
    if read == 0 || line.len() as u64 > MAX_METADATA_LINE_BYTES || line.last() != Some(&b'\n') {
        return Err(SourceOpenError::IdentityChanged);
    }
    let text = std::str::from_utf8(&line[..line.len() - 1])
        .map_err(|_| SourceOpenError::IdentityChanged)?;
    let record = parse_rollout_record(text).map_err(|_| SourceOpenError::IdentityChanged)?;
    let RolloutRecordKind::SessionMeta(meta) = record.kind else {
        return Err(SourceOpenError::IdentityChanged);
    };
    let source = SourceIdentity::new(
        identity::rollout_identity(&meta.session_id),
        identity::source_generation_identity(
            &meta.session_id,
            &meta.thread_id,
            &meta.cli_version,
            &record.timestamp,
        ),
    )
    .map_err(|_| SourceOpenError::IdentityChanged)?;
    if source != rollout.source || meta.cli_version != rollout.cli_version {
        return Err(SourceOpenError::IdentityChanged);
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|_| SourceOpenError::Unreadable)?;
    Ok(file)
}

#[cfg(unix)]
fn open_regular_file_without_following(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.file_type().is_file() {
        return Err(std::io::Error::other("not a regular file"));
    }
    Ok(file)
}

#[cfg(not(unix))]
fn open_regular_file_without_following(path: &Path) -> std::io::Result<File> {
    let before = fs::symlink_metadata(path)?;
    if !before.file_type().is_file() {
        return Err(std::io::Error::other("not a regular file"));
    }
    let file = File::open(path)?;
    let after = fs::symlink_metadata(path)?;
    if !after.file_type().is_file() {
        return Err(std::io::Error::other("not a regular file"));
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_BOUNDARY_TEST: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn default_home_is_platform_appropriate_and_does_not_embed_a_username() {
        assert_eq!(
            home_for_platform(
                HostPlatform::Linux,
                Some(Path::new("/home/alice").as_os_str()),
                None
            ),
            Some(PathBuf::from("/home/alice/.codex"))
        );
        assert_eq!(
            home_for_platform(
                HostPlatform::Windows,
                None,
                Some(Path::new("C:/Users/Alice").as_os_str())
            ),
            Some(PathBuf::from("C:/Users/Alice/.codex"))
        );
        assert_eq!(home_for_platform(HostPlatform::Linux, None, None), None);
    }

    #[test]
    fn committed_boundary_accepts_a_maximum_sized_line_after_prior_records() {
        let path = std::env::temp_dir().join(format!(
            "codex-meter-boundary-{}-{}.jsonl",
            std::process::id(),
            NEXT_BOUNDARY_TEST.fetch_add(1, Ordering::Relaxed)
        ));
        let mut contents = b"prior\n".to_vec();
        let line_start = contents.len();
        contents.resize(line_start + MAX_METADATA_LINE_BYTES as usize - 1, b'x');
        contents.push(b'\n');
        let expected_hash = Sha256::digest(&contents[line_start..])
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let mut file = File::create(&path).expect("boundary test source should be created");
        file.write_all(&contents)
            .expect("boundary test source should be written");
        drop(file);
        let mut file = File::open(&path).expect("boundary test source should open");
        assert_eq!(
            verify_committed_boundary(&mut file, contents.len() as u64, Some(&expected_hash)),
            BoundaryCheck::Matches
        );
        drop(file);
        fs::remove_file(path).expect("boundary test source should be removed");
    }
}
