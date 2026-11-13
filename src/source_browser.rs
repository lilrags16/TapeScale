//! Bounded, read-only browsing for live sources and retained backup manifests.
//!
//! Live browsing never follows a symbolic link and never exposes the source
//! root's absolute container path. Retained browsing uses only the durable,
//! content-addressed source manifest; it does not read the original source or
//! imply that path-selective restore execution is available.

use std::{
    cmp::Ordering,
    collections::BTreeMap,
    fs,
    io::{BufReader, Read},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use rustix::{
    fd::OwnedFd,
    fs::{open, openat, openat2, statat, AtFlags, Dir, FileType, Mode, OFlags, ResolveFlags, Stat},
    io::Errno,
};
use serde::{
    de::{self, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor},
    Serialize,
};
use sha2::{Digest, Sha256};

use crate::{
    data_path::{descriptor_is_on_read_only_mount, EntryKind, ManifestCoverage, ManifestEntry},
    domain::path::validate_archive_path,
};

pub const MAX_BROWSER_ENTRIES: usize = 2_000;
const MAX_BROWSER_PATH_BYTES: usize = 4_096;
const RETAINED_MANIFEST_FORMAT: &str = "tapescale-file-manifest-v2";

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
    browse_retained_manifest_with_limit(
        configured_evidence_root,
        reference,
        requested_path,
        MAX_BROWSER_ENTRIES,
    )
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
    let (entries, truncated) =
        live_entries_from_directory(&directory, &relative, read_only, entry_limit)?;

    let snapshot = is_snapshot_path(&relative);
    Ok(BrowserListing {
        parent_path: relative.parent().map(portable_path).transpose()?,
        breadcrumbs: breadcrumbs(&relative)?,
        // Browsing is selection assistance, not the write-safety authority.
        // Let the operator choose any contained non-root directory; plan
        // creation independently opens the path and proves its mount is
        // read-only before persisting it.
        selectable: !relative.as_os_str().is_empty(),
        current_path,
        read_only,
        snapshot,
        entries,
        truncated,
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
    descriptor_is_on_read_only_mount(directory).unwrap_or(false)
}

fn live_entries_from_directory(
    directory: &OwnedFd,
    parent: &Path,
    read_only: bool,
    entry_limit: usize,
) -> Result<(Vec<BrowserEntry>, bool), SourceBrowserError> {
    let mut entries = BTreeMap::new();
    let mut direct_entry_count = 0_usize;
    let mut saw_hidden_zfs = false;
    let reader = Dir::read_from(directory).map_err(map_rustix_io)?;
    for item in reader {
        let item = item.map_err(map_rustix_io)?;
        let raw_name = item.file_name().to_bytes();
        if raw_name == b"." || raw_name == b".." {
            continue;
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
        direct_entry_count = direct_entry_count.saturating_add(1);
        retain_bounded_live_entry(
            &mut entries,
            live_entry_from_stat(parent, &name, &metadata, read_only)?,
            entry_limit,
        );
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
                direct_entry_count = direct_entry_count.saturating_add(1);
                retain_bounded_live_entry(
                    &mut entries,
                    live_entry_from_stat(parent, ".zfs", &metadata, read_only)?,
                    entry_limit,
                );
            }
        }
    }
    Ok((
        entries.into_values().collect(),
        direct_entry_count > entry_limit,
    ))
}

fn retain_bounded_live_entry(
    entries: &mut BTreeMap<(u8, String, String), BrowserEntry>,
    entry: BrowserEntry,
    entry_limit: usize,
) {
    if entry_limit == 0 {
        return;
    }
    let key = (
        entry_rank(entry.kind),
        entry.name.to_lowercase(),
        entry.name.clone(),
    );
    entries.insert(key, entry);
    if entries.len() > entry_limit {
        entries.pop_last();
    }
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
        selectable: directory,
    })
}

