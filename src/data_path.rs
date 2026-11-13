//! Deterministic file-tree evidence for physical backup and restore exercises.
//!
//! This module deliberately does not follow symlinks. A successful comparison
//! proves the file content and the metadata fields represented here; extended
//! attributes and ACL semantics remain an explicit later qualification gate.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    fs::{self, File, Metadata},
    io::{self, BufRead, BufReader, BufWriter, Read, Write},
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
pub const MAX_RESTORE_SELECTION_PATHS: usize = 256;
const STREAMING_MANIFEST_SORT_CHUNK_ENTRIES: usize = 1_024;

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
    #[error("backup manifests require UTF-8 path and symlink names: {0}")]
    NonUtf8Path(PathBuf),
    #[error("unsafe archive path or symlink target at {0}")]
    UnsafeArchivePath(String),
    #[error("nested filesystem boundary is outside the selected one-filesystem capture at {0}")]
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
    #[error("a selected-path restore requires at least one retained path")]
    EmptyRestoreSelection,
    #[error("a selected-path restore accepts at most {limit} requested paths (received {actual})")]
    RestoreSelectionTooLarge { limit: usize, actual: usize },
    #[error("restore selection path is not a safe retained relative path: {0}")]
    InvalidRestoreSelectionPath(String),
    #[error("restore selection path does not exist in the retained manifest: {0}")]
    RestoreSelectionPathNotFound(String),
    #[error("the resolved restore selection no longer matches its retained manifest")]
    RestoreSelectionMismatch,
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

/// Client intent for one immutable restore plan.  The API accepts this shape,
/// resolves it against an authenticated retained manifest, and persists only
/// the corresponding [`ResolvedManifestSelection`] as execution authority.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ManifestSelectionRequest {
    CompletePoint {
        #[serde(default)]
        paths: Vec<String>,
    },
    SelectedPaths {
        paths: Vec<String>,
    },
}

