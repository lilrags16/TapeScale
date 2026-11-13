//! Bounded, read-only browsing for live sources and retained backup manifests.
//!
//! Live browsing never follows a symbolic link and never exposes the source
//! root's absolute container path. Retained browsing uses only the durable,
//! content-addressed source manifest; it does not read the original source or
//! imply that path-selective restore execution is available.

use std::{
    cmp::Ordering,
    fs,
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use rustix::{
    fd::OwnedFd,
    fs::{open, openat, openat2, statat, AtFlags, Dir, FileType, Mode, OFlags, ResolveFlags, Stat},
    io::Errno,
};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    data_path::{load_manifest_bytes, verify_read_only_mount, EntryKind, FileManifest},
    domain::path::validate_archive_path,
};

pub const MAX_BROWSER_ENTRIES: usize = 2_000;
const MAX_BROWSER_PATH_BYTES: usize = 4_096;
const MAX_MANIFEST_ARTIFACT_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum SourceBrowserError {
    #[error("browser path must be empty or a contained relative path")]
    InvalidPath,
    #[error("browser path exceeds {MAX_BROWSER_PATH_BYTES} bytes")]
    PathTooLong,
    #[error("browser path was not found")]
    NotFound,
    #[error("browser path is not a directory")]
    NotDirectory,
    #[error("source browser does not follow symbolic links")]
    SymlinkTraversal,
    #[error("source root is not available")]
    InvalidRoot,
    #[error("directory contains a name that is not valid UTF-8")]
    NonUtf8Name,
    #[error("directory contains a path that cannot be represented safely")]
    UnsafeEntryName,
    #[error("directory contains more than {limit} entries")]
    TooManyEntries { limit: usize },
    #[error("retained manifest is not available to the API")]
    ManifestUnavailable,
    #[error("retained manifest is outside the configured evidence root")]
    ManifestOutsideRoot,
    #[error("retained manifest catalog binding is invalid")]
    InvalidManifestBinding,
    #[error("retained manifest failed its content-addressed integrity check")]
    ManifestIntegrity,
    #[error("directory or retained manifest could not be read")]
    Io(#[source] std::io::Error),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserEntryKind {
    Directory,
    File,
    Symlink,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BrowserEntry {
    pub name: String,
    pub path: String,
    pub kind: BrowserEntryKind,
    pub size_bytes: Option<u64>,
    pub modified_at: Option<DateTime<Utc>>,
    pub read_only: bool,
    pub symlink: bool,
    pub snapshot: bool,
    pub navigable: bool,
    pub selectable: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BrowserBreadcrumb {
    pub name: String,
    pub path: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BrowserListing {
    pub current_path: String,
    pub parent_path: Option<String>,
    pub breadcrumbs: Vec<BrowserBreadcrumb>,
    pub read_only: bool,
    pub snapshot: bool,
    pub selectable: bool,
    pub entries: Vec<BrowserEntry>,
    pub truncated: bool,
    pub entry_limit: usize,
}

#[derive(Clone, Debug)]
pub struct RetainedManifestReference {
    pub storage_ref: String,
    pub artifact_sha256: String,
    pub artifact_size_bytes: i64,
    pub manifest_digest: String,
}

pub fn browse_source(
    configured_root: &Path,
    requested_path: &str,
) -> Result<BrowserListing, SourceBrowserError> {
    browse_source_with_limit(configured_root, requested_path, MAX_BROWSER_ENTRIES)
}

pub fn browse_retained_manifest(
    configured_evidence_root: &Path,
    reference: &RetainedManifestReference,
    requested_path: &str,
) -> Result<BrowserListing, SourceBrowserError> {
    let manifest = load_retained_manifest(configured_evidence_root, reference)?;
    browse_manifest_with_limit(&manifest, requested_path, MAX_BROWSER_ENTRIES)
}

fn browse_source_with_limit(
    configured_root: &Path,
    requested_path: &str,
    entry_limit: usize,
) -> Result<BrowserListing, SourceBrowserError> {
    let (relative, current_path) = normalized_browser_path(requested_path)?;
    let root = open_source_root(configured_root)?;
    let directory = open_source_directory(&root, &relative)?;
    let read_only = read_only_directory(&directory);
    let entries = live_entries_from_directory(&directory, &relative, read_only, entry_limit)?;

    let snapshot = is_snapshot_path(&relative);
    Ok(BrowserListing {
        parent_path: relative.parent().map(portable_path).transpose()?,
        breadcrumbs: breadcrumbs(&relative)?,
        selectable: !relative.as_os_str().is_empty() && read_only,
        current_path,
        read_only,
        snapshot,
        entries,
        // Oversized directories fail instead of returning a partial view.
        truncated: false,
        entry_limit,
    })
}

fn open_source_root(configured_root: &Path) -> Result<OwnedFd, SourceBrowserError> {
    open(
        configured_root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| SourceBrowserError::InvalidRoot)
}

fn open_source_directory(root: &OwnedFd, relative: &Path) -> Result<OwnedFd, SourceBrowserError> {
    let requested = if relative.as_os_str().is_empty() {
        Path::new(".")
    } else {
        relative
    };
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    match openat2(
        root,
        requested,
        flags,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
    ) {
        Ok(directory) => Ok(directory),
        // Linux before 5.6 has no openat2. Some older container seccomp
        // profiles return EPERM for the unknown syscall. Walking stable
        // directory descriptors with O_NOFOLLOW preserves the same boundary.
        Err(Errno::NOSYS | Errno::PERM) => open_source_directory_fallback(root, relative),
        // O_DIRECTORY can report ENOTDIR for both a regular file and a
        // symlink. The descriptor walk classifies those without following it.
        Err(Errno::LOOP | Errno::NOTDIR) => open_source_directory_fallback(root, relative),
        Err(error) => Err(map_rustix_lookup(error)),
    }
}

fn open_source_directory_fallback(
    root: &OwnedFd,
    relative: &Path,
) -> Result<OwnedFd, SourceBrowserError> {
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut current = openat(root, ".", flags, Mode::empty()).map_err(map_rustix_lookup)?;
    for component in relative.components() {
        let std::path::Component::Normal(component) = component else {
            return Err(SourceBrowserError::InvalidPath);
        };
        let metadata =
            statat(&current, component, AtFlags::SYMLINK_NOFOLLOW).map_err(map_rustix_lookup)?;
        let file_type = FileType::from_raw_mode(metadata.st_mode);
        if file_type.is_symlink() {
            return Err(SourceBrowserError::SymlinkTraversal);
        }
        if !file_type.is_dir() {
            return Err(SourceBrowserError::NotDirectory);
        }
        current = openat(&current, component, flags, Mode::empty()).map_err(map_rustix_lookup)?;
    }
    Ok(current)
}

fn read_only_directory(directory: &OwnedFd) -> bool {
    let descriptor_path = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
    verify_read_only_mount(&descriptor_path).is_ok()
}

fn live_entries_from_directory(
    directory: &OwnedFd,
    parent: &Path,
    read_only: bool,
    entry_limit: usize,
) -> Result<Vec<BrowserEntry>, SourceBrowserError> {
    let mut entries = Vec::new();
    let mut saw_hidden_zfs = false;
    let reader = Dir::read_from(directory).map_err(map_rustix_io)?;
    for item in reader {
        let item = item.map_err(map_rustix_io)?;
        let raw_name = item.file_name().to_bytes();
        if raw_name == b"." || raw_name == b".." {
            continue;
        }
        if entries.len() == entry_limit {
            return Err(SourceBrowserError::TooManyEntries { limit: entry_limit });
        }
        let name = std::str::from_utf8(raw_name)
            .map_err(|_| SourceBrowserError::NonUtf8Name)?
            .to_owned();
        saw_hidden_zfs |= name == ".zfs";
        let metadata = statat(
            directory,
            item.file_name(),
            AtFlags::SYMLINK_NOFOLLOW | AtFlags::NO_AUTOMOUNT,
        )
        .map_err(map_rustix_io)?;
        entries.push(live_entry_from_stat(parent, &name, &metadata, read_only)?);
    }

    // TrueNAS can make `<dataset>/.zfs` directly accessible while omitting it
    // from readdir when the ZFS snapdir property is `hidden`. The descriptor-
    // relative lookup cannot escape even if the host renames the dataset.
    if !saw_hidden_zfs {
        if let Ok(metadata) = statat(
            directory,
            ".zfs",
            AtFlags::SYMLINK_NOFOLLOW | AtFlags::NO_AUTOMOUNT,
        ) {
            if FileType::from_raw_mode(metadata.st_mode).is_dir() {
                if entries.len() == entry_limit {
                    return Err(SourceBrowserError::TooManyEntries { limit: entry_limit });
                }
                entries.push(live_entry_from_stat(parent, ".zfs", &metadata, read_only)?);
            }
        }
    }
    entries.sort_by(compare_entries);
    Ok(entries)
}

fn live_entry_from_stat(
    parent: &Path,
    name: &str,
    metadata: &Stat,
    read_only: bool,
) -> Result<BrowserEntry, SourceBrowserError> {
    if name.chars().any(char::is_control) {
        return Err(SourceBrowserError::UnsafeEntryName);
    }
    let child_relative = if parent.as_os_str().is_empty() {
        PathBuf::from(name)
    } else {
        parent.join(name)
    };
    let child_safe =
        validate_archive_path(&child_relative).map_err(|_| SourceBrowserError::UnsafeEntryName)?;
    let child_path = portable_path(child_safe.as_path())?;
    if child_path.len() > MAX_BROWSER_PATH_BYTES {
        return Err(SourceBrowserError::PathTooLong);
    }
    let file_type = FileType::from_raw_mode(metadata.st_mode);
    let kind = if file_type.is_symlink() {
        BrowserEntryKind::Symlink
    } else if file_type.is_dir() {
        BrowserEntryKind::Directory
    } else if file_type.is_file() {
        BrowserEntryKind::File
    } else {
        BrowserEntryKind::Other
    };
    let directory = kind == BrowserEntryKind::Directory;
    Ok(BrowserEntry {
        name: name.to_owned(),
        path: child_path,
        kind,
        size_bytes: (kind == BrowserEntryKind::File)
            .then(|| u64::try_from(metadata.st_size).ok())
            .flatten(),
        modified_at: stat_modified_at(metadata),
        read_only,
        symlink: kind == BrowserEntryKind::Symlink,
        snapshot: is_snapshot_path(child_safe.as_path()),
        navigable: directory,
        selectable: directory && read_only,
    })
}

fn load_retained_manifest(
    configured_evidence_root: &Path,
    reference: &RetainedManifestReference,
) -> Result<FileManifest, SourceBrowserError> {
    if reference.artifact_size_bytes <= 0
        || u64::try_from(reference.artifact_size_bytes)
            .ok()
            .is_none_or(|size| size > MAX_MANIFEST_ARTIFACT_BYTES)
        || reference.artifact_sha256.len() != 64
        || !reference
            .artifact_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || reference.manifest_digest.len() != 64
        || !reference
            .manifest_digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || reference.storage_ref.chars().any(char::is_control)
    {
        return Err(SourceBrowserError::InvalidManifestBinding);
    }
    let root = configured_evidence_root
        .canonicalize()
        .map_err(|_| SourceBrowserError::ManifestUnavailable)?;
    if !root.is_dir() {
        return Err(SourceBrowserError::ManifestUnavailable);
    }
    let recorded = Path::new(&reference.storage_ref);
    let relative = recorded
        .strip_prefix(configured_evidence_root)
        .or_else(|_| recorded.strip_prefix(&root))
        .map_err(|_| SourceBrowserError::ManifestOutsideRoot)?;
    if relative.as_os_str().is_empty()
        || relative.file_name().and_then(|name| name.to_str()) != Some("source-manifest.json")
    {
        return Err(SourceBrowserError::InvalidManifestBinding);
    }
    let safe_relative =
        validate_archive_path(relative).map_err(|_| SourceBrowserError::ManifestOutsideRoot)?;
    let manifest_path = resolve_file_without_symlinks(&root, safe_relative.as_path())?;
    let before = manifest_path
        .symlink_metadata()
        .map_err(|_| SourceBrowserError::ManifestUnavailable)?;
    let expected_size = u64::try_from(reference.artifact_size_bytes)
        .map_err(|_| SourceBrowserError::InvalidManifestBinding)?;
    if !before.is_file() || before.len() != expected_size {
        return Err(SourceBrowserError::ManifestIntegrity);
    }
    // Read once. The artifact digest and manifest validation below must apply to
    // this exact byte snapshot; reopening the pathname would introduce a race.
    let bytes = fs::read(&manifest_path).map_err(|_| SourceBrowserError::ManifestUnavailable)?;
    if u64::try_from(bytes.len()).ok() != Some(expected_size)
        || format!("{:x}", Sha256::digest(&bytes)) != reference.artifact_sha256
    {
        return Err(SourceBrowserError::ManifestIntegrity);
    }
    let manifest =
        load_manifest_bytes(&bytes).map_err(|_| SourceBrowserError::ManifestIntegrity)?;
    let after = manifest_path
        .symlink_metadata()
        .map_err(|_| SourceBrowserError::ManifestUnavailable)?;
    if metadata_identity(&before) != metadata_identity(&after)
        || manifest.digest_sha256 != reference.manifest_digest
    {
        return Err(SourceBrowserError::ManifestIntegrity);
    }
    validate_manifest_paths(&manifest)?;
    Ok(manifest)
}

fn browse_manifest_with_limit(
    manifest: &FileManifest,
    requested_path: &str,
    entry_limit: usize,
) -> Result<BrowserListing, SourceBrowserError> {
    let (relative, current_path) = normalized_browser_path(requested_path)?;
    if !relative.as_os_str().is_empty()
        && !manifest
            .entries
            .iter()
            .any(|entry| entry.kind == EntryKind::Directory && entry.path == current_path)
    {
        return Err(SourceBrowserError::NotDirectory);
    }

    let mut entries = Vec::new();
    for entry in &manifest.entries {
        if entry.path == "." {
            continue;
        }
        let path = Path::new(&entry.path);
        let parent = path.parent().unwrap_or_else(|| Path::new(""));
        if parent != relative {
            continue;
        }
        if entries.len() == entry_limit {
            return Err(SourceBrowserError::TooManyEntries { limit: entry_limit });
        }
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(SourceBrowserError::NonUtf8Name)?
            .to_owned();
        let kind = match entry.kind {
            EntryKind::Directory => BrowserEntryKind::Directory,
            EntryKind::File => BrowserEntryKind::File,
            EntryKind::Symlink => BrowserEntryKind::Symlink,
        };
        entries.push(BrowserEntry {
            name,
            path: entry.path.clone(),
            kind,
            size_bytes: entry.size,
            modified_at: DateTime::<Utc>::from_timestamp(entry.mtime_seconds, 0),
            read_only: true,
            symlink: kind == BrowserEntryKind::Symlink,
            snapshot: false,
            navigable: kind == BrowserEntryKind::Directory,
            // Browsing retained contents is not path-selective restore authority.
            selectable: false,
        });
    }
    entries.sort_by(compare_entries);
    Ok(BrowserListing {
        parent_path: relative.parent().map(portable_path).transpose()?,
        breadcrumbs: breadcrumbs(&relative)?,
        current_path,
        read_only: true,
        snapshot: false,
        selectable: false,
        entries,
        truncated: false,
        entry_limit,
    })
}

fn validate_manifest_paths(manifest: &FileManifest) -> Result<(), SourceBrowserError> {
    let mut root_seen = false;
    let mut previous: Option<&str> = None;
    for entry in &manifest.entries {
        if previous.is_some_and(|path| path >= entry.path.as_str()) {
            return Err(SourceBrowserError::ManifestIntegrity);
        }
        previous = Some(&entry.path);
        if entry.path == "." {
            if root_seen || entry.kind != EntryKind::Directory {
                return Err(SourceBrowserError::ManifestIntegrity);
            }
            root_seen = true;
            continue;
        }
        if entry.path.chars().any(char::is_control) {
            return Err(SourceBrowserError::ManifestIntegrity);
        }
        validate_archive_path(Path::new(&entry.path))
            .map_err(|_| SourceBrowserError::ManifestIntegrity)?;
    }
    if root_seen {
        Ok(())
    } else {
        Err(SourceBrowserError::ManifestIntegrity)
    }
}

fn normalized_browser_path(requested_path: &str) -> Result<(PathBuf, String), SourceBrowserError> {
    if requested_path.len() > MAX_BROWSER_PATH_BYTES {
        return Err(SourceBrowserError::PathTooLong);
    }
    if requested_path.chars().any(char::is_control) {
        return Err(SourceBrowserError::InvalidPath);
    }
    if requested_path.is_empty() {
        return Ok((PathBuf::new(), String::new()));
    }
    let safe = validate_archive_path(Path::new(requested_path))
        .map_err(|_| SourceBrowserError::InvalidPath)?;
    let normalized = safe.into_path_buf();
    let portable = portable_path(&normalized)?;
    Ok((normalized, portable))
}

fn resolve_file_without_symlinks(
    root: &Path,
    relative: &Path,
) -> Result<PathBuf, SourceBrowserError> {
    let candidate = resolve_without_symlinks(root, relative)?;
    if candidate.is_file() {
        Ok(candidate)
    } else {
        Err(SourceBrowserError::ManifestUnavailable)
    }
}

fn resolve_without_symlinks(root: &Path, relative: &Path) -> Result<PathBuf, SourceBrowserError> {
    let mut candidate = root.to_owned();
    for component in relative.components() {
        let std::path::Component::Normal(component) = component else {
            return Err(SourceBrowserError::InvalidPath);
        };
        candidate.push(component);
        let metadata = candidate.symlink_metadata().map_err(map_lookup_io)?;
        if metadata.file_type().is_symlink() {
            return Err(SourceBrowserError::SymlinkTraversal);
        }
    }
    let canonical = candidate.canonicalize().map_err(map_lookup_io)?;
    if !canonical.starts_with(root) {
        return Err(SourceBrowserError::ManifestOutsideRoot);
    }
    Ok(canonical)
}

fn breadcrumbs(relative: &Path) -> Result<Vec<BrowserBreadcrumb>, SourceBrowserError> {
    let mut result = vec![BrowserBreadcrumb {
        name: "Source".to_owned(),
        path: String::new(),
    }];
    let mut path = PathBuf::new();
    for component in relative.components() {
        let std::path::Component::Normal(name) = component else {
            return Err(SourceBrowserError::InvalidPath);
        };
        path.push(name);
        result.push(BrowserBreadcrumb {
            name: name
                .to_str()
                .ok_or(SourceBrowserError::NonUtf8Name)?
                .to_owned(),
            path: portable_path(&path)?,
        });
    }
    Ok(result)
}

fn portable_path(path: &Path) -> Result<String, SourceBrowserError> {
    let mut result = String::new();
    for component in path.components() {
        let std::path::Component::Normal(component) = component else {
            return Err(SourceBrowserError::InvalidPath);
        };
        if !result.is_empty() {
            result.push('/');
        }
        result.push_str(component.to_str().ok_or(SourceBrowserError::NonUtf8Name)?);
    }
    Ok(result)
}

fn is_snapshot_path(path: &Path) -> bool {
    let components = path
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>();
    components
        .windows(3)
        .any(|window| window[0] == ".zfs" && window[1] == "snapshot")
}

fn compare_entries(left: &BrowserEntry, right: &BrowserEntry) -> Ordering {
    entry_rank(left.kind)
        .cmp(&entry_rank(right.kind))
        .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
        .then_with(|| left.name.cmp(&right.name))
}

const fn entry_rank(kind: BrowserEntryKind) -> u8 {
    match kind {
        BrowserEntryKind::Directory => 0,
        BrowserEntryKind::File => 1,
        BrowserEntryKind::Symlink => 2,
        BrowserEntryKind::Other => 3,
    }
}

fn stat_modified_at(metadata: &Stat) -> Option<DateTime<Utc>> {
    #[allow(clippy::useless_conversion)]
    let seconds = i64::try_from(metadata.st_mtime).ok()?;
    #[allow(clippy::useless_conversion)]
    let nanoseconds = u32::try_from(metadata.st_mtime_nsec).ok()?;
    DateTime::<Utc>::from_timestamp(seconds, nanoseconds)
}

fn metadata_identity(metadata: &fs::Metadata) -> (u64, u64, u64, i64, i64, i64, i64) {
    (
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec(),
    )
}

fn map_lookup_io(error: std::io::Error) -> SourceBrowserError {
    match error.kind() {
        std::io::ErrorKind::NotFound => SourceBrowserError::NotFound,
        _ => SourceBrowserError::Io(error),
    }
}

fn map_rustix_lookup(error: Errno) -> SourceBrowserError {
    match error {
        Errno::NOENT => SourceBrowserError::NotFound,
        Errno::NOTDIR => SourceBrowserError::NotDirectory,
        Errno::LOOP | Errno::XDEV => SourceBrowserError::SymlinkTraversal,
        _ => map_rustix_io(error),
    }
}

fn map_rustix_io(error: Errno) -> SourceBrowserError {
    SourceBrowserError::Io(error.into())
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::symlink};

    use tempfile::tempdir;

    use crate::data_path::{build_manifest, persist_manifest};

    use super::*;

    #[test]
    fn lists_directories_files_and_breadcrumbs_without_absolute_paths() {
        let root = tempdir().expect("source root");
        fs::create_dir_all(root.path().join("pool/snapshot/photos")).expect("directories");
        fs::write(root.path().join("pool/snapshot/image.raw"), b"raw-data").expect("source file");

        let listing = browse_source(root.path(), "pool/snapshot").expect("listing");
        assert_eq!(listing.current_path, "pool/snapshot");
        assert_eq!(listing.parent_path.as_deref(), Some("pool"));
        assert_eq!(listing.entries.len(), 2);
        assert_eq!(listing.entries[0].name, "photos");
        assert_eq!(listing.entries[0].kind, BrowserEntryKind::Directory);
        assert!(listing.entries[0].navigable);
        assert_eq!(listing.entries[1].name, "image.raw");
        assert_eq!(listing.entries[1].size_bytes, Some(8));
        assert!(!listing.entries[1].selectable);
        assert!(listing.entries.iter().all(|entry| !entry
            .path
            .starts_with(root.path().to_string_lossy().as_ref())));
    }

    #[test]
    fn root_is_browsable_but_not_a_backup_selection() {
        let root = tempdir().expect("source root");
        fs::create_dir(root.path().join("dataset")).expect("dataset");
        let listing = browse_source(root.path(), "").expect("root listing");
        assert_eq!(listing.current_path, "");
        assert_eq!(listing.parent_path, None);
        assert!(!listing.selectable);
        assert_eq!(listing.breadcrumbs.len(), 1);
    }

    #[test]
    fn synthesizes_hidden_zfs_and_marks_snapshot_context() {
        let root = tempdir().expect("source root");
        fs::create_dir_all(root.path().join("dataset/.zfs/snapshot/nightly/photos"))
            .expect("snapshot tree");
        let dataset = browse_source(root.path(), "dataset").expect("dataset listing");
        assert!(dataset.entries.iter().any(|entry| entry.name == ".zfs"));
        let snapshot =
            browse_source(root.path(), "dataset/.zfs/snapshot/nightly").expect("snapshot listing");
        assert!(snapshot.snapshot);
        assert!(snapshot.entries[0].snapshot);
    }

    #[test]
    fn rejects_traversal_files_and_symlink_navigation() {
        let root = tempdir().expect("source root");
        let outside = tempdir().expect("outside");
        fs::write(root.path().join("file"), b"data").expect("file");
        symlink(outside.path(), root.path().join("escape")).expect("escape link");

        assert!(matches!(
            browse_source(root.path(), "../outside"),
            Err(SourceBrowserError::InvalidPath)
        ));
        assert!(matches!(
            browse_source(root.path(), "file"),
            Err(SourceBrowserError::NotDirectory)
        ));
        assert!(matches!(
            browse_source(root.path(), "escape"),
            Err(SourceBrowserError::SymlinkTraversal)
        ));
        let listing = browse_source(root.path(), "").expect("safe root listing");
        let link = listing
            .entries
            .iter()
            .find(|entry| entry.name == "escape")
            .expect("listed link");
        assert!(link.symlink);
        assert!(!link.navigable);
        assert!(!link.selectable);
    }

    #[test]
    fn held_directory_descriptor_cannot_be_redirected_by_path_swap() {
        let root = tempdir().expect("source root");
        let outside = tempdir().expect("outside root");
        fs::create_dir(root.path().join("selected")).expect("selected directory");
        fs::write(root.path().join("selected/original.txt"), b"inside").expect("inside file");
        fs::write(outside.path().join("secret.txt"), b"outside").expect("outside file");

        let root_descriptor = open_source_root(root.path()).expect("source descriptor");
        let selected = Path::new("selected");
        let selected_descriptor =
            open_source_directory(&root_descriptor, selected).expect("selected descriptor");

        fs::rename(root.path().join("selected"), root.path().join("moved"))
            .expect("selected directory should move");
        symlink(outside.path(), root.path().join("selected"))
            .expect("replacement link should exist");

        let entries = live_entries_from_directory(&selected_descriptor, selected, false, 10)
            .expect("the held descriptor should remain listable");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "original.txt");
        assert!(entries.iter().all(|entry| entry.name != "secret.txt"));
        assert!(matches!(
            browse_source(root.path(), "selected"),
            Err(SourceBrowserError::SymlinkTraversal)
        ));
    }

    #[test]
    fn special_objects_are_visible_but_never_navigable_or_selectable() {
        let root = tempdir().expect("source root");
        rustix::fs::mkfifoat(
            rustix::fs::CWD,
            root.path().join("stream.fifo"),
            Mode::RUSR | Mode::WUSR,
        )
        .expect("FIFO should be created");

        let listing = browse_source(root.path(), "").expect("root listing");
        let fifo = listing
            .entries
            .iter()
            .find(|entry| entry.name == "stream.fifo")
            .expect("FIFO should be listed");
        assert_eq!(fifo.kind, BrowserEntryKind::Other);
        assert_eq!(fifo.size_bytes, None);
        assert!(!fifo.navigable);
        assert!(!fifo.selectable);
    }

    #[test]
    fn oversized_directory_fails_instead_of_returning_a_partial_view() {
        let root = tempdir().expect("source root");
        for name in ["one", "two", "three"] {
            fs::write(root.path().join(name), b"").expect("entry");
        }
        assert!(matches!(
            browse_source_with_limit(root.path(), "", 2),
            Err(SourceBrowserError::TooManyEntries { limit: 2 })
        ));
    }

    #[test]
    fn control_characters_fail_closed_in_live_and_retained_names() {
        let source = tempdir().expect("source root");
        fs::write(source.path().join("bad\nname"), b"data").expect("control name");
        assert!(matches!(
            browse_source(source.path(), ""),
            Err(SourceBrowserError::UnsafeEntryName)
        ));
        assert!(matches!(
            browse_source(source.path(), "bad\nname"),
            Err(SourceBrowserError::InvalidPath)
        ));

        let manifest = build_manifest(source.path()).expect("manifest should retain source facts");
        let evidence = tempdir().expect("evidence");
        let artifact = persist_manifest(&evidence.path().join("run"), "source-manifest", &manifest)
            .expect("artifact");
        let reference = RetainedManifestReference {
            storage_ref: artifact.storage_path.to_string_lossy().into_owned(),
            artifact_sha256: artifact.artifact_sha256,
            artifact_size_bytes: i64::try_from(artifact.size_bytes).expect("small artifact"),
            manifest_digest: artifact.manifest_digest,
        };
        assert!(matches!(
            browse_retained_manifest(evidence.path(), &reference, ""),
            Err(SourceBrowserError::ManifestIntegrity)
        ));
    }

    #[test]
    fn retained_manifest_browses_one_level_and_rejects_tampering() {
        let source = tempdir().expect("source");
        fs::create_dir(source.path().join("photos")).expect("photos");
        fs::write(source.path().join("photos/image.raw"), b"image").expect("image");
        fs::write(source.path().join("notes.txt"), b"notes").expect("notes");
        let manifest = build_manifest(source.path()).expect("manifest");
        let evidence = tempdir().expect("evidence");
        let run = evidence.path().join("run");
        let artifact = persist_manifest(&run, "source-manifest", &manifest).expect("artifact");
        let reference = RetainedManifestReference {
            storage_ref: artifact.storage_path.to_string_lossy().into_owned(),
            artifact_sha256: artifact.artifact_sha256,
            artifact_size_bytes: i64::try_from(artifact.size_bytes).expect("small artifact"),
            manifest_digest: artifact.manifest_digest,
        };

        let root = browse_retained_manifest(evidence.path(), &reference, "").expect("root");
        assert_eq!(root.entries[0].name, "photos");
        assert_eq!(root.entries[1].name, "notes.txt");
        assert!(root.entries.iter().all(|entry| !entry.selectable));
        let photos =
            browse_retained_manifest(evidence.path(), &reference, "photos").expect("photos");
        assert_eq!(photos.entries.len(), 1);
        assert_eq!(photos.entries[0].name, "image.raw");

        fs::write(&reference.storage_ref, b"{}").expect("tamper");
        assert!(matches!(
            browse_retained_manifest(evidence.path(), &reference, ""),
            Err(SourceBrowserError::ManifestIntegrity)
        ));
    }
}