fn browse_retained_manifest_with_limit(
    configured_evidence_root: &Path,
    reference: &RetainedManifestReference,
    requested_path: &str,
    entry_limit: usize,
) -> Result<BrowserListing, SourceBrowserError> {
    if reference.artifact_size_bytes <= 0
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
    let file =
        fs::File::open(&manifest_path).map_err(|_| SourceBrowserError::ManifestUnavailable)?;
    let before = file
        .metadata()
        .map_err(|_| SourceBrowserError::ManifestUnavailable)?;
    let expected_size = u64::try_from(reference.artifact_size_bytes)
        .map_err(|_| SourceBrowserError::InvalidManifestBinding)?;
    if !before.is_file() || before.len() != expected_size {
        return Err(SourceBrowserError::ManifestIntegrity);
    }

    let (requested_relative, current_path) = normalized_browser_path(requested_path)?;
    let mut reader = HashingReader::new(BufReader::new(file));
    let scan = {
        let mut deserializer = serde_json::Deserializer::from_reader(&mut reader);
        let scan = RetainedManifestSeed {
            requested_relative: &requested_relative,
            current_path: &current_path,
            entry_limit,
        }
        .deserialize(&mut deserializer)
        .map_err(|_| SourceBrowserError::ManifestIntegrity)?;
        deserializer
            .end()
            .map_err(|_| SourceBrowserError::ManifestIntegrity)?;
        scan
    };
    let (reader, artifact_digest, bytes_read) = reader.finish();
    let after = reader
        .get_ref()
        .metadata()
        .map_err(|_| SourceBrowserError::ManifestUnavailable)?;
    let path_after = manifest_path
        .symlink_metadata()
        .map_err(|_| SourceBrowserError::ManifestUnavailable)?;
    if bytes_read != expected_size
        || artifact_digest != reference.artifact_sha256
        || metadata_identity(&before) != metadata_identity(&after)
        || metadata_identity(&before) != metadata_identity(&path_after)
    {
        return Err(SourceBrowserError::ManifestIntegrity);
    }
    scan.validate(reference)?;
    if !requested_relative.as_os_str().is_empty() && !scan.requested_directory_found {
        return Err(SourceBrowserError::NotDirectory);
    }
    let mut entries = scan.entries;
    entries.sort_by(compare_entries);
    entries.truncate(entry_limit);
    Ok(BrowserListing {
        parent_path: requested_relative.parent().map(portable_path).transpose()?,
        breadcrumbs: breadcrumbs(&requested_relative)?,
        current_path,
        read_only: true,
        snapshot: false,
        selectable: false,
        entries,
        truncated: scan.direct_entry_count > entry_limit,
        entry_limit,
    })
}

struct HashingReader<R> {
    inner: R,
    digest: Sha256,
    bytes_read: u64,
}

impl<R> HashingReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            digest: Sha256::new(),
            bytes_read: 0,
        }
    }

    fn finish(self) -> (R, String, u64) {
        (
            self.inner,
            format!("{:x}", self.digest.finalize()),
            self.bytes_read,
        )
    }
}

impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buffer)?;
        self.digest.update(&buffer[..read]);
        self.bytes_read = self
            .bytes_read
            .checked_add(read as u64)
            .ok_or_else(|| std::io::Error::other("manifest byte count overflow"))?;
        Ok(read)
    }
}

struct RetainedManifestSeed<'a> {
    requested_relative: &'a Path,
    current_path: &'a str,
    entry_limit: usize,
}

impl<'de> DeserializeSeed<'de> for RetainedManifestSeed<'_> {
    type Value = RetainedManifestScan;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for RetainedManifestSeed<'_> {
    type Value = RetainedManifestScan;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a TapeScale v2 file manifest")
    }

    fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        let mut format = None;
        let mut entries = None;
        let mut entry_count = None;
        let mut file_count = None;
        let mut logical_bytes = None;
        let mut digest_sha256 = None;
        let mut coverage = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "format" => set_manifest_field(&mut format, map.next_value()?, "format")?,
                "entries" => {
                    let value = map.next_value_seed(ManifestEntriesSeed {
                        requested_relative: self.requested_relative,
                        current_path: self.current_path,
                        entry_limit: self.entry_limit,
                    })?;
                    set_manifest_field(&mut entries, value, "entries")?;
                }
                "entry_count" => {
                    set_manifest_field(&mut entry_count, map.next_value()?, "entry_count")?;
                }
                "file_count" => {
                    set_manifest_field(&mut file_count, map.next_value()?, "file_count")?;
                }
                "logical_bytes" => {
                    set_manifest_field(&mut logical_bytes, map.next_value()?, "logical_bytes")?;
                }
                "digest_sha256" => {
                    set_manifest_field(&mut digest_sha256, map.next_value()?, "digest_sha256")?;
                }
                "coverage" => {
                    set_manifest_field(&mut coverage, map.next_value()?, "coverage")?;
                }
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        let entries = entries.ok_or_else(|| de::Error::missing_field("entries"))?;
        Ok(RetainedManifestScan {
            format: format.ok_or_else(|| de::Error::missing_field("format"))?,
            entries: entries.entries,
            direct_entry_count: entries.direct_entry_count,
            requested_directory_found: entries.requested_directory_found,
            computed_entry_count: entries.entry_count,
            computed_file_count: entries.file_count,
            computed_logical_bytes: entries.logical_bytes,
            computed_digest_sha256: entries.digest_sha256,
            declared_entry_count: entry_count
                .ok_or_else(|| de::Error::missing_field("entry_count"))?,
            declared_file_count: file_count
                .ok_or_else(|| de::Error::missing_field("file_count"))?,
            declared_logical_bytes: logical_bytes
                .ok_or_else(|| de::Error::missing_field("logical_bytes"))?,
            declared_digest_sha256: digest_sha256
                .ok_or_else(|| de::Error::missing_field("digest_sha256"))?,
            coverage: coverage.ok_or_else(|| de::Error::missing_field("coverage"))?,
        })
    }
}