impl Default for ManifestSelectionRequest {
    fn default() -> Self {
        Self::CompletePoint { paths: Vec::new() }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestSelectionKind {
    CompletePoint,
    SelectedPaths,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestSelectionRoot {
    pub path: String,
    pub kind: EntryKind,
    pub implicit_hard_link: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestSelectionSummary {
    pub path_count: usize,
    pub entry_count: u64,
    pub file_count: u64,
    pub logical_bytes: u64,
}

/// Server-resolved, content-addressed restore scope.  `normalized_paths`
/// includes every effective root, including hard-link peers that must travel
/// together to preserve the retained link relationship.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResolvedManifestSelection {
    pub kind: ManifestSelectionKind,
    pub requested_paths: Vec<String>,
    pub normalized_paths: Vec<String>,
    pub implicit_hard_link_paths: Vec<String>,
    pub roots: Vec<ManifestSelectionRoot>,
    pub recursive_directories: bool,
    pub manifest_digest: String,
    pub selected_manifest_digest: String,
    pub selection_hash: String,
    pub summary: ManifestSelectionSummary,
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
    pub source_stability_digest_sha256: String,
    pub entry_count: u64,
    pub file_count: u64,
    pub logical_bytes: u64,
    pub coverage: ManifestCoverage,
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

/// Inventory and persist a manifest without retaining the complete file tree in memory.
///
/// This is the migration path for operational backups whose file count or logical size is
/// larger than the original G0.2 laboratory envelope. Entries are captured into fixed-size
/// sorted runs on the evidence filesystem and merged with at most two entries resident at a
/// time. The emitted artifact remains `tapescale-file-manifest-v2`, so existing recovery tools
/// can consume it once their own whole-file read bounds are replaced with streaming readers.
/// No total entry-count or logical-byte policy limit is applied; only integer overflow, local
/// evidence-storage exhaustion, cancellation, and source-integrity failures can stop the scan.
pub fn build_and_persist_manifest_with_cancel(
    root: &Path,
    run_directory: &Path,
    artifact_name: &str,
    cancellation: &AtomicBool,
) -> Result<ManifestArtifactSummary, ManifestError> {
    validate_artifact_name(artifact_name)?;
    fs::create_dir_all(run_directory).map_err(|source| ManifestError::Io {
        path: run_directory.to_owned(),
        source,
    })?;
    let destination = run_directory.join(format!("{artifact_name}.json"));
    if destination.exists() {
        return Err(ManifestError::ArtifactExists(destination));
    }

    let workspace = StreamingWorkspace::new(run_directory, artifact_name)?;
    let mut sorter =
        RawEntryExternalSorter::new(workspace.path(), STREAMING_MANIFEST_SORT_CHUNK_ENTRIES);
    scan_raw_entries_to_sorter(root, cancellation, &mut sorter)?;
    let sorted_entries = sorter.finish()?;
    write_streaming_manifest_artifact(
        &sorted_entries,
        workspace.path(),
        &destination,
        cancellation,
    )
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
        source_stability_digest_sha256: manifest.source_stability_digest_sha256.clone(),
        entry_count: manifest.entry_count,
        file_count: manifest.file_count,
        logical_bytes: manifest.logical_bytes,
        coverage: manifest.coverage.clone(),
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
    validate_artifact_name(artifact_name)?;
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

fn validate_artifact_name(artifact_name: &str) -> Result<(), ManifestError> {
    if artifact_name.is_empty()
        || artifact_name.len() > 64
        || !artifact_name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        Err(ManifestError::InvalidRelativePath)
    } else {
        Ok(())
    }
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
    // The G0.2 fixture builder remains deliberately bounded, but retained
    // operational manifests must not inherit that laboratory entry ceiling.
    // Callers already impose an authenticated artifact-byte bound before this
    // decoder is reached, so use the serialized artifact as the resource
    // authority rather than rejecting a valid inventory at 100,000 entries.
    load_manifest_bytes_with_limit(bytes, u64::MAX)
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

/// Resolve client restore intent against one authenticated retained manifest.
///
/// Paths are normalized and deduplicated by the server. Selecting a directory
/// includes its complete retained subtree. Selecting any member of a hard-link
/// group also includes every retained peer so a selective restore cannot turn
/// a proven link relationship into unrelated file data.
pub fn resolve_manifest_selection(
    manifest: &FileManifest,
    request: &ManifestSelectionRequest,
) -> Result<ResolvedManifestSelection, ManifestError> {
    validate_selection_manifest(manifest)?;
    match request {
        ManifestSelectionRequest::CompletePoint { paths } => {
            if !paths.is_empty() {
                return Err(ManifestError::InvalidRestoreSelectionPath(
                    "complete_point does not accept paths".to_owned(),
                ));
            }
            resolved_selection(
                ManifestSelectionKind::CompletePoint,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                manifest,
            )
        }
        ManifestSelectionRequest::SelectedPaths { paths } => {
            resolve_selected_manifest_paths(manifest, paths)
        }
    }
}

fn resolve_selected_manifest_paths(
    manifest: &FileManifest,
    paths: &[String],
) -> Result<ResolvedManifestSelection, ManifestError> {
    if paths.is_empty() {
        return Err(ManifestError::EmptyRestoreSelection);
    }
    if paths.len() > MAX_RESTORE_SELECTION_PATHS {
        return Err(ManifestError::RestoreSelectionTooLarge {
            limit: MAX_RESTORE_SELECTION_PATHS,
            actual: paths.len(),
        });
    }

    let by_path = manifest
        .entries
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    let mut requested = BTreeSet::new();
    for path in paths {
        let normalized = normalize_restore_selection_path(path)?;
        if !by_path.contains_key(normalized.as_str()) {
            return Err(ManifestError::RestoreSelectionPathNotFound(normalized));
        }
        requested.insert(normalized);
    }
    let requested_paths = requested.into_iter().collect::<Vec<_>>();

    // Consider shallow candidates first so a selected directory can subsume
    // every redundant descendant even when the request arrived out of order.
    let mut candidates = requested_paths.clone();
    candidates.sort_by(|left, right| {
        Path::new(left)
            .components()
            .count()
            .cmp(&Path::new(right).components().count())
            .then_with(|| left.cmp(right))
    });
    let mut roots = Vec::<ManifestSelectionRoot>::new();
    for path in candidates {
        if roots.iter().any(|root| {
            root.kind == EntryKind::Directory && manifest_path_within(&path, &root.path)
        }) {
            continue;
        }
        let entry = by_path
            .get(path.as_str())
            .expect("requested paths were checked above");
        roots.push(ManifestSelectionRoot {
            path,
            kind: entry.kind,
            implicit_hard_link: false,
        });
    }

    let selected_groups = manifest
        .entries
        .iter()
        .filter(|entry| selection_roots_contain(&roots, &entry.path))
        .filter_map(|entry| entry.hard_link_group.as_deref())
        .collect::<BTreeSet<_>>();
    let mut implicit_hard_link_paths = Vec::new();
    for entry in &manifest.entries {
        if entry.kind != EntryKind::File
            || entry
                .hard_link_group
                .as_deref()
                .is_none_or(|group| !selected_groups.contains(group))
            || selection_roots_contain(&roots, &entry.path)
        {
            continue;
        }
        implicit_hard_link_paths.push(entry.path.clone());
        roots.push(ManifestSelectionRoot {
            path: entry.path.clone(),
            kind: EntryKind::File,
            implicit_hard_link: true,
        });
    }
    roots.sort_by(|left, right| left.path.cmp(&right.path));
    implicit_hard_link_paths.sort();

    resolved_selection(
        ManifestSelectionKind::SelectedPaths,
        requested_paths,
        roots,
        implicit_hard_link_paths,
        manifest,
    )
}

fn resolved_selection(
    kind: ManifestSelectionKind,
    requested_paths: Vec<String>,
    roots: Vec<ManifestSelectionRoot>,
    implicit_hard_link_paths: Vec<String>,
    manifest: &FileManifest,
) -> Result<ResolvedManifestSelection, ManifestError> {
    let normalized_paths = roots
        .iter()
        .map(|root| root.path.clone())
        .collect::<Vec<_>>();
    let (entry_count, file_count, logical_bytes, selected_manifest_digest) = match kind {
        ManifestSelectionKind::CompletePoint => (
            manifest.entry_count,
            manifest.file_count,
            manifest.logical_bytes,
            manifest.digest_sha256.clone(),
        ),
        ManifestSelectionKind::SelectedPaths => {
            // The projected manifest retains `.` as a neutral comparison root.
            // Including it here keeps the summary identical to the exact
            // manifest artifact used for verification.
            let mut entry_count = 1_u64;
            let mut file_count = 0_u64;
            let mut logical_bytes = 0_u64;
            for entry in manifest
                .entries
                .iter()
                .filter(|entry| entry.path != "." && selection_roots_contain(&roots, &entry.path))
            {
                entry_count = entry_count
                    .checked_add(1)
                    .ok_or(ManifestError::Integrity("selection entry count overflow"))?;
                if entry.kind == EntryKind::File {
                    file_count = file_count
                        .checked_add(1)
                        .ok_or(ManifestError::Integrity("selection file count overflow"))?;
                }
                logical_bytes = logical_bytes.checked_add(entry.size.unwrap_or(0)).ok_or(
                    ManifestError::Integrity("selection logical byte count overflow"),
                )?;
            }
            let selected_entries = manifest
                .entries
                .iter()
                .filter(|entry| entry.path == "." || selection_roots_contain(&roots, &entry.path))
                .cloned()
                .collect::<Vec<_>>();
            let selected_manifest_digest =
                manifest_from_scoped_entries(selected_entries)?.digest_sha256;
            (
                entry_count,
                file_count,
                logical_bytes,
                selected_manifest_digest,
            )
        }
    };
    let summary = ManifestSelectionSummary {
        path_count: normalized_paths.len(),
        entry_count,
        file_count,
        logical_bytes,
    };
    let mut resolved = ResolvedManifestSelection {
        kind,
        requested_paths,
        normalized_paths,
        implicit_hard_link_paths,
        roots,
        recursive_directories: true,
        manifest_digest: manifest.digest_sha256.clone(),
        selected_manifest_digest,
        selection_hash: String::new(),
        summary,
    };
    resolved.selection_hash = resolved_selection_hash(&resolved)?;
    Ok(resolved)
}

#[derive(Serialize)]
struct SelectionHashMaterial<'a> {
    format: &'static str,
    kind: ManifestSelectionKind,
    requested_paths: &'a [String],
    normalized_paths: &'a [String],
    implicit_hard_link_paths: &'a [String],
    roots: &'a [ManifestSelectionRoot],
    recursive_directories: bool,
    manifest_digest: &'a str,
    selected_manifest_digest: &'a str,
    summary: &'a ManifestSelectionSummary,
}

fn resolved_selection_hash(selection: &ResolvedManifestSelection) -> Result<String, ManifestError> {
    let material = SelectionHashMaterial {
        format: "tapescale-restore-selection-v1",
        kind: selection.kind,
        requested_paths: &selection.requested_paths,
        normalized_paths: &selection.normalized_paths,
        implicit_hard_link_paths: &selection.implicit_hard_link_paths,
        roots: &selection.roots,
        recursive_directories: selection.recursive_directories,
        manifest_digest: &selection.manifest_digest,
        selected_manifest_digest: &selection.selected_manifest_digest,
        summary: &selection.summary,
    };
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&material)?)
    ))
}

