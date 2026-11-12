//! Recovery-point dependency retention and incremental-base selection.

use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RetentionLimit {
    Until(u64),
    Indefinite,
}

impl RetentionLimit {
    fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::Indefinite, _) | (_, Self::Indefinite) => Self::Indefinite,
            (Self::Until(left), Self::Until(right)) => Self::Until(left.max(right)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetentionNode {
    pub id: String,
    pub retain_until: Option<u64>,
    pub indefinite_archive: bool,
    pub hold_until: Option<u64>,
    pub indefinite_hold: bool,
    pub dependencies: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum RetentionProblem {
    DuplicateRecoveryPoint(String),
    MissingDependency {
        recovery_point: String,
        dependency: String,
    },
    DependencyCycle(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RetentionProjection {
    /// Effective protection for directly retained recovery points and every
    /// dependency required to reconstruct them.
    pub effective: BTreeMap<String, RetentionLimit>,
    pub problems: Vec<RetentionProblem>,
}

pub fn project_retention(nodes: &[RetentionNode], now: u64) -> RetentionProjection {
    let mut projection = RetentionProjection::default();
    let mut by_id = BTreeMap::new();

    for node in nodes {
        if by_id.insert(node.id.as_str(), node).is_some() {
            projection
                .problems
                .push(RetentionProblem::DuplicateRecoveryPoint(node.id.clone()));
        }
    }

    for node in by_id.values() {
        if let Some(limit) = direct_limit(node, now) {
            protect_dependency(&node.id, limit, &by_id, &mut projection, &mut Vec::new());
        }
    }

    projection.problems.sort();
    projection.problems.dedup();
    projection
}

fn direct_limit(node: &RetentionNode, now: u64) -> Option<RetentionLimit> {
    if node.indefinite_archive || node.indefinite_hold {
        return Some(RetentionLimit::Indefinite);
    }

    [node.retain_until, node.hold_until]
        .into_iter()
        .flatten()
        .filter(|until| *until > now)
        .max()
        .map(RetentionLimit::Until)
}

fn protect_dependency(
    id: &str,
    limit: RetentionLimit,
    by_id: &BTreeMap<&str, &RetentionNode>,
    projection: &mut RetentionProjection,
    stack: &mut Vec<String>,
) {
    if let Some(position) = stack.iter().position(|item| item == id) {
        let mut cycle = stack[position..].to_vec();
        cycle.push(id.to_owned());
        projection
            .problems
            .push(RetentionProblem::DependencyCycle(cycle));
        return;
    }

    projection
        .effective
        .entry(id.to_owned())
        .and_modify(|current| *current = current.merge(limit))
        .or_insert(limit);

    let Some(node) = by_id.get(id) else {
        return;
    };

    stack.push(id.to_owned());
    for dependency in &node.dependencies {
        if by_id.contains_key(dependency.as_str()) {
            protect_dependency(dependency, limit, by_id, projection, stack);
        } else {
            projection
                .problems
                .push(RetentionProblem::MissingDependency {
                    recovery_point: id.to_owned(),
                    dependency: dependency.clone(),
                });
        }
    }
    stack.pop();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Representation {
    FileArchive,
    NativeZfs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryPointStatus {
    Finalized,
    Verified,
    Writing,
    Incomplete,
    Failed,
    Canceled,
}

impl RecoveryPointStatus {
    fn is_valid_base(self) -> bool {
        matches!(self, Self::Finalized | Self::Verified)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseCandidate {
    pub id: String,
    pub completed_at: u64,
    pub status: RecoveryPointStatus,
    pub source_id: String,
    pub scope_hash: String,
    pub representation: Representation,
    pub format_version: u32,
    pub encryption_lineage: String,
    /// Identifies the compatible namespace or native snapshot lineage.
    pub change_lineage: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseRequirements {
    pub source_id: String,
    pub scope_hash: String,
    pub representation: Representation,
    pub format_version: u32,
    pub encryption_lineage: String,
    pub change_lineage: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BaseIncompatibility {
    NotFinalized,
    SourceChanged,
    ScopeChanged,
    RepresentationChanged,
    FormatChanged,
    EncryptionLineageChanged,
    ChangeLineageChanged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateRejection {
    pub candidate_id: String,
    pub reasons: BTreeSet<BaseIncompatibility>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaseSelection {
    Incremental { recovery_point_id: String },
    FullRequired { rejections: Vec<CandidateRejection> },
}

pub fn select_incremental_base(
    candidates: &[BaseCandidate],
    requirements: &BaseRequirements,
) -> BaseSelection {
    let mut compatible = Vec::new();
    let mut rejections = Vec::new();

    for candidate in candidates {
        let reasons = incompatibilities(candidate, requirements);
        if reasons.is_empty() {
            compatible.push(candidate);
        } else {
            rejections.push(CandidateRejection {
                candidate_id: candidate.id.clone(),
                reasons,
            });
        }
    }

    if let Some(candidate) = compatible.into_iter().max_by_key(|item| item.completed_at) {
        BaseSelection::Incremental {
            recovery_point_id: candidate.id.clone(),
        }
    } else {
        rejections.sort_by(|left, right| left.candidate_id.cmp(&right.candidate_id));
        BaseSelection::FullRequired { rejections }
    }
}

fn incompatibilities(
    candidate: &BaseCandidate,
    requirements: &BaseRequirements,
) -> BTreeSet<BaseIncompatibility> {
    let mut reasons = BTreeSet::new();
    if !candidate.status.is_valid_base() {
        reasons.insert(BaseIncompatibility::NotFinalized);
    }
    if candidate.source_id != requirements.source_id {
        reasons.insert(BaseIncompatibility::SourceChanged);
    }
    if candidate.scope_hash != requirements.scope_hash {
        reasons.insert(BaseIncompatibility::ScopeChanged);
    }
    if candidate.representation != requirements.representation {
        reasons.insert(BaseIncompatibility::RepresentationChanged);
    }
    if candidate.format_version != requirements.format_version {
        reasons.insert(BaseIncompatibility::FormatChanged);
    }
    if candidate.encryption_lineage != requirements.encryption_lineage {
        reasons.insert(BaseIncompatibility::EncryptionLineageChanged);
    }
    if candidate.change_lineage != requirements.change_lineage {
        reasons.insert(BaseIncompatibility::ChangeLineageChanged);
    }
    reasons
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, retain_until: Option<u64>, dependencies: &[&str]) -> RetentionNode {
        RetentionNode {
            id: id.into(),
            retain_until,
            indefinite_archive: false,
            hold_until: None,
            indefinite_hold: false,
            dependencies: dependencies.iter().map(|item| (*item).into()).collect(),
        }
    }

    #[test]
    fn retained_incremental_extends_every_dependency() {
        let nodes = vec![
            node("full", Some(10), &[]),
            node("inc-1", Some(20), &["full"]),
            node("inc-2", Some(40), &["inc-1"]),
        ];
        let projection = project_retention(&nodes, 15);
        assert_eq!(projection.effective["full"], RetentionLimit::Until(40));
        assert_eq!(projection.effective["inc-1"], RetentionLimit::Until(40));
        assert_eq!(projection.effective["inc-2"], RetentionLimit::Until(40));
        assert!(projection.problems.is_empty());
    }

    #[test]
    fn indefinite_archive_protection_propagates_to_its_chain() {
        let full = node("full", None, &[]);
        let mut archive = node("archive", None, &["full"]);
        archive.indefinite_archive = true;
        let projection = project_retention(&[full, archive], 100);
        assert_eq!(projection.effective["full"], RetentionLimit::Indefinite);
        assert_eq!(projection.effective["archive"], RetentionLimit::Indefinite);
    }

    #[test]
    fn expired_points_without_live_dependents_are_not_protected() {
        let projection = project_retention(&[node("expired", Some(10), &[])], 10);
        assert!(projection.effective.is_empty());
    }

    #[test]
    fn missing_dependencies_and_cycles_are_explicit() {
        let projection = project_retention(
            &[
                node("a", Some(50), &["b", "missing"]),
                node("b", None, &["a"]),
            ],
            0,
        );
        assert!(projection.problems.iter().any(|problem| matches!(
            problem,
            RetentionProblem::MissingDependency { dependency, .. } if dependency == "missing"
        )));
        assert!(projection
            .problems
            .iter()
            .any(|problem| matches!(problem, RetentionProblem::DependencyCycle(_))));
    }

    fn requirements() -> BaseRequirements {
        BaseRequirements {
            source_id: "dataset-guid".into(),
            scope_hash: "scope-v1".into(),
            representation: Representation::NativeZfs,
            format_version: 1,
            encryption_lineage: "key-family-a".into(),
            change_lineage: "snapshot-family-a".into(),
        }
    }

    fn candidate(id: &str, completed_at: u64) -> BaseCandidate {
        let expected = requirements();
        BaseCandidate {
            id: id.into(),
            completed_at,
            status: RecoveryPointStatus::Finalized,
            source_id: expected.source_id,
            scope_hash: expected.scope_hash,
            representation: expected.representation,
            format_version: expected.format_version,
            encryption_lineage: expected.encryption_lineage,
            change_lineage: expected.change_lineage,
        }
    }

    #[test]
    fn selects_the_latest_valid_compatible_base() {
        let older = candidate("older", 10);
        let newer = candidate("newer", 20);
        assert_eq!(
            select_incremental_base(&[newer, older], &requirements()),
            BaseSelection::Incremental {
                recovery_point_id: "newer".into()
            }
        );
    }

    #[test]
    fn failed_latest_run_does_not_replace_the_prior_valid_base() {
        let older = candidate("valid", 10);
        let mut failed = candidate("failed", 20);
        failed.status = RecoveryPointStatus::Failed;
        assert_eq!(
            select_incremental_base(&[older, failed], &requirements()),
            BaseSelection::Incremental {
                recovery_point_id: "valid".into()
            }
        );
    }

    #[test]
    fn scope_format_and_key_changes_require_a_full() {
        let mut incompatible = candidate("old", 10);
        incompatible.scope_hash = "scope-v0".into();
        incompatible.format_version = 0;
        incompatible.encryption_lineage = "retired-key-family".into();

        let BaseSelection::FullRequired { rejections } =
            select_incremental_base(&[incompatible], &requirements())
        else {
            panic!("an incompatible base was selected")
        };
        let reasons = &rejections[0].reasons;
        assert!(reasons.contains(&BaseIncompatibility::ScopeChanged));
        assert!(reasons.contains(&BaseIncompatibility::FormatChanged));
        assert!(reasons.contains(&BaseIncompatibility::EncryptionLineageChanged));
    }
}