fn set_manifest_field<E: de::Error, T>(
    field: &mut Option<T>,
    value: T,
    name: &'static str,
) -> Result<(), E> {
    if field.replace(value).is_some() {
        Err(E::duplicate_field(name))
    } else {
        Ok(())
    }
}

struct ManifestEntriesSeed<'a> {
    requested_relative: &'a Path,
    current_path: &'a str,
    entry_limit: usize,
}

impl<'de> DeserializeSeed<'de> for ManifestEntriesSeed<'_> {
    type Value = ManifestEntriesScan;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for ManifestEntriesSeed<'_> {
    type Value = ManifestEntriesScan;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a sorted sequence of manifest entries")
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut entry_count = 0_u64;
        let mut file_count = 0_u64;
        let mut logical_bytes = 0_u64;
        let mut previous_path: Option<String> = None;
        let mut root_seen = false;
        let mut requested_directory_found = self.requested_relative.as_os_str().is_empty();
        let mut direct_entry_count = 0_usize;
        // Each kind retains at most one directory-page worth of rows. This is
        // bounded even for a manifest containing millions of direct children,
        // while final sorting preserves the existing directories-first view.
        let mut entries_by_kind = [Vec::new(), Vec::new(), Vec::new()];
        let mut digest = Sha256::new();
        digest.update(b"[");
        while let Some(entry) = sequence.next_element::<ManifestEntry>()? {
            if entry_count > 0 {
                digest.update(b",");
            }
            let encoded = serde_json::to_vec(&entry).map_err(de::Error::custom)?;
            digest.update(encoded);
            entry_count = entry_count
                .checked_add(1)
                .ok_or_else(|| de::Error::custom("manifest entry count overflow"))?;
            if previous_path
                .as_deref()
                .is_some_and(|path| path >= entry.path.as_str())
            {
                return Err(de::Error::custom(
                    "manifest paths are duplicated or not strictly sorted",
                ));
            }
            previous_path = Some(entry.path.clone());
            if entry.path == "." {
                if root_seen || entry.kind != EntryKind::Directory {
                    return Err(de::Error::custom("manifest root entry is invalid"));
                }
                root_seen = true;
            } else {
                if entry.path.chars().any(char::is_control)
                    || validate_archive_path(Path::new(&entry.path)).is_err()
                {
                    return Err(de::Error::custom("manifest entry path is unsafe"));
                }
                if entry.kind == EntryKind::Directory && entry.path == self.current_path {
                    requested_directory_found = true;
                }
                let path = Path::new(&entry.path);
                if path.parent().unwrap_or_else(|| Path::new("")) == self.requested_relative {
                    direct_entry_count = direct_entry_count.saturating_add(1);
                    let browser_entry = retained_browser_entry(&entry)
                        .map_err(|error| de::Error::custom(error.to_string()))?;
                    let rank = usize::from(entry_rank(browser_entry.kind));
                    if entries_by_kind[rank].len() < self.entry_limit {
                        entries_by_kind[rank].push(browser_entry);
                    }
                }
            }
            if entry.kind == EntryKind::File {
                file_count = file_count
                    .checked_add(1)
                    .ok_or_else(|| de::Error::custom("manifest file count overflow"))?;
            }
            if let Some(size) = entry.size {
                logical_bytes = logical_bytes
                    .checked_add(size)
                    .ok_or_else(|| de::Error::custom("manifest logical byte count overflow"))?;
            }
        }
        digest.update(b"]");
        if !root_seen {
            return Err(de::Error::custom("manifest root entry is missing"));
        }
        Ok(ManifestEntriesScan {
            entries: entries_by_kind.into_iter().flatten().collect(),
            direct_entry_count,
            requested_directory_found,
            entry_count,
            file_count,
            logical_bytes,
            digest_sha256: format!("{:x}", digest.finalize()),
        })
    }
}