/// Produce the exact expected manifest for an immutable restore scope.
/// Selected manifests retain the original relative paths and a neutral root;
/// callers can therefore verify several disjoint selections in one scan.
pub fn project_manifest_selection(
    manifest: &FileManifest,
    selection: &ResolvedManifestSelection,
) -> Result<FileManifest, ManifestError> {
    ensure_resolved_selection(manifest, selection)?;
    if selection.kind == ManifestSelectionKind::CompletePoint {
        return Ok(manifest.clone());
    }
    let entries = manifest
        .entries
        .iter()
        .filter(|entry| entry.path == "." || selection_roots_contain(&selection.roots, &entry.path))
        .cloned()
        .collect::<Vec<_>>();
    let projected = manifest_from_scoped_entries(entries)?;
    if projected.entry_count != selection.summary.entry_count
        || projected.file_count != selection.summary.file_count
        || projected.logical_bytes != selection.summary.logical_bytes
        || projected.digest_sha256 != selection.selected_manifest_digest
    {
        return Err(ManifestError::RestoreSelectionMismatch);
    }
    Ok(projected)
}

/// Compare a restored tree with exactly the approved scope.
///
/// Bacula may materialize parent directories needed to reach individually
/// selected objects. Those directory scaffolds are ignored, but any other
/// out-of-scope object is retained in the observed projection and therefore
/// reported as unexpected by the ordinary manifest comparator.
pub fn compare_manifest_selection(
    expected: &FileManifest,
    actual: &FileManifest,
    selection: &ResolvedManifestSelection,
) -> Result<ManifestComparison, ManifestError> {
    ensure_resolved_selection(expected, selection)?;
    validate_selection_manifest(actual)?;
    if selection.kind == ManifestSelectionKind::CompletePoint {
        return Ok(compare_manifests(expected, actual));
    }
    let projected_expected = project_manifest_selection(expected, selection)?;
    let expected_root = projected_expected
        .entries
        .iter()
        .find(|entry| entry.path == ".")
        .cloned()
        .ok_or(ManifestError::Integrity("manifest root entry is missing"))?;
    let mut observed_entries = vec![expected_root];
    for entry in actual.entries.iter().filter(|entry| entry.path != ".") {
        let selected = selection_roots_contain(&selection.roots, &entry.path);
        let scaffold = entry.kind == EntryKind::Directory
            && selection.roots.iter().any(|root| {
                root.path != entry.path && manifest_path_within(&root.path, &entry.path)
            });
        if selected || !scaffold {
            observed_entries.push(entry.clone());
        }
    }
    let projected_actual = manifest_from_scoped_entries(observed_entries)?;
    Ok(compare_manifests(&projected_expected, &projected_actual))
}

fn ensure_resolved_selection(
    manifest: &FileManifest,
    selection: &ResolvedManifestSelection,
) -> Result<(), ManifestError> {
    let request = match selection.kind {
        ManifestSelectionKind::CompletePoint => {
            ManifestSelectionRequest::CompletePoint { paths: Vec::new() }
        }
        ManifestSelectionKind::SelectedPaths => ManifestSelectionRequest::SelectedPaths {
            paths: selection.requested_paths.clone(),
        },
    };
    let resolved = resolve_manifest_selection(manifest, &request)?;
    if &resolved == selection {
        Ok(())
    } else {
        Err(ManifestError::RestoreSelectionMismatch)
    }
}

fn validate_selection_manifest(manifest: &FileManifest) -> Result<(), ManifestError> {
    if manifest.format != MANIFEST_FORMAT
        || manifest.entry_count != manifest.entries.len() as u64
        || manifest.file_count
            != manifest
                .entries
                .iter()
                .filter(|entry| entry.kind == EntryKind::File)
                .count() as u64
        || manifest.logical_bytes
            != manifest
                .entries
                .iter()
                .filter_map(|entry| entry.size)
                .sum::<u64>()
        || manifest.coverage != ManifestCoverage::default()
        || manifest.digest_sha256 != manifest_digest(&manifest.entries)?
    {
        return Err(ManifestError::Integrity(
            "manifest counters, coverage, or digest are invalid",
        ));
    }
    let mut previous: Option<&str> = None;
    let mut root_seen = false;
    for entry in &manifest.entries {
        if previous.is_some_and(|value| value >= entry.path.as_str()) {
            return Err(ManifestError::Integrity(
                "manifest paths are duplicated or not strictly sorted",
            ));
        }
        previous = Some(&entry.path);
        if entry.path == "." {
            if root_seen || entry.kind != EntryKind::Directory {
                return Err(ManifestError::Integrity("manifest root entry is invalid"));
            }
            root_seen = true;
            continue;
        }
        if entry.path.chars().any(char::is_control) {
            return Err(ManifestError::UnsafeArchivePath(entry.path.clone()));
        }
        let safe = validate_archive_path(Path::new(&entry.path))
            .map_err(|_| ManifestError::UnsafeArchivePath(entry.path.clone()))?;
        if safe.as_path().to_str() != Some(entry.path.as_str()) {
            return Err(ManifestError::UnsafeArchivePath(entry.path.clone()));
        }
        if let Some(target) = &entry.symlink_target {
            resolve_link_target(&safe, Path::new(target))
                .map_err(|_| ManifestError::UnsafeArchivePath(entry.path.clone()))?;
        }
    }
    if !root_seen {
        return Err(ManifestError::Integrity("manifest root entry is missing"));
    }
    Ok(())
}

