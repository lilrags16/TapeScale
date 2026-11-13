//! Deterministic file-tree evidence for physical backup and restore exercises.
//!
//! This module deliberately does not follow symlinks. A successful comparison
//! proves the file content and the metadata fields represented here; extended
//! attributes and ACL semantics remain an explicit later qualification gate.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    fs::{self, File, Metadata},
    io::{self, Read, Write},
    os::fd::AsFd,
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use rustix::{
    fs::{fstatvfs, mkdirat, open, openat, Mode, OFlags, StatVfsMountFlags},
    io::Errno,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::domain::path::{resolve_link_target, validate_archive_path};

const MANIFEST_FORMAT: &str = "tapescale-file-manifest-v2";
const MAX_DIFFERENCES: usize = 1_000;
pub const MAX_G02_MANIFEST_ENTRIES: u64 = 100_000;
pub const MAX_G02_LOGICAL_BYTES: u64 = 100 * 1024 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("path must be a non-empty relative path without traversal")]
    InvalidRelativePath,
    #[error("configured root is not an accessible directory: {0}")]
    InvalidRoot(PathBuf),
    #[error("source selection is not on a read-only mount: {0}")]
    WritableSource(PathBuf),
    #[error("source selection must be inside a named ZFS snapshot: {0}")]
    NotNamedZfsSnapshot(PathBuf),
    #[error("path escaped the configured root: {0}")]
    EscapedRoot(PathBuf),
    #[error("unsupported source object at {path}: {kind}")]
    UnsupportedObject { path: String, kind: &'static str },
    #[error("source object changed while it was being inventoried: {0}")]
    ChangedDuringRead(String),
    #[error("manifest inventory was cancelled")]
    Cancelled,
    #[error("G0.2 currently requires UTF-8 path and symlink names: {0}")]
    NonUtf8Path(PathBuf),
    #[error("unsafe archive path or symlink target at {0}")]
    UnsafeArchivePath(String),
    #[error("nested filesystem boundary is outside the G0.2 OneFS capture at {0}")]
    FilesystemBoundary(PathBuf),
    #[error("G0.2 fixture exceeds the laboratory {kind} limit of {limit}")]
    FixtureLimit { kind: &'static str, limit: u64 },
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("manifest serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("manifest integrity validation failed: {0}")]
    Integrity(&'static str),
    #[error("a manifest artifact already exists and will not be overwritten: {0}")]
    ArtifactExists(PathBuf),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FileManifest {
    pub format: String,
    pub entries: Vec<ManifestEntry>,
    pub entry_count: u64,
    pub file_count: u64,
    pub logical_bytes: u64,
    pub digest_sha256: String,
    pub coverage: ManifestCoverage,
    #[serde(skip)]
    pub source_stability_digest_sha256: String,
}

impl PartialEq for FileManifest {
    fn eq(&self, other: &Self) -> bool {
        self.format == other.format
            && self.entries == other.entries
            && self.entry_count == other.entry_count
            && self.file_count == other.file_count
            && self.logical_bytes == other.logical_bytes
            && self.digest_sha256 == other.digest_sha256
            && self.coverage == other.coverage
    }
}

impl Eq for FileManifest {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestCoverage {
    pub content_sha256: bool,
    pub object_type: bool,
    pub mode: bool,
    pub numeric_owner: bool,
    pub mtime_nanoseconds: bool,
    pub symlink_target: bool,
    pub hard_link_relationships: bool,
    pub sparse_shape: bool,
    pub extended_attributes: bool,
    pub posix_acl: bool,
    pub nfs4_acl: bool,
    pub atime: bool,
}

impl Default for ManifestCoverage {
    fn default() -> Self {
        Self {
            content_sha256: true,
            object_type: true,
            mode: true,
            numeric_owner: true,
            mtime_nanoseconds: false,
            symlink_target: true,
            hard_link_relationships: true,
            sparse_shape: false,
            extended_attributes: false,
            posix_acl: false,
            nfs4_acl: false,
            atime: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestEntry {
    pub path: String,
    pub kind: EntryKind,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub mtime_seconds: i64,
    pub size: Option<u64>,
    pub content_sha256: Option<String>,
    pub symlink_target: Option<String>,
    pub hard_link_group: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Directory,
    File,
    Symlink,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestComparison {
    pub matched: bool,
    pub expected_digest: String,
    pub actual_digest: String,
    pub expected_entries: u64,
    pub actual_entries: u64,
    pub differences: Vec<String>,
    pub differences_truncated: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestArtifactSummary {
    pub storage_path: PathBuf,
    pub artifact_sha256: String,
    pub size_bytes: u64,
    pub manifest_digest: String,
    pub entry_count: u64,
    pub logical_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct JsonArtifactSummary {
    pub storage_path: PathBuf,
    pub artifact_sha256: String,
    pub size_bytes: u64,
}

/// Resolve an existing relative selection beneath an allowlisted root.
pub fn resolve_existing_selection(
    configured_root: &Path,
    relative: &str,
) -> Result<PathBuf, ManifestError> {
    validate_relative(relative)?;
    let root = configured_root
        .canonicalize()
        .map_err(|source| ManifestError::Io {
            path: configured_root.to_owned(),
            source,
        })?;
    if !root.is_dir() {
        return Err(ManifestError::InvalidRoot(root));
    }
    let candidate = root
        .join(relative)
        .canonicalize()
        .map_err(|source| ManifestError::Io {
            path: root.join(relative),
            source,
        })?;
    if !candidate.starts_with(&root) {
        return Err(ManifestError::EscapedRoot(candidate));
    }
    Ok(candidate)
}

/// Resolve one operational backup source and prove that both the requested
/// path and its canonical target remain beneath `.zfs/snapshot/<name>`.
///
/// Checking both forms prevents a manually entered symlink inside a snapshot
/// from resolving back into a mutable live dataset while still looking safe in
/// the persisted request string.
pub fn resolve_existing_zfs_snapshot_selection(
    configured_root: &Path,
    relative: &str,
) -> Result<PathBuf, ManifestError> {
    let requested = Path::new(relative);
    if !is_named_zfs_snapshot_path(requested) {
        return Err(ManifestError::NotNamedZfsSnapshot(requested.to_owned()));
    }

    let root = configured_root
        .canonicalize()
        .map_err(|source| ManifestError::Io {
            path: configured_root.to_owned(),
            source,
        })?;
    let resolved = resolve_existing_selection(&root, relative)?;
    let canonical_relative = resolved
        .strip_prefix(&root)
        .map_err(|_| ManifestError::EscapedRoot(resolved.clone()))?;
    if !is_named_zfs_snapshot_path(canonical_relative) {
        return Err(ManifestError::NotNamedZfsSnapshot(resolved));
    }
    Ok(resolved)
}

#[must_use]
pub fn is_named_zfs_snapshot_path(path: &Path) -> bool {
    let components = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>();
    components
        .windows(3)
        .any(|window| window[0] == ".zfs" && window[1] == "snapshot")
}

/// Prove the source is covered by the most-specific read-only Linux mount.
/// Container deployments bind the TrueNAS snapshot tree at `/source:ro`; this
/// check makes the immutability assertion independently visible to the worker.
pub fn verify_read_only_mount(path: &Path) -> Result<(), ManifestError> {
    let canonical = path.canonicalize().map_err(|source| ManifestError::Io {
        path: path.to_owned(),
        source,
    })?;
    let directory = open(
        &canonical,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|source| ManifestError::Io {
        path: canonical.clone(),
        source: source.into(),
    })?;
    if descriptor_is_on_read_only_mount(&directory).map_err(|source| ManifestError::Io {
        path: canonical.clone(),
        source,
    })? {
        Ok(())
    } else {
        Err(ManifestError::WritableSource(canonical))
    }
}

/// Ask the kernel for the mount flags attached to an already-open path.
///
/// An open descriptor retains the exact bind mount used to reach the source,
/// so this avoids translating `/proc/self/fd/*` back into a pathname and then
/// trying to rediscover the matching mount from `/proc/self/mountinfo`.
pub fn descriptor_is_on_read_only_mount<Fd: AsFd>(descriptor: Fd) -> io::Result<bool> {
    let status = fstatvfs(descriptor).map_err(io::Error::from)?;
    Ok(mount_flags_are_read_only(status.f_flag))
}

/// Construct a destination path without creating it. Every component remains
/// beneath the allowlisted restore root and the run id prevents accidental
/// reuse of an operator-selected directory.
pub fn isolated_restore_destination(
    configured_root: &Path,
    relative: &str,
    run_id: uuid::Uuid,
) -> Result<PathBuf, ManifestError> {
    validate_relative(relative)?;
    let root = configured_root
        .canonicalize()
        .map_err(|source| ManifestError::Io {
            path: configured_root.to_owned(),
            source,
        })?;
    if !root.is_dir() {
        return Err(ManifestError::InvalidRoot(root));
    }
    let parent = root.join(relative);
    let nearest_existing = parent
        .ancestors()
        .find(|path| path.exists())
        .ok_or_else(|| ManifestError::InvalidRoot(root.clone()))?
        .canonicalize()
        .map_err(|source| ManifestError::Io {
            path: parent.clone(),
            source,
        })?;
    if !nearest_existing.starts_with(&root) {
        return Err(ManifestError::EscapedRoot(parent));
    }
    Ok(parent.join(run_id.to_string()))
}

/// Create the unique restore directory by walking from an already-open root.
///
/// Every component is opened with `O_NOFOLLOW`, and every mkdir is relative to
/// the previously verified directory descriptor. This closes the gap between
/// API-time path validation and worker-time creation without following a
/// symlink that appeared in the meantime.
pub fn create_isolated_restore_destination(
    configured_root: &Path,
    relative: &str,
    run_id: uuid::Uuid,
) -> Result<PathBuf, ManifestError> {
    validate_relative(relative)?;
    let root = configured_root
        .canonicalize()
        .map_err(|source| ManifestError::Io {
            path: configured_root.to_owned(),
            source,
        })?;
    if !root.is_dir() || root != configured_root {
        return Err(ManifestError::InvalidRoot(configured_root.to_owned()));
    }
    let mut directory = open(
        &root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|source| ManifestError::Io {
        path: root.clone(),
        source: source.into(),
    })?;
    let mut components = Path::new(relative)
        .components()
        .map(|component| match component {
            Component::Normal(value) => Ok(value.to_os_string()),
            _ => Err(ManifestError::InvalidRelativePath),
        })
        .collect::<Result<Vec<_>, _>>()?;
    components.push(run_id.to_string().into());
    let mut destination = root;

    for (index, component) in components.iter().enumerate() {
        let final_component = index + 1 == components.len();
        destination.push(component);
        match mkdirat(&directory, component, Mode::RWXU) {
            Ok(()) => {}
            Err(Errno::EXIST) if !final_component => {}
            Err(Errno::EXIST) => return Err(ManifestError::ArtifactExists(destination)),
            Err(source) => {
                return Err(ManifestError::Io {
                    path: destination,
                    source: source.into(),
                });
            }
        }
        directory = openat(
            &directory,
            component,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|source| ManifestError::Io {
            path: destination.clone(),
            source: source.into(),
        })?;
    }
    Ok(destination)
}

pub fn build_manifest(root: &Path) -> Result<FileManifest, ManifestError> {
    build_manifest_with_cancel(root, &AtomicBool::new(false))
}

pub fn build_manifest_with_cancel(
    root: &Path,
    cancellation: &AtomicBool,
) -> Result<FileManifest, ManifestError> {
    let root = root.canonicalize().map_err(|source| ManifestError::Io {
        path: root.to_owned(),
        source,
    })?;
    if !root.is_dir() {
        return Err(ManifestError::InvalidRoot(root));
    }
    let root_device = fs::symlink_metadata(&root)
        .map_err(|source| ManifestError::Io {
            path: root.clone(),
            source,
        })?
        .dev();

    let mut pending = vec![PathBuf::new()];
    let mut raw_entries = Vec::new();
    let mut logical_bytes_seen = 0_u64;
    while let Some(relative) = pending.pop() {
        if cancellation.load(Ordering::Acquire) {
            return Err(ManifestError::Cancelled);
        }
        let absolute = root.join(&relative);
        let metadata = fs::symlink_metadata(&absolute).map_err(|source| ManifestError::Io {
            path: absolute.clone(),
            source,
        })?;
        ensure_same_filesystem(root_device, metadata.dev(), &relative)?;
        let kind = entry_kind(&metadata, &relative)?;
        if kind == EntryKind::Directory {
            let directory = fs::read_dir(&absolute).map_err(|source| ManifestError::Io {
                path: absolute.clone(),
                source,
            })?;
            let mut children = Vec::new();
            for entry in directory {
                if cancellation.load(Ordering::Acquire) {
                    return Err(ManifestError::Cancelled);
                }
                children.push(
                    entry
                        .map_err(|source| ManifestError::Io {
                            path: absolute.clone(),
                            source,
                        })?
                        .file_name(),
                );
                let known_entries = raw_entries
                    .len()
                    .saturating_add(pending.len())
                    .saturating_add(children.len())
                    .saturating_add(1);
                if u64::try_from(known_entries).unwrap_or(u64::MAX) > MAX_G02_MANIFEST_ENTRIES {
                    return Err(ManifestError::FixtureLimit {
                        kind: "manifest-entry",
                        limit: MAX_G02_MANIFEST_ENTRIES,
                    });
                }
            }
            if let Some(non_utf8) = children.iter().find(|child| child.to_str().is_none()) {
                return Err(ManifestError::NonUtf8Path(relative.join(non_utf8)));
            }
            children.sort_by_key(|left| os_sort_key(left));
            for child in children.into_iter().rev() {
                pending.push(relative.join(child));
            }
        }
        let known_entries = raw_entries
            .len()
            .saturating_add(pending.len())
            .saturating_add(1);
        if u64::try_from(known_entries).unwrap_or(u64::MAX) > MAX_G02_MANIFEST_ENTRIES {
            return Err(ManifestError::FixtureLimit {
                kind: "manifest-entry",
                limit: MAX_G02_MANIFEST_ENTRIES,
            });
        }
        if kind == EntryKind::File {
            logical_bytes_seen = logical_bytes_seen.saturating_add(metadata.len());
            if logical_bytes_seen > MAX_G02_LOGICAL_BYTES {
                return Err(ManifestError::FixtureLimit {
                    kind: "logical-byte",
                    limit: MAX_G02_LOGICAL_BYTES,
                });
            }
        }
        raw_entries.push(RawEntry::capture(
            &absolute,
            &relative,
            metadata,
            kind,
            cancellation,
        )?);
    }

    raw_entries.sort_by(|left, right| left.path.cmp(&right.path));
    let source_stability_digest_sha256 = exact_stability_digest(&raw_entries)?;
    let mut link_groups = BTreeMap::<(u64, u64), String>::new();
    let mut next_link_group = 1_u64;
    let mut entries = Vec::with_capacity(raw_entries.len());
    for raw in raw_entries {
        let hard_link_group = raw.hard_link_identity.map(|identity| {
            link_groups
                .entry(identity)
                .or_insert_with(|| {
                    let value = format!("link-{next_link_group}");
                    next_link_group += 1;
                    value
                })
                .clone()
        });
        entries.push(ManifestEntry {
            path: raw.path,
            kind: raw.kind,
            mode: raw.mode,
            uid: raw.uid,
            gid: raw.gid,
            mtime_seconds: raw.mtime_seconds,
            size: raw.size,
            content_sha256: raw.content_sha256,
            symlink_target: raw.symlink_target,
            hard_link_group,
        });
    }

    let file_count = entries
        .iter()
        .filter(|entry| entry.kind == EntryKind::File)
        .count() as u64;
    let logical_bytes = entries.iter().filter_map(|entry| entry.size).sum();
    let digest_sha256 = manifest_digest(&entries)?;
    Ok(FileManifest {
        format: MANIFEST_FORMAT.to_owned(),
        entry_count: entries.len() as u64,
        entries,
        file_count,
        logical_bytes,
        digest_sha256,
        coverage: ManifestCoverage::default(),
        source_stability_digest_sha256,
    })
}

/// Compare two scans of the same mounted source using exact filesystem
/// identity and nanosecond timestamps. This is deliberately separate from the
/// recovery manifest, whose mtime precision matches Bacula's whole seconds.
#[must_use]
pub fn source_manifests_stable(before: &FileManifest, after: &FileManifest) -> bool {
    !before.source_stability_digest_sha256.is_empty()
        && before.source_stability_digest_sha256 == after.source_stability_digest_sha256
}

/// Persist a manifest without replacing prior evidence. The caller supplies a
/// trusted per-run directory; `artifact_name` is restricted to a short token.
pub fn persist_manifest(
    run_directory: &Path,
    artifact_name: &str,
    manifest: &FileManifest,
) -> Result<ManifestArtifactSummary, ManifestError> {
    let bytes = serde_json::to_vec_pretty(manifest)?;
    let artifact = persist_artifact_bytes(run_directory, artifact_name, &bytes)?;
    Ok(ManifestArtifactSummary {
        storage_path: artifact.storage_path,
        artifact_sha256: artifact.artifact_sha256,
        size_bytes: artifact.size_bytes,
        manifest_digest: manifest.digest_sha256.clone(),
        entry_count: manifest.entry_count,
        logical_bytes: manifest.logical_bytes,
    })
}

/// Persist structured run evidence with create-once semantics.
pub fn persist_json_artifact<T: Serialize>(
    run_directory: &Path,
    artifact_name: &str,
    value: &T,
) -> Result<JsonArtifactSummary, ManifestError> {
    let bytes = serde_json::to_vec_pretty(value)?;
    persist_artifact_bytes(run_directory, artifact_name, &bytes)
}

fn persist_artifact_bytes(
    run_directory: &Path,
    artifact_name: &str,
    bytes: &[u8],
) -> Result<JsonArtifactSummary, ManifestError> {
    if artifact_name.is_empty()
        || artifact_name.len() > 64
        || !artifact_name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(ManifestError::InvalidRelativePath);
    }
    fs::create_dir_all(run_directory).map_err(|source| ManifestError::Io {
        path: run_directory.to_owned(),
        source,
    })?;
    let destination = run_directory.join(format!("{artifact_name}.json"));
    if destination.exists() {
        return Err(ManifestError::ArtifactExists(destination));
    }
    let temporary = run_directory.join(format!(".{artifact_name}.{}.tmp", uuid::Uuid::now_v7()));
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|source| ManifestError::Io {
            path: temporary.clone(),
            source,
        })?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|source| ManifestError::Io {
            path: temporary.clone(),
            source,
        })?;
    drop(file);
    if let Err(source) = fs::hard_link(&temporary, &destination) {
        let _ = fs::remove_file(&temporary);
        return if source.kind() == io::ErrorKind::AlreadyExists {
            Err(ManifestError::ArtifactExists(destination))
        } else {
            Err(ManifestError::Io {
                path: destination,
                source,
            })
        };
    }
    fs::remove_file(&temporary).map_err(|source| ManifestError::Io {
        path: temporary,
        source,
    })?;
    Ok(JsonArtifactSummary {
        storage_path: destination,
        artifact_sha256: format!("{:x}", Sha256::digest(bytes)),
        size_bytes: bytes.len() as u64,
    })
}

pub fn load_manifest(path: &Path) -> Result<FileManifest, ManifestError> {
    let bytes = fs::read(path).map_err(|source| ManifestError::Io {
        path: path.to_owned(),
        source,
    })?;
    load_manifest_bytes(&bytes)
}

/// Decode and validate a manifest from one caller-owned byte snapshot.
///
/// Callers that separately authenticate an artifact must use this entry point
/// so the bytes they validate are exactly the bytes that are decoded. Reopening
/// a pathname after hashing would leave a time-of-check/time-of-use gap.
pub fn load_manifest_bytes(bytes: &[u8]) -> Result<FileManifest, ManifestError> {
    load_manifest_bytes_with_limit(bytes, MAX_G02_MANIFEST_ENTRIES)
}

fn load_manifest_bytes_with_limit(
    bytes: &[u8],
    max_entries: u64,
) -> Result<FileManifest, ManifestError> {
    let manifest: FileManifest = serde_json::from_slice(bytes)?;
    if manifest.format != MANIFEST_FORMAT {
        return Err(ManifestError::Integrity("unsupported format"));
    }
    if manifest.entry_count != manifest.entries.len() as u64 {
        return Err(ManifestError::Integrity("entry count mismatch"));
    }
    if manifest.entry_count > max_entries {
        return Err(ManifestError::Integrity(
            "entry count exceeds product limit",
        ));
    }
    if manifest.file_count
        != manifest
            .entries
            .iter()
            .filter(|entry| entry.kind == EntryKind::File)
            .count() as u64
    {
        return Err(ManifestError::Integrity("file count mismatch"));
    }
    if manifest.logical_bytes
        != manifest
            .entries
            .iter()
            .filter_map(|entry| entry.size)
            .sum::<u64>()
    {
        return Err(ManifestError::Integrity("logical byte count mismatch"));
    }
    if manifest.coverage != ManifestCoverage::default() {
        return Err(ManifestError::Integrity("coverage declaration mismatch"));
    }
    if manifest.digest_sha256 != manifest_digest(&manifest.entries)? {
        return Err(ManifestError::Integrity("entry digest mismatch"));
    }
    Ok(manifest)
}

#[must_use]
pub fn compare_manifests(expected: &FileManifest, actual: &FileManifest) -> ManifestComparison {
    let expected_by_path = expected
        .entries
        .iter()
        .map(|entry| (&entry.path, entry))
        .collect::<BTreeMap<_, _>>();
    let actual_by_path = actual
        .entries
        .iter()
        .map(|entry| (&entry.path, entry))
        .collect::<BTreeMap<_, _>>();
    let paths = expected_by_path
        .keys()
        .chain(actual_by_path.keys())
        .copied()
        .collect::<BTreeSet<_>>();
    let mut differences = Vec::new();
    let mut differences_truncated = false;
    for path in paths {
        let difference = match (expected_by_path.get(path), actual_by_path.get(path)) {
            (Some(expected), Some(actual)) if expected == actual => None,
            (Some(expected), Some(actual)) => Some(format!(
                "metadata or content mismatch at {path}: expected {}, observed {}",
                entry_digest(expected),
                entry_digest(actual)
            )),
            (Some(_), None) => Some(format!("missing restored path: {path}")),
            (None, Some(_)) => Some(format!("unexpected restored path: {path}")),
            (None, None) => None,
        };
        if let Some(difference) = difference {
            if differences.len() < MAX_DIFFERENCES {
                differences.push(difference);
            } else {
                differences_truncated = true;
            }
        }
    }
    ManifestComparison {
        matched: differences.is_empty() && !differences_truncated,
        expected_digest: expected.digest_sha256.clone(),
        actual_digest: actual.digest_sha256.clone(),
        expected_entries: expected.entry_count,
        actual_entries: actual.entry_count,
        differences,
        differences_truncated,
    }
}

fn validate_relative(value: &str) -> Result<(), ManifestError> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(ManifestError::InvalidRelativePath);
    }
    Ok(())
}

fn mount_flags_are_read_only(flags: StatVfsMountFlags) -> bool {
    flags.contains(StatVfsMountFlags::RDONLY)
}

fn entry_kind(metadata: &Metadata, relative: &Path) -> Result<EntryKind, ManifestError> {
    let file_type = metadata.file_type();
    if file_type.is_dir() {
        Ok(EntryKind::Directory)
    } else if file_type.is_file() {
        Ok(EntryKind::File)
    } else if file_type.is_symlink() {
        Ok(EntryKind::Symlink)
    } else {
        let kind = if file_type.is_socket() {
            "socket"
        } else if file_type.is_fifo() {
            "fifo"
        } else if file_type.is_block_device() {
            "block device"
        } else if file_type.is_char_device() {
            "character device"
        } else {
            "unknown special file"
        };
        Err(ManifestError::UnsupportedObject {
            path: display_relative(relative),
            kind,
        })
    }
}

fn ensure_same_filesystem(
    root_device: u64,
    entry_device: u64,
    relative: &Path,
) -> Result<(), ManifestError> {
    if entry_device == root_device {
        Ok(())
    } else {
        Err(ManifestError::FilesystemBoundary(relative.to_owned()))
    }
}

#[derive(Debug, Serialize)]
struct RawEntry {
    path: String,
    kind: EntryKind,
    mode: u32,
    uid: u32,
    gid: u32,
    mtime_seconds: i64,
    mtime_nanoseconds: i64,
    size: Option<u64>,
    content_sha256: Option<String>,
    symlink_target: Option<String>,
    hard_link_identity: Option<(u64, u64)>,
    sparse: Option<bool>,
}

impl RawEntry {
    fn capture(
        absolute: &Path,
        relative: &Path,
        before: Metadata,
        kind: EntryKind,
        cancellation: &AtomicBool,
    ) -> Result<Self, ManifestError> {
        let path = display_relative(relative);
        let safe_path = if relative.as_os_str().is_empty() {
            None
        } else {
            Some(
                validate_archive_path(relative)
                    .map_err(|_| ManifestError::UnsafeArchivePath(path.clone()))?,
            )
        };
        let (size, content_sha256, hard_link_identity, sparse) = if kind == EntryKind::File {
            let digest = hash_file(absolute, cancellation)?;
            let after = fs::symlink_metadata(absolute).map_err(|source| ManifestError::Io {
                path: absolute.to_owned(),
                source,
            })?;
            if metadata_identity(&before) != metadata_identity(&after) {
                return Err(ManifestError::ChangedDuringRead(path));
            }
            (
                Some(before.len()),
                Some(digest),
                (before.nlink() > 1).then(|| (before.dev(), before.ino())),
                Some(before.blocks().saturating_mul(512) < before.len()),
            )
        } else {
            (None, None, None, None)
        };
        let symlink_target = if kind == EntryKind::Symlink {
            let target = fs::read_link(absolute).map_err(|source| ManifestError::Io {
                path: absolute.to_owned(),
                source,
            })?;
            resolve_link_target(
                safe_path
                    .as_ref()
                    .expect("the source root itself cannot be a symlink after canonicalization"),
                &target,
            )
            .map_err(|_| ManifestError::UnsafeArchivePath(path.clone()))?;
            Some(
                target
                    .to_str()
                    .ok_or_else(|| ManifestError::NonUtf8Path(absolute.to_owned()))?
                    .to_owned(),
            )
        } else {
            None
        };
        Ok(Self {
            path,
            kind,
            mode: before.permissions().mode() & 0o7777,
            uid: before.uid(),
            gid: before.gid(),
            mtime_seconds: before.mtime(),
            mtime_nanoseconds: before.mtime_nsec(),
            size,
            content_sha256,
            symlink_target,
            hard_link_identity,
            sparse,
        })
    }
}

fn metadata_identity(metadata: &Metadata) -> (u64, u64, u64, i64, i64) {
    (
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
    )
}

fn hash_file(path: &Path, cancellation: &AtomicBool) -> Result<String, ManifestError> {
    let mut file = File::open(path).map_err(|source| ManifestError::Io {
        path: path.to_owned(),
        source,
    })?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        if cancellation.load(Ordering::Acquire) {
            return Err(ManifestError::Cancelled);
        }
        let read = file.read(&mut buffer).map_err(|source| ManifestError::Io {
            path: path.to_owned(),
            source,
        })?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn manifest_digest(entries: &[ManifestEntry]) -> Result<String, ManifestError> {
    let bytes = serde_json::to_vec(entries)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn exact_stability_digest(entries: &[RawEntry]) -> Result<String, ManifestError> {
    let bytes = serde_json::to_vec(entries)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn entry_digest(entry: &ManifestEntry) -> String {
    serde_json::to_vec(entry).map_or_else(
        |_| "unavailable".to_owned(),
        |bytes| format!("{:x}", Sha256::digest(bytes)),
    )
}

fn display_relative(path: &Path) -> String {
    if path.as_os_str().is_empty() {
        ".".to_owned()
    } else {
        path.to_string_lossy().replace('\\', "/")
    }
}

fn os_sort_key(value: &OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    value.as_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{self, FileTimes},
        os::unix::fs::symlink,
        sync::atomic::AtomicBool,
        time::{Duration, SystemTime},
    };

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn manifest_detects_content_and_link_relationships() {
        let source = tempdir().expect("source");
        fs::create_dir(source.path().join("folder")).expect("folder");
        fs::write(source.path().join("folder/file.txt"), b"tape evidence").expect("file");
        fs::hard_link(
            source.path().join("folder/file.txt"),
            source.path().join("hard-link.txt"),
        )
        .expect("hard link");
        symlink("folder/file.txt", source.path().join("symbolic-link")).expect("symlink");

        let manifest = build_manifest(source.path()).expect("manifest");
        assert_eq!(manifest.file_count, 2);
        assert_eq!(manifest.logical_bytes, 26);
        assert!(manifest.entries.iter().any(|entry| entry.path == "."));
        assert!(!manifest.coverage.mtime_nanoseconds);
        assert!(!manifest.coverage.sparse_shape);
        let links = manifest
            .entries
            .iter()
            .filter_map(|entry| entry.hard_link_group.as_deref())
            .collect::<Vec<_>>();
        assert_eq!(links.len(), 2);
        assert_eq!(links[0], links[1]);
        assert!(manifest
            .entries
            .iter()
            .any(|entry| entry.symlink_target.as_deref() == Some("folder/file.txt")));
        assert!(compare_manifests(&manifest, &manifest).matched);
    }

    #[test]
    fn comparison_reports_missing_changed_and_extra_paths() {
        let expected_root = tempdir().expect("expected");
        fs::write(expected_root.path().join("same"), b"expected").expect("expected file");
        fs::write(expected_root.path().join("missing"), b"missing").expect("missing file");
        let actual_root = tempdir().expect("actual");
        fs::write(actual_root.path().join("same"), b"changed").expect("changed file");
        fs::write(actual_root.path().join("extra"), b"extra").expect("extra file");

        let comparison = compare_manifests(
            &build_manifest(expected_root.path()).expect("expected manifest"),
            &build_manifest(actual_root.path()).expect("actual manifest"),
        );
        assert!(!comparison.matched);
        assert!(comparison
            .differences
            .iter()
            .any(|difference| difference.contains("missing restored path: missing")));
        assert!(comparison
            .differences
            .iter()
            .any(|difference| difference.contains("unexpected restored path: extra")));
        assert!(comparison
            .differences
            .iter()
            .any(|difference| difference.contains("mismatch at same")));
    }

    #[test]
    fn recovery_comparison_uses_seconds_but_source_stability_keeps_nanoseconds() {
        let source = tempdir().expect("source");
        let file_path = source.path().join("timestamped");
        fs::write(&file_path, b"same bytes").expect("file");
        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        fs::File::options()
            .write(true)
            .open(&file_path)
            .expect("open")
            .set_times(FileTimes::new().set_modified(base + Duration::from_nanos(100)))
            .expect("first timestamp");
        let before = build_manifest(source.path()).expect("before");
        fs::File::options()
            .write(true)
            .open(&file_path)
            .expect("open")
            .set_times(FileTimes::new().set_modified(base + Duration::from_nanos(900)))
            .expect("second timestamp");
        let after = build_manifest(source.path()).expect("after");

        assert!(compare_manifests(&before, &after).matched);
        assert!(!source_manifests_stable(&before, &after));
    }

    #[test]
    fn inventory_cancellation_and_nested_filesystems_are_explicit() {
        let source = tempdir().expect("source");
        let cancellation = AtomicBool::new(true);
        assert!(matches!(
            build_manifest_with_cancel(source.path(), &cancellation),
            Err(ManifestError::Cancelled)
        ));
        assert!(matches!(
            ensure_same_filesystem(10, 11, Path::new("nested-dataset")),
            Err(ManifestError::FilesystemBoundary(path)) if path == Path::new("nested-dataset")
        ));
    }

    #[test]
    fn contained_paths_reject_traversal_and_symlink_escape() {
        let root = tempdir().expect("root");
        fs::create_dir(root.path().join("snapshot")).expect("snapshot");
        assert_eq!(
            resolve_existing_selection(root.path(), "snapshot").expect("contained"),
            root.path()
                .join("snapshot")
                .canonicalize()
                .expect("canonical")
        );
        assert!(resolve_existing_selection(root.path(), "../outside").is_err());

        let outside = tempdir().expect("outside");
        symlink(outside.path(), root.path().join("escape")).expect("escape symlink");
        assert!(resolve_existing_selection(root.path(), "escape").is_err());
        assert!(
            isolated_restore_destination(root.path(), "../escape", uuid::Uuid::now_v7()).is_err()
        );
    }

    #[test]
    fn operational_source_requires_a_named_snapshot_after_symlink_resolution() {
        let root = tempdir().expect("root");
        let live = root.path().join("pool/dataset/live");
        let snapshot = root.path().join("pool/dataset/.zfs/snapshot/nightly/files");
        fs::create_dir_all(&live).expect("live dataset");
        fs::create_dir_all(&snapshot).expect("snapshot directory");

        assert_eq!(
            resolve_existing_zfs_snapshot_selection(
                root.path(),
                "pool/dataset/.zfs/snapshot/nightly/files"
            )
            .expect("named snapshot"),
            snapshot.canonicalize().expect("canonical snapshot")
        );
        assert!(matches!(
            resolve_existing_zfs_snapshot_selection(root.path(), "pool/dataset/live"),
            Err(ManifestError::NotNamedZfsSnapshot(_))
        ));

        let link = root
            .path()
            .join("pool/dataset/.zfs/snapshot/nightly/live-link");
        symlink(&live, &link).expect("snapshot-to-live symlink");
        assert!(matches!(
            resolve_existing_zfs_snapshot_selection(
                root.path(),
                "pool/dataset/.zfs/snapshot/nightly/live-link"
            ),
            Err(ManifestError::NotNamedZfsSnapshot(_))
        ));
    }

    #[test]
    fn restore_creation_rejects_a_symlink_replaced_after_planning() {
        let root = tempdir().expect("root");
        let outside = tempdir().expect("outside");
        fs::create_dir(root.path().join("exercise")).expect("initial restore parent");
        let run_id = uuid::Uuid::now_v7();
        isolated_restore_destination(root.path(), "exercise", run_id).expect("safe plan");
        fs::remove_dir(root.path().join("exercise")).expect("remove original parent");
        symlink(outside.path(), root.path().join("exercise")).expect("replace with symlink");

        assert!(create_isolated_restore_destination(root.path(), "exercise", run_id).is_err());
        assert!(outside
            .path()
            .read_dir()
            .expect("outside remains readable")
            .next()
            .is_none());
    }

    #[test]
    fn unsafe_source_symlink_target_is_rejected() {
        let root = tempdir().expect("root");
        symlink("../../outside", root.path().join("escape")).expect("escape symlink");
        assert!(matches!(
            build_manifest(root.path()),
            Err(ManifestError::UnsafeArchivePath(path)) if path == "escape"
        ));
    }

    #[test]
    fn persisted_manifest_is_immutable_and_integrity_checked() {
        let source = tempdir().expect("source");
        fs::write(source.path().join("evidence.txt"), b"recoverable").expect("file");
        let manifest = build_manifest(source.path()).expect("manifest");
        let evidence = tempdir().expect("evidence");

        let artifact =
            persist_manifest(evidence.path(), "source-manifest", &manifest).expect("persist");
        assert_eq!(
            load_manifest(&artifact.storage_path).expect("load"),
            manifest
        );
        assert_eq!(artifact.entry_count, 2);
        assert!(matches!(
            persist_manifest(evidence.path(), "source-manifest", &manifest),
            Err(ManifestError::ArtifactExists(_))
        ));

        let mut corrupted = manifest;
        corrupted.logical_bytes += 1;
        fs::write(
            evidence.path().join("corrupted.json"),
            serde_json::to_vec(&corrupted).expect("serialize"),
        )
        .expect("corrupt");
        assert!(matches!(
            load_manifest(&evidence.path().join("corrupted.json")),
            Err(ManifestError::Integrity("logical byte count mismatch"))
        ));
    }

    #[test]
    fn authenticated_manifest_bytes_are_decoded_without_reopening_the_path() {
        let source = tempdir().expect("source");
        fs::write(source.path().join("evidence.txt"), b"recoverable").expect("file");
        let manifest = build_manifest(source.path()).expect("manifest");
        let evidence = tempdir().expect("evidence");
        let artifact =
            persist_manifest(evidence.path(), "source-manifest", &manifest).expect("persist");
        let authenticated_bytes = fs::read(&artifact.storage_path).expect("artifact bytes");

        // Simulate a pathname replacement after the caller has authenticated a
        // byte snapshot. Decoding that snapshot remains deterministic and does
        // not consult the now-invalid pathname.
        fs::write(&artifact.storage_path, b"not a manifest").expect("replace pathname contents");
        assert_eq!(
            load_manifest_bytes(&authenticated_bytes).expect("authenticated snapshot"),
            manifest
        );
        assert!(load_manifest(&artifact.storage_path).is_err());
    }

    #[test]
    fn manifest_byte_decoder_enforces_the_entry_limit() {
        let source = tempdir().expect("source");
        fs::write(source.path().join("one.txt"), b"one").expect("file");
        let manifest = build_manifest(source.path()).expect("manifest");
        assert_eq!(manifest.entry_count, 2);
        let bytes = serde_json::to_vec(&manifest).expect("serialize manifest");

        assert!(matches!(
            load_manifest_bytes_with_limit(&bytes, 1),
            Err(ManifestError::Integrity(
                "entry count exceeds product limit"
            ))
        ));
        assert_eq!(
            load_manifest_bytes_with_limit(&bytes, 2).expect("limit permits manifest"),
            manifest
        );
    }

    #[test]
    fn kernel_mount_flags_control_read_only_proof() {
        assert!(mount_flags_are_read_only(StatVfsMountFlags::RDONLY));
        assert!(!mount_flags_are_read_only(StatVfsMountFlags::empty()));

        let writable = tempdir().expect("writable directory");
        let descriptor = open(
            writable.path(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .expect("directory descriptor");
        assert!(!descriptor_is_on_read_only_mount(&descriptor)
            .expect("kernel should report mount flags"));
    }
}