fn retained_browser_entry(entry: &ManifestEntry) -> Result<BrowserEntry, SourceBrowserError> {
    let path = Path::new(&entry.path);
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
    Ok(BrowserEntry {
        name,
        path: entry.path.clone(),
        kind,
        size_bytes: entry.size,
        modified_at: DateTime::<Utc>::from_timestamp(entry.mtime_seconds, 0),
        read_only: true,
        symlink: kind == BrowserEntryKind::Symlink,
        snapshot: false,
        navigable: kind == BrowserEntryKind::Directory,
        selectable: false,
    })
}

struct ManifestEntriesScan {
    entries: Vec<BrowserEntry>,
    direct_entry_count: usize,
    requested_directory_found: bool,
    entry_count: u64,
    file_count: u64,
    logical_bytes: u64,
    digest_sha256: String,
}

struct RetainedManifestScan {
    format: String,
    entries: Vec<BrowserEntry>,
    direct_entry_count: usize,
    requested_directory_found: bool,
    computed_entry_count: u64,
    computed_file_count: u64,
    computed_logical_bytes: u64,
    computed_digest_sha256: String,
    declared_entry_count: u64,
    declared_file_count: u64,
    declared_logical_bytes: u64,
    declared_digest_sha256: String,
    coverage: ManifestCoverage,
}

impl RetainedManifestScan {
    fn validate(&self, reference: &RetainedManifestReference) -> Result<(), SourceBrowserError> {
        if self.format != RETAINED_MANIFEST_FORMAT
            || self.declared_entry_count != self.computed_entry_count
            || self.declared_file_count != self.computed_file_count
            || self.declared_logical_bytes != self.computed_logical_bytes
            || self.declared_digest_sha256 != self.computed_digest_sha256
            || self.declared_digest_sha256 != reference.manifest_digest
            || self.coverage != ManifestCoverage::default()
        {
            Err(SourceBrowserError::ManifestIntegrity)
        } else {
            Ok(())
        }
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
        .any(|window| window[0] == ".zfs" && window[1] == "snapshot" && !window[2].is_empty())
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
    use std::{
        fs,
        io::{BufWriter, Read, Write},
        os::unix::fs::symlink,
    };

    use tempfile::tempdir;

    use crate::data_path::{build_manifest, persist_manifest};

    use super::*;

    fn write_large_retained_manifest(
        evidence_root: &Path,
        bucket_count: usize,
        files_per_bucket: usize,
    ) -> RetainedManifestReference {
        let run = evidence_root.join("large-run");
        fs::create_dir(&run).expect("large run directory");
        let path = run.join("source-manifest.json");
        let file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .expect("large manifest artifact");
        let mut writer = BufWriter::new(file);
        writer
            .write_all(b"{\"format\":\"tapescale-file-manifest-v2\",\"entries\":[")
            .expect("manifest header");
        let mut entry_digest = Sha256::new();
        entry_digest.update(b"[");
        let mut first = true;
        let mut entry_count = 0_u64;
        let mut file_count = 0_u64;
        let mut logical_bytes = 0_u64;
        let mut append = |entry: &ManifestEntry| {
            if !first {
                writer.write_all(b",").expect("entry separator");
                entry_digest.update(b",");
            }
            first = false;
            let encoded = serde_json::to_vec(entry).expect("synthetic manifest entry");
            writer.write_all(&encoded).expect("manifest entry");
            entry_digest.update(&encoded);
            entry_count += 1;
            if entry.kind == EntryKind::File {
                file_count += 1;
            }
            logical_bytes += entry.size.unwrap_or(0);
        };
        append(&ManifestEntry {
            path: ".".to_owned(),
            kind: EntryKind::Directory,
            mode: 0o755,
            uid: 1_000,
            gid: 1_000,
            mtime_seconds: 1_700_000_000,
            size: None,
            content_sha256: None,
            symlink_target: None,
            hard_link_group: None,
        });
        for bucket in 0..bucket_count {
            let directory = format!("bucket-{bucket:03}");
            append(&ManifestEntry {
                path: directory.clone(),
                kind: EntryKind::Directory,
                mode: 0o755,
                uid: 1_000,
                gid: 1_000,
                mtime_seconds: 1_700_000_000,
                size: None,
                content_sha256: None,
                symlink_target: None,
                hard_link_group: None,
            });
            for file in 0..files_per_bucket {
                append(&ManifestEntry {
                    path: format!("{directory}/file-{file:04}.bin"),
                    kind: EntryKind::File,
                    mode: 0o644,
                    uid: 1_000,
                    gid: 1_000,
                    mtime_seconds: 1_700_000_000,
                    size: Some(1),
                    content_sha256: Some("0".repeat(64)),
                    symlink_target: None,
                    hard_link_group: None,
                });
            }
        }
        entry_digest.update(b"]");
        let manifest_digest = format!("{:x}", entry_digest.finalize());
        write!(
            writer,
            "],\"entry_count\":{entry_count},\"file_count\":{file_count},\"logical_bytes\":{logical_bytes},\"digest_sha256\":\"{manifest_digest}\",\"coverage\":"
        )
        .expect("manifest summary");
        serde_json::to_writer(&mut writer, &ManifestCoverage::default()).expect("coverage");
        writer.write_all(b"}").expect("manifest footer");
        writer.flush().expect("flush large manifest");
        writer.get_ref().sync_all().expect("sync large manifest");
        drop(writer);

        let mut artifact_file = fs::File::open(&path).expect("reopen large manifest");
        let artifact_size_bytes = artifact_file.metadata().expect("artifact metadata").len();
        let mut artifact_digest = Sha256::new();
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = artifact_file.read(&mut buffer).expect("hash artifact");
            if read == 0 {
                break;
            }
            artifact_digest.update(&buffer[..read]);
        }
        RetainedManifestReference {
            storage_ref: path.to_string_lossy().into_owned(),
            artifact_sha256: format!("{:x}", artifact_digest.finalize()),
            artifact_size_bytes: i64::try_from(artifact_size_bytes).expect("artifact size"),
            manifest_digest,
        }
    }