fn normalize_restore_selection_path(value: &str) -> Result<String, ManifestError> {
    if value.is_empty() || value == "." || value.chars().any(char::is_control) {
        return Err(ManifestError::InvalidRestoreSelectionPath(value.to_owned()));
    }
    let safe = validate_archive_path(Path::new(value))
        .map_err(|_| ManifestError::InvalidRestoreSelectionPath(value.to_owned()))?;
    let normalized = safe
        .as_path()
        .to_str()
        .ok_or_else(|| ManifestError::InvalidRestoreSelectionPath(value.to_owned()))?
        .to_owned();
    if normalized == "." {
        Err(ManifestError::InvalidRestoreSelectionPath(value.to_owned()))
    } else {
        Ok(normalized)
    }
}

fn manifest_path_within(path: &str, directory: &str) -> bool {
    Path::new(path).starts_with(Path::new(directory))
}

fn selection_roots_contain(roots: &[ManifestSelectionRoot], path: &str) -> bool {
    roots.iter().any(|root| {
        root.path == path
            || (root.kind == EntryKind::Directory && manifest_path_within(path, &root.path))
    })
}

fn manifest_from_scoped_entries(
    mut entries: Vec<ManifestEntry>,
) -> Result<FileManifest, ManifestError> {
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    let mut canonical_groups = BTreeMap::<String, String>::new();
    let mut next_group = 1_u64;
    for entry in &mut entries {
        if let Some(group) = entry.hard_link_group.clone() {
            entry.hard_link_group = Some(
                canonical_groups
                    .entry(group)
                    .or_insert_with(|| {
                        let value = format!("link-{next_group}");
                        next_group += 1;
                        value
                    })
                    .clone(),
            );
        }
    }
    let entry_count = entries.len() as u64;
    let file_count = entries
        .iter()
        .filter(|entry| entry.kind == EntryKind::File)
        .count() as u64;
    let logical_bytes = entries.iter().filter_map(|entry| entry.size).sum();
    let digest_sha256 = manifest_digest(&entries)?;
    Ok(FileManifest {
        format: MANIFEST_FORMAT.to_owned(),
        entries,
        entry_count,
        file_count,
        logical_bytes,
        digest_sha256,
        coverage: ManifestCoverage::default(),
        source_stability_digest_sha256: String::new(),
    })
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

#[derive(Debug, Deserialize, Serialize)]
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

struct StreamingWorkspace {
    path: PathBuf,
}

impl StreamingWorkspace {
    fn new(run_directory: &Path, artifact_name: &str) -> Result<Self, ManifestError> {
        let path = run_directory.join(format!(
            ".{artifact_name}.{}.manifest-work",
            uuid::Uuid::now_v7()
        ));
        fs::create_dir(&path).map_err(|source| ManifestError::Io {
            path: path.clone(),
            source,
        })?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for StreamingWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct RawEntryExternalSorter {
    workspace: PathBuf,
    chunk_entries: usize,
    buffer: Vec<RawEntry>,
    levels: Vec<Option<PathBuf>>,
    sequence: u64,
    max_buffered_entries: usize,
}

impl RawEntryExternalSorter {
    fn new(workspace: &Path, chunk_entries: usize) -> Self {
        debug_assert!(chunk_entries > 0);
        Self {
            workspace: workspace.to_owned(),
            chunk_entries,
            buffer: Vec::with_capacity(chunk_entries),
            levels: Vec::new(),
            sequence: 0,
            max_buffered_entries: 0,
        }
    }

    fn push(&mut self, entry: RawEntry) -> Result<(), ManifestError> {
        self.buffer.push(entry);
        self.max_buffered_entries = self.max_buffered_entries.max(self.buffer.len());
        if self.buffer.len() == self.chunk_entries {
            self.flush_buffer()?;
        }
        Ok(())
    }

    fn finish(mut self) -> Result<PathBuf, ManifestError> {
        self.flush_buffer()?;
        let mut merged: Option<PathBuf> = None;
        for path in self.levels.into_iter().flatten() {
            merged = Some(match merged {
                None => path,
                Some(left) => {
                    let destination =
                        next_sort_path(&self.workspace, &mut self.sequence, "final-merge")?;
                    merge_raw_entry_files(&left, &path, &destination)?;
                    remove_streaming_file(&left)?;
                    remove_streaming_file(&path)?;
                    destination
                }
            });
        }
        merged.ok_or(ManifestError::Integrity(
            "streaming manifest produced no root entry",
        ))
    }

    fn flush_buffer(&mut self) -> Result<(), ManifestError> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        self.buffer
            .sort_by(|left, right| left.path.cmp(&right.path));
        if self
            .buffer
            .windows(2)
            .any(|entries| entries[0].path == entries[1].path)
        {
            return Err(ManifestError::Integrity(
                "streaming manifest contains duplicate paths",
            ));
        }
        let path = next_sort_path(&self.workspace, &mut self.sequence, "run")?;
        write_raw_entry_file(&path, &self.buffer)?;
        self.buffer.clear();
        self.insert_run(path, 0)
    }

    fn insert_run(&mut self, mut path: PathBuf, mut level: usize) -> Result<(), ManifestError> {
        loop {
            if self.levels.len() <= level {
                self.levels.resize_with(level + 1, || None);
            }
            let Some(existing) = self.levels[level].take() else {
                self.levels[level] = Some(path);
                return Ok(());
            };
            let merged = next_sort_path(&self.workspace, &mut self.sequence, "merge")?;
            merge_raw_entry_files(&existing, &path, &merged)?;
            remove_streaming_file(&existing)?;
            remove_streaming_file(&path)?;
            path = merged;
            level = level
                .checked_add(1)
                .ok_or(ManifestError::Integrity("streaming sort level overflow"))?;
        }
    }
}

fn next_sort_path(
    workspace: &Path,
    sequence: &mut u64,
    kind: &str,
) -> Result<PathBuf, ManifestError> {
    let current = *sequence;
    *sequence = sequence
        .checked_add(1)
        .ok_or(ManifestError::Integrity("streaming sort sequence overflow"))?;
    Ok(workspace.join(format!("{kind}-{current:020}.jsonl")))
}

fn write_raw_entry_file(path: &Path, entries: &[RawEntry]) -> Result<(), ManifestError> {
    let file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|source| ManifestError::Io {
            path: path.to_owned(),
            source,
        })?;
    let mut writer = BufWriter::new(file);
    for entry in entries {
        serde_json::to_writer(&mut writer, entry)?;
        writer
            .write_all(b"\n")
            .map_err(|source| ManifestError::Io {
                path: path.to_owned(),
                source,
            })?;
    }
    writer.flush().map_err(|source| ManifestError::Io {
        path: path.to_owned(),
        source,
    })?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|source| ManifestError::Io {
            path: path.to_owned(),
            source,
        })
}

