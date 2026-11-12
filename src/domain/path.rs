//! Lexical archive-path containment rules.
//!
//! These checks reject hostile names before extraction.  Production writers
//! must additionally use directory-relative, no-follow operations (for
//! example `openat2` with `RESOLVE_BENEATH`) to prevent symlink races.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SafeRelativePath(PathBuf);

impl SafeRelativePath {
    pub fn as_path(&self) -> &Path {
        &self.0
    }

    pub fn into_path_buf(self) -> PathBuf {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathViolation {
    Empty,
    Absolute,
    ParentTraversal,
    RootEscapeThroughLink,
    AmbiguousSeparator,
    NulByte,
    HardLinkTargetNotPreviouslyExtracted,
}

pub fn validate_archive_path(path: &Path) -> Result<SafeRelativePath, PathViolation> {
    if path.as_os_str().is_empty() {
        return Err(PathViolation::Empty);
    }

    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => return Err(PathViolation::Absolute),
            Component::ParentDir => return Err(PathViolation::ParentTraversal),
            Component::CurDir => {}
            Component::Normal(part) => {
                validate_component(part.to_string_lossy().as_ref())?;
                parts.push(part.to_os_string());
            }
        }
    }
    if parts.is_empty() {
        return Err(PathViolation::Empty);
    }
    Ok(SafeRelativePath(parts.into_iter().collect()))
}

/// Resolves a relative symlink target from the directory containing `link` and
/// rejects any target that would escape the extraction root.
pub fn resolve_link_target(
    link: &SafeRelativePath,
    target: &Path,
) -> Result<SafeRelativePath, PathViolation> {
    if target.as_os_str().is_empty() {
        return Err(PathViolation::Empty);
    }

    let mut parts: Vec<OsString> = link
        .as_path()
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_os_string()),
            _ => None,
        })
        .collect();
    parts.pop();

    for component in target.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => return Err(PathViolation::Absolute),
            Component::CurDir => {}
            Component::ParentDir => {
                if parts.pop().is_none() {
                    return Err(PathViolation::RootEscapeThroughLink);
                }
            }
            Component::Normal(part) => {
                validate_component(part.to_string_lossy().as_ref())?;
                parts.push(part.to_os_string());
            }
        }
    }

    if parts.is_empty() {
        return Err(PathViolation::Empty);
    }
    Ok(SafeRelativePath(parts.into_iter().collect()))
}

/// Hard links are permitted only to a contained entry that has already been
/// materialized. This prevents forward-link tricks and links to host files.
pub fn validate_hard_link_target(
    target: &Path,
    previously_extracted: &BTreeSet<SafeRelativePath>,
) -> Result<SafeRelativePath, PathViolation> {
    let target = validate_archive_path(target)?;
    if previously_extracted.contains(&target) {
        Ok(target)
    } else {
        Err(PathViolation::HardLinkTargetNotPreviouslyExtracted)
    }
}

/// Produces the lexical destination for diagnostics/planning only. Extraction
/// must still use a race-safe directory handle rather than this joined path.
pub fn lexical_destination(root: &Path, relative: &SafeRelativePath) -> PathBuf {
    root.join(relative.as_path())
}

fn validate_component(component: &str) -> Result<(), PathViolation> {
    if component.contains('\0') {
        return Err(PathViolation::NulByte);
    }
    // On Unix a backslash is legal, but rejecting it prevents the same archive
    // from changing meaning when recovery is performed on another platform.
    if component.contains('\\') {
        return Err(PathViolation::AmbiguousSeparator);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_and_normalizes_a_contained_relative_path() {
        let safe = validate_archive_path(Path::new("datasets/./photos/image.raw")).unwrap();
        assert_eq!(safe.as_path(), Path::new("datasets/photos/image.raw"));
        assert_eq!(
            lexical_destination(Path::new("/restore"), &safe),
            PathBuf::from("/restore/datasets/photos/image.raw")
        );
    }

    #[test]
    fn rejects_absolute_parent_and_ambiguous_paths() {
        assert_eq!(
            validate_archive_path(Path::new("/etc/shadow")),
            Err(PathViolation::Absolute)
        );
        assert_eq!(
            validate_archive_path(Path::new("datasets/../../etc/shadow")),
            Err(PathViolation::ParentTraversal)
        );
        assert_eq!(
            validate_archive_path(Path::new(r"C:\Windows\system.ini")),
            Err(PathViolation::AmbiguousSeparator)
        );
        assert_eq!(
            validate_archive_path(Path::new("")),
            Err(PathViolation::Empty)
        );
    }

    #[test]
    fn symlink_target_may_move_within_but_never_above_the_root() {
        let link = validate_archive_path(Path::new("dataset/links/latest")).unwrap();
        let target = resolve_link_target(&link, Path::new("../versions/v2")).unwrap();
        assert_eq!(target.as_path(), Path::new("dataset/versions/v2"));

        assert_eq!(
            resolve_link_target(&link, Path::new("../../../etc/shadow")),
            Err(PathViolation::RootEscapeThroughLink)
        );
        assert_eq!(
            resolve_link_target(&link, Path::new("/etc/shadow")),
            Err(PathViolation::Absolute)
        );
    }

    #[test]
    fn hard_link_must_target_a_prior_contained_entry() {
        let existing = validate_archive_path(Path::new("dataset/file-a")).unwrap();
        let mut extracted = BTreeSet::new();
        extracted.insert(existing.clone());
        assert_eq!(
            validate_hard_link_target(Path::new("dataset/file-a"), &extracted),
            Ok(existing)
        );
        assert_eq!(
            validate_hard_link_target(Path::new("dataset/future-file"), &extracted),
            Err(PathViolation::HardLinkTargetNotPreviouslyExtracted)
        );
        assert_eq!(
            validate_hard_link_target(Path::new("../host-file"), &extracted),
            Err(PathViolation::ParentTraversal)
        );
    }
}