    #[test]
    fn lists_directories_files_and_breadcrumbs_without_absolute_paths() {
        let root = tempdir().expect("source root");
        fs::create_dir_all(root.path().join("pool/snapshot/photos")).expect("directories");
        fs::write(root.path().join("pool/snapshot/image.raw"), b"raw-data").expect("source file");

        let listing = browse_source(root.path(), "pool/snapshot").expect("listing");
        assert_eq!(listing.current_path, "pool/snapshot");
        assert_eq!(listing.parent_path.as_deref(), Some("pool"));
        assert!(!listing.read_only);
        assert!(listing.selectable);
        assert_eq!(listing.entries.len(), 2);
        assert_eq!(listing.entries[0].name, "photos");
        assert_eq!(listing.entries[0].kind, BrowserEntryKind::Directory);
        assert!(listing.entries[0].navigable);
        assert!(listing.entries[0].selectable);
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
        let snapshot_container = browse_source(root.path(), "dataset/.zfs/snapshot")
            .expect("snapshot container listing");
        assert!(!snapshot_container.snapshot);
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

        let (entries, truncated) =
            live_entries_from_directory(&selected_descriptor, selected, false, 10)
                .expect("the held descriptor should remain listable");
        assert!(!truncated);
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
    fn oversized_directory_returns_a_deterministic_partial_view() {
        let root = tempdir().expect("source root");
        for name in ["z-last", "b-second", "a-first"] {
            fs::write(root.path().join(name), b"").expect("entry");
        }
        let listing = browse_source_with_limit(root.path(), "", 2)
            .expect("oversized directories should remain browsable");
        assert!(listing.truncated);
        assert_eq!(listing.entry_limit, 2);
        assert_eq!(listing.entries.len(), 2);
        assert_eq!(listing.entries[0].name, "a-first");
        assert_eq!(listing.entries[1].name, "b-second");
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
        let partial = browse_retained_manifest_with_limit(evidence.path(), &reference, "", 1)
            .expect("bounded retained view");
        assert!(partial.truncated);
        assert_eq!(partial.entry_limit, 1);
        assert_eq!(partial.entries.len(), 1);
        assert_eq!(partial.entries[0].name, "photos");
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

    #[test]
    fn retained_browser_streams_manifests_beyond_the_g02_entry_limit() {
        let evidence = tempdir().expect("evidence");
        let reference = write_large_retained_manifest(evidence.path(), 101, 1_000);

        let root = browse_retained_manifest(evidence.path(), &reference, "")
            .expect("large manifest root should be browsable");
        assert_eq!(root.entries.len(), 101);
        assert!(!root.truncated);
        assert_eq!(root.entries[0].name, "bucket-000");
        assert_eq!(root.entries[100].name, "bucket-100");

        let bucket = browse_retained_manifest(evidence.path(), &reference, "bucket-100")
            .expect("nested directory after 100,000 entries should be browsable");
        assert_eq!(bucket.entries.len(), 1_000);
        assert!(!bucket.truncated);
        assert_eq!(bucket.entries[999].name, "file-0999.bin");
    }
}