fn merge_raw_entry_files(
    left: &Path,
    right: &Path,
    destination: &Path,
) -> Result<(), ManifestError> {
    let mut left_reader = open_streaming_reader(left)?;
    let mut right_reader = open_streaming_reader(right)?;
    let output = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)
        .map_err(|source| ManifestError::Io {
            path: destination.to_owned(),
            source,
        })?;
    let mut writer = BufWriter::new(output);
    let mut left_line = String::new();
    let mut right_line = String::new();
    let mut left_entry = read_raw_entry(&mut left_reader, left, &mut left_line)?;
    let mut right_entry = read_raw_entry(&mut right_reader, right, &mut right_line)?;
    while left_entry.is_some() || right_entry.is_some() {
        let take_left = match (&left_entry, &right_entry) {
            (Some(left), Some(right)) if left.path == right.path => {
                return Err(ManifestError::Integrity(
                    "streaming manifest contains duplicate paths",
                ))
            }
            (Some(left), Some(right)) => left.path < right.path,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => break,
        };
        if take_left {
            let entry = left_entry
                .take()
                .expect("left merge entry is present when selected");
            write_raw_entry(&mut writer, destination, &entry)?;
            left_entry = read_raw_entry(&mut left_reader, left, &mut left_line)?;
        } else {
            let entry = right_entry
                .take()
                .expect("right merge entry is present when selected");
            write_raw_entry(&mut writer, destination, &entry)?;
            right_entry = read_raw_entry(&mut right_reader, right, &mut right_line)?;
        }
    }
    writer.flush().map_err(|source| ManifestError::Io {
        path: destination.to_owned(),
        source,
    })?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|source| ManifestError::Io {
            path: destination.to_owned(),
            source,
        })
}

fn open_streaming_reader(path: &Path) -> Result<BufReader<File>, ManifestError> {
    File::open(path)
        .map(BufReader::new)
        .map_err(|source| ManifestError::Io {
            path: path.to_owned(),
            source,
        })
}

fn read_raw_entry(
    reader: &mut impl BufRead,
    path: &Path,
    line: &mut String,
) -> Result<Option<RawEntry>, ManifestError> {
    line.clear();
    let read = reader.read_line(line).map_err(|source| ManifestError::Io {
        path: path.to_owned(),
        source,
    })?;
    if read == 0 {
        return Ok(None);
    }
    let encoded = line.trim_end_matches(['\r', '\n']);
    serde_json::from_str(encoded)
        .map(Some)
        .map_err(ManifestError::Serialization)
}

fn write_raw_entry(
    writer: &mut impl Write,
    path: &Path,
    entry: &RawEntry,
) -> Result<(), ManifestError> {
    serde_json::to_writer(&mut *writer, entry)?;
    writer.write_all(b"\n").map_err(|source| ManifestError::Io {
        path: path.to_owned(),
        source,
    })
}

fn remove_streaming_file(path: &Path) -> Result<(), ManifestError> {
    fs::remove_file(path).map_err(|source| ManifestError::Io {
        path: path.to_owned(),
        source,
    })
}

fn scan_raw_entries_to_sorter(
    root: &Path,
    cancellation: &AtomicBool,
    sorter: &mut RawEntryExternalSorter,
) -> Result<(), ManifestError> {
    let root = root.canonicalize().map_err(|source| ManifestError::Io {
        path: root.to_owned(),
        source,
    })?;
    if !root.is_dir() {
        return Err(ManifestError::InvalidRoot(root));
    }
    let root_metadata = fs::symlink_metadata(&root).map_err(|source| ManifestError::Io {
        path: root.clone(),
        source,
    })?;
    let root_device = root_metadata.dev();
    sorter.push(RawEntry::capture(
        &root,
        Path::new(""),
        root_metadata,
        EntryKind::Directory,
        cancellation,
    )?)?;
    let root_directory = fs::read_dir(&root).map_err(|source| ManifestError::Io {
        path: root.clone(),
        source,
    })?;
    let mut directories = vec![(PathBuf::new(), root_directory)];
    while !directories.is_empty() {
        if cancellation.load(Ordering::Acquire) {
            return Err(ManifestError::Cancelled);
        }
        let next = {
            let (relative, directory) = directories
                .last_mut()
                .expect("non-empty directory stack has a final iterator");
            directory.next().map(|entry| (relative.clone(), entry))
        };
        let Some((parent, entry)) = next else {
            directories.pop();
            continue;
        };
        let entry = entry.map_err(|source| ManifestError::Io {
            path: root.join(&parent),
            source,
        })?;
        let relative = parent.join(entry.file_name());
        if relative.to_str().is_none() {
            return Err(ManifestError::NonUtf8Path(relative));
        }
        let absolute = root.join(&relative);
        let metadata = fs::symlink_metadata(&absolute).map_err(|source| ManifestError::Io {
            path: absolute.clone(),
            source,
        })?;
        ensure_same_filesystem(root_device, metadata.dev(), &relative)?;
        let kind = entry_kind(&metadata, &relative)?;
        sorter.push(RawEntry::capture(
            &absolute,
            &relative,
            metadata,
            kind,
            cancellation,
        )?)?;
        if kind == EntryKind::Directory {
            let directory = fs::read_dir(&absolute).map_err(|source| ManifestError::Io {
                path: absolute,
                source,
            })?;
            directories.push((relative, directory));
        }
    }
    Ok(())
}

fn write_streaming_manifest_artifact(
    sorted_entries: &Path,
    workspace: &Path,
    destination: &Path,
    cancellation: &AtomicBool,
) -> Result<ManifestArtifactSummary, ManifestError> {
    let hard_links = workspace.join("hard-links");
    fs::create_dir(&hard_links).map_err(|source| ManifestError::Io {
        path: hard_links.clone(),
        source,
    })?;
    let temporary = workspace.join("manifest.json");
    let output = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|source| ManifestError::Io {
            path: temporary.clone(),
            source,
        })?;
    let mut writer = BufWriter::new(output);
    writer
        .write_all(b"{\"format\":\"tapescale-file-manifest-v2\",\"entries\":[")
        .map_err(|source| ManifestError::Io {
            path: temporary.clone(),
            source,
        })?;
    let mut manifest_digest = Sha256::new();
    let mut stability_digest = Sha256::new();
    manifest_digest.update(b"[");
    stability_digest.update(b"[");
    let mut reader = open_streaming_reader(sorted_entries)?;
    let mut line = String::new();
    let mut entry_count = 0_u64;
    let mut file_count = 0_u64;
    let mut logical_bytes = 0_u64;
    let mut next_hard_link_group = 1_u64;
    let mut first = true;
    while let Some(raw) = read_raw_entry(&mut reader, sorted_entries, &mut line)? {
        if cancellation.load(Ordering::Acquire) {
            return Err(ManifestError::Cancelled);
        }
        let raw_bytes = serde_json::to_vec(&raw)?;
        let entry = manifest_entry_from_raw(raw, &hard_links, &mut next_hard_link_group)?;
        let entry_bytes = serde_json::to_vec(&entry)?;
        if !first {
            writer.write_all(b",").map_err(|source| ManifestError::Io {
                path: temporary.clone(),
                source,
            })?;
            manifest_digest.update(b",");
            stability_digest.update(b",");
        }
        first = false;
        writer
            .write_all(&entry_bytes)
            .map_err(|source| ManifestError::Io {
                path: temporary.clone(),
                source,
            })?;
        manifest_digest.update(&entry_bytes);
        stability_digest.update(&raw_bytes);
        entry_count = entry_count
            .checked_add(1)
            .ok_or(ManifestError::Integrity("manifest entry count overflow"))?;
        if entry.kind == EntryKind::File {
            file_count = file_count
                .checked_add(1)
                .ok_or(ManifestError::Integrity("manifest file count overflow"))?;
            logical_bytes = logical_bytes.checked_add(entry.size.unwrap_or(0)).ok_or(
                ManifestError::Integrity("manifest logical byte count overflow"),
            )?;
        }
    }
    manifest_digest.update(b"]");
    stability_digest.update(b"]");
    let digest_sha256 = format!("{:x}", manifest_digest.finalize());
    let source_stability_digest_sha256 = format!("{:x}", stability_digest.finalize());
    let coverage = serde_json::to_vec(&ManifestCoverage::default())?;
    let suffix = format!(
        "],\"entry_count\":{entry_count},\"file_count\":{file_count},\"logical_bytes\":{logical_bytes},\"digest_sha256\":\"{digest_sha256}\",\"coverage\":"
    );
    writer
        .write_all(suffix.as_bytes())
        .and_then(|()| writer.write_all(&coverage))
        .and_then(|()| writer.write_all(b"}"))
        .and_then(|()| writer.flush())
        .map_err(|source| ManifestError::Io {
            path: temporary.clone(),
            source,
        })?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|source| ManifestError::Io {
            path: temporary.clone(),
            source,
        })?;
    drop(writer);
    let (artifact_sha256, size_bytes) = digest_file(&temporary)?;
    if let Err(source) = fs::hard_link(&temporary, destination) {
        return if source.kind() == io::ErrorKind::AlreadyExists {
            Err(ManifestError::ArtifactExists(destination.to_owned()))
        } else {
            Err(ManifestError::Io {
                path: destination.to_owned(),
                source,
            })
        };
    }
    Ok(ManifestArtifactSummary {
        storage_path: destination.to_owned(),
        artifact_sha256,
        size_bytes,
        manifest_digest: digest_sha256,
        source_stability_digest_sha256,
        entry_count,
        file_count,
        logical_bytes,
        coverage: ManifestCoverage::default(),
    })
}

fn manifest_entry_from_raw(
    raw: RawEntry,
    hard_link_directory: &Path,
    next_hard_link_group: &mut u64,
) -> Result<ManifestEntry, ManifestError> {
    let hard_link_group = raw
        .hard_link_identity
        .map(|identity| hard_link_group(hard_link_directory, identity, next_hard_link_group))
        .transpose()?;
    Ok(ManifestEntry {
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
    })
}

fn hard_link_group(
    directory: &Path,
    (device, inode): (u64, u64),
    next_hard_link_group: &mut u64,
) -> Result<String, ManifestError> {
    let mapping = directory.join(format!("{device:016x}-{inode:016x}"));
    let group = format!("link-{}", *next_hard_link_group);
    match fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&mapping)
    {
        Ok(mut file) => {
            file.write_all(group.as_bytes())
                .and_then(|()| file.sync_all())
                .map_err(|source| ManifestError::Io {
                    path: mapping,
                    source,
                })?;
            *next_hard_link_group = next_hard_link_group
                .checked_add(1)
                .ok_or(ManifestError::Integrity("hard-link group count overflow"))?;
            Ok(group)
        }
        Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
            fs::read_to_string(&mapping).map_err(|source| ManifestError::Io {
                path: mapping,
                source,
            })
        }
        Err(source) => Err(ManifestError::Io {
            path: mapping,
            source,
        }),
    }
}

fn digest_file(path: &Path) -> Result<(String, u64), ManifestError> {
    let mut file = File::open(path).map_err(|source| ManifestError::Io {
        path: path.to_owned(),
        source,
    })?;
    let mut digest = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|source| ManifestError::Io {
            path: path.to_owned(),
            source,
        })?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        size = size
            .checked_add(read as u64)
            .ok_or(ManifestError::Integrity("manifest artifact size overflow"))?;
    }
    Ok((format!("{:x}", digest.finalize()), size))
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

    fn selection_entry(
        path: &str,
        kind: EntryKind,
        size: Option<u64>,
        hard_link_group: Option<&str>,
    ) -> ManifestEntry {
        ManifestEntry {
            path: path.to_owned(),
            kind,
            mode: if kind == EntryKind::Directory {
                0o755
            } else {
                0o640
            },
            uid: 1_000,
            gid: 1_000,
            mtime_seconds: 1_700_000_000,
            size,
            content_sha256: size.map(|value| format!("{value:064x}")),
            symlink_target: None,
            hard_link_group: hard_link_group.map(str::to_owned),
        }
    }

    fn selection_fixture_manifest() -> FileManifest {
        manifest_from_scoped_entries(vec![
            selection_entry(".", EntryKind::Directory, None, None),
            selection_entry("alpha", EntryKind::Directory, None, None),
            selection_entry("alpha/a.bin", EntryKind::File, Some(4), Some("source-pair")),
            selection_entry("alpha/solo.txt", EntryKind::File, Some(2), None),
            selection_entry("beta", EntryKind::Directory, None, None),
            selection_entry(
                "beta/a-link.bin",
                EntryKind::File,
                Some(4),
                Some("source-pair"),
            ),
            selection_entry("gamma.txt", EntryKind::File, Some(8), None),
        ])
        .expect("selection fixture")
    }

    #[test]
    fn selected_manifest_scope_is_recursive_deduplicated_and_hard_link_closed() {
        let manifest = selection_fixture_manifest();
        let request = ManifestSelectionRequest::SelectedPaths {
            paths: vec!["alpha/a.bin".to_owned(), "alpha".to_owned()],
        };
        let selection = resolve_manifest_selection(&manifest, &request).expect("selection");

        assert_eq!(selection.kind, ManifestSelectionKind::SelectedPaths);
        assert_eq!(selection.requested_paths, vec!["alpha", "alpha/a.bin"]);
        assert_eq!(selection.normalized_paths, vec!["alpha", "beta/a-link.bin"]);
        assert_eq!(selection.implicit_hard_link_paths, vec!["beta/a-link.bin"]);
        assert_eq!(selection.summary.path_count, 2);
        assert_eq!(selection.summary.entry_count, 5);
        assert_eq!(selection.summary.file_count, 3);
        assert_eq!(selection.summary.logical_bytes, 10);
        assert_eq!(selection.selection_hash.len(), 64);
        assert_eq!(selection.selected_manifest_digest.len(), 64);
        assert_ne!(
            selection.selected_manifest_digest,
            selection.manifest_digest
        );

        let projected = project_manifest_selection(&manifest, &selection).expect("projection");
        assert_eq!(projected.digest_sha256, selection.selected_manifest_digest);
        assert_eq!(projected.entry_count, selection.summary.entry_count);
        assert!(projected.entries.iter().all(|entry| entry.path == "."
            || entry.path == "alpha"
            || entry.path.starts_with("alpha/")
            || entry.path == "beta/a-link.bin"));

        let reordered = resolve_manifest_selection(
            &manifest,
            &ManifestSelectionRequest::SelectedPaths {
                paths: vec!["alpha".to_owned(), "alpha/a.bin".to_owned()],
            },
        )
        .expect("reordered selection");
        assert_eq!(selection, reordered);
    }

    #[test]
    fn selected_manifest_scope_rejects_empty_unsafe_missing_and_oversized_requests() {
        let manifest = selection_fixture_manifest();
        assert!(matches!(
            resolve_manifest_selection(
                &manifest,
                &ManifestSelectionRequest::SelectedPaths { paths: Vec::new() }
            ),
            Err(ManifestError::EmptyRestoreSelection)
        ));
        for path in [".", "../alpha", "/alpha", "alpha\nfile"] {
            assert!(matches!(
                resolve_manifest_selection(
                    &manifest,
                    &ManifestSelectionRequest::SelectedPaths {
                        paths: vec![path.to_owned()]
                    }
                ),
                Err(ManifestError::InvalidRestoreSelectionPath(_))
            ));
        }
        assert!(matches!(
            resolve_manifest_selection(
                &manifest,
                &ManifestSelectionRequest::SelectedPaths {
                    paths: vec!["missing".to_owned()]
                }
            ),
            Err(ManifestError::RestoreSelectionPathNotFound(path)) if path == "missing"
        ));
        assert!(matches!(
            resolve_manifest_selection(
                &manifest,
                &ManifestSelectionRequest::SelectedPaths {
                    paths: vec!["alpha".to_owned(); MAX_RESTORE_SELECTION_PATHS + 1]
                }
            ),
            Err(ManifestError::RestoreSelectionTooLarge { .. })
        ));
    }

    #[test]
    fn selective_comparison_ignores_only_required_directory_scaffolds() {
        let manifest = selection_fixture_manifest();
        let selection = resolve_manifest_selection(
            &manifest,
            &ManifestSelectionRequest::SelectedPaths {
                paths: vec!["alpha/solo.txt".to_owned()],
            },
        )
        .expect("selection");
        let mut root = manifest
            .entries
            .iter()
            .find(|entry| entry.path == ".")
            .expect("root")
            .clone();
        root.mode = 0o700;
        let mut scaffold = manifest
            .entries
            .iter()
            .find(|entry| entry.path == "alpha")
            .expect("alpha")
            .clone();
        scaffold.mode = 0o711;
        let selected = manifest
            .entries
            .iter()
            .find(|entry| entry.path == "alpha/solo.txt")
            .expect("selected")
            .clone();
        let actual = manifest_from_scoped_entries(vec![root, scaffold, selected.clone()])
            .expect("actual manifest");
        assert!(
            compare_manifest_selection(&manifest, &actual, &selection)
                .expect("comparison")
                .matched
        );

        let rogue = selection_entry("alpha/rogue.txt", EntryKind::File, Some(1), None);
        let actual_with_rogue = manifest_from_scoped_entries(vec![
            selection_entry(".", EntryKind::Directory, None, None),
            selection_entry("alpha", EntryKind::Directory, None, None),
            selected,
            rogue,
        ])
        .expect("rogue manifest");
        let comparison =
            compare_manifest_selection(&manifest, &actual_with_rogue, &selection).expect("compare");
        assert!(!comparison.matched);
        assert!(comparison
            .differences
            .iter()
            .any(|difference| difference.contains("unexpected restored path: alpha/rogue.txt")));
    }

    #[test]
    fn selective_comparison_canonicalizes_closed_hard_link_groups() {
        let manifest = selection_fixture_manifest();
        let selection = resolve_manifest_selection(
            &manifest,
            &ManifestSelectionRequest::SelectedPaths {
                paths: vec!["alpha/a.bin".to_owned()],
            },
        )
        .expect("selection");
        let mut first = manifest
            .entries
            .iter()
            .find(|entry| entry.path == "alpha/a.bin")
            .expect("first link")
            .clone();
        let mut second = manifest
            .entries
            .iter()
            .find(|entry| entry.path == "beta/a-link.bin")
            .expect("second link")
            .clone();
        first.hard_link_group = Some("restored-inode".to_owned());
        second.hard_link_group = Some("restored-inode".to_owned());
        let actual = manifest_from_scoped_entries(vec![
            selection_entry(".", EntryKind::Directory, None, None),
            selection_entry("alpha", EntryKind::Directory, None, None),
            first,
            selection_entry("beta", EntryKind::Directory, None, None),
            second,
        ])
        .expect("restored links");
        assert!(
            compare_manifest_selection(&manifest, &actual, &selection)
                .expect("comparison")
                .matched
        );
    }

    #[test]
    fn complete_point_selection_preserves_the_existing_comparison_contract() {
        let manifest = selection_fixture_manifest();
        let selection = resolve_manifest_selection(
            &manifest,
            &ManifestSelectionRequest::CompletePoint { paths: Vec::new() },
        )
        .expect("complete selection");
        assert_eq!(selection.manifest_digest, manifest.digest_sha256);
        assert_eq!(selection.selected_manifest_digest, manifest.digest_sha256);
        assert_eq!(
            compare_manifest_selection(&manifest, &manifest, &selection).expect("comparison"),
            compare_manifests(&manifest, &manifest)
        );
    }

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

    fn synthetic_file_entry(path: String, size: u64) -> RawEntry {
        RawEntry {
            path,
            kind: EntryKind::File,
            mode: 0o644,
            uid: 1_000,
            gid: 1_000,
            mtime_seconds: 1_700_000_000,
            mtime_nanoseconds: 123_456_789,
            size: Some(size),
            content_sha256: Some("0".repeat(64)),
            symlink_target: None,
            hard_link_identity: None,
            sparse: Some(size > 0),
        }
    }

    #[test]
    fn streaming_manifest_matches_the_legacy_v2_artifact() {
        let source = tempdir().expect("source");
        fs::create_dir(source.path().join("nested")).expect("directory");
        fs::write(source.path().join("nested/original"), b"recoverable").expect("file");
        fs::hard_link(
            source.path().join("nested/original"),
            source.path().join("linked"),
        )
        .expect("hard link");
        symlink("nested/original", source.path().join("shortcut")).expect("symlink");
        let legacy = build_manifest(source.path()).expect("legacy manifest");
        let evidence = tempdir().expect("evidence");
        let artifact = build_and_persist_manifest_with_cancel(
            source.path(),
            evidence.path(),
            "streamed",
            &AtomicBool::new(false),
        )
        .expect("streaming manifest");
        let streamed = load_manifest(&artifact.storage_path).expect("load streamed artifact");

        assert_eq!(streamed, legacy);
        assert_eq!(artifact.manifest_digest, legacy.digest_sha256);
        assert_eq!(
            artifact.source_stability_digest_sha256,
            legacy.source_stability_digest_sha256
        );
        assert_eq!(artifact.entry_count, legacy.entry_count);
        assert_eq!(artifact.logical_bytes, legacy.logical_bytes);
    }

    #[test]
    fn external_sorter_bounds_the_entry_buffer_and_orders_runs() {
        let workspace = tempdir().expect("workspace");
        let mut sorter = RawEntryExternalSorter::new(workspace.path(), 3);
        for index in (0..100).rev() {
            sorter
                .push(synthetic_file_entry(format!("file-{index:03}"), 0))
                .expect("spool entry");
        }
        assert!(sorter.max_buffered_entries <= 3);
        let sorted = sorter.finish().expect("finish sort");
        let mut reader = open_streaming_reader(&sorted).expect("open sorted run");
        let mut line = String::new();
        let mut previous = None;
        let mut count = 0;
        while let Some(entry) = read_raw_entry(&mut reader, &sorted, &mut line).expect("entry") {
            if let Some(previous) = &previous {
                assert!(previous < &entry.path);
            }
            previous = Some(entry.path);
            count += 1;
        }
        assert_eq!(count, 100);
    }

    #[test]
    fn streaming_artifact_has_no_total_entry_or_logical_byte_ceiling() {
        let evidence = tempdir().expect("evidence");
        let workspace = evidence.path().join("work");
        fs::create_dir(&workspace).expect("workspace");
        let sorted = workspace.join("sorted.jsonl");
        let file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&sorted)
            .expect("sorted run");
        let mut writer = BufWriter::new(file);
        let count = MAX_G02_MANIFEST_ENTRIES + 1;
        for index in 0..count {
            let size = if index == 0 {
                MAX_G02_LOGICAL_BYTES + 1
            } else {
                0
            };
            write_raw_entry(
                &mut writer,
                &sorted,
                &synthetic_file_entry(format!("file-{index:06}"), size),
            )
            .expect("write synthetic entry");
        }
        writer.flush().expect("flush sorted run");
        drop(writer);
        let destination = evidence.path().join("large.json");
        let summary = write_streaming_manifest_artifact(
            &sorted,
            &workspace,
            &destination,
            &AtomicBool::new(false),
        )
        .expect("large streaming manifest");

        assert_eq!(summary.entry_count, count);
        assert_eq!(summary.logical_bytes, MAX_G02_LOGICAL_BYTES + 1);
        assert!(summary.storage_path.is_file());
        assert_eq!(summary.artifact_sha256.len(), 64);
        assert_eq!(summary.manifest_digest.len(), 64);
        let loaded = load_manifest(&summary.storage_path)
            .expect("operational decoder must not inherit the G0.2 entry ceiling");
        assert_eq!(loaded.entry_count, count);
        assert_eq!(loaded.logical_bytes, MAX_G02_LOGICAL_BYTES + 1);
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
