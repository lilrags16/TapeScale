//! Restore prerequisite planning.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreMode {
    FileArchive,
    NativeDataset,
    FilesFromNative,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConflictPolicy {
    #[default]
    Skip,
    Rename,
    Replace,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TapePrerequisite {
    pub tape_id: String,
    pub available: bool,
    pub confirmed_location: Option<String>,
    /// Only physically qualified, currently eligible drives belong here.
    pub compatible_qualified_drive_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyPrerequisite {
    pub key_id: String,
    pub available: bool,
    pub recovery_package_tested: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VerificationEvidence {
    pub full_readback_passed: bool,
    pub restore_test_passed: bool,
    pub native_stream_validation_available: bool,
    pub per_file_digests_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DestinationAssessment {
    pub approved: bool,
    pub is_new_destination: bool,
    pub conflicts_found: usize,
    pub conflict_policy: ConflictPolicy,
    pub acl_semantics_supported: bool,
    pub identity_mapping_complete: bool,
    pub estimated_required_bytes: Option<u64>,
    pub available_bytes: Option<u64>,
    pub required_free_reserve_bytes: u64,
    pub administrator_reauthenticated: bool,
    pub destructive_preview_accepted: bool,
}

impl Default for DestinationAssessment {
    fn default() -> Self {
        Self {
            approved: false,
            is_new_destination: true,
            conflicts_found: 0,
            conflict_policy: ConflictPolicy::Skip,
            acl_semantics_supported: false,
            identity_mapping_complete: false,
            estimated_required_bytes: None,
            available_bytes: None,
            required_free_reserve_bytes: 0,
            administrator_reauthenticated: false,
            destructive_preview_accepted: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreRequest {
    pub recovery_point_id: String,
    pub mode: RestoreMode,
    pub dependency_chain: Vec<String>,
    pub missing_dependencies: Vec<String>,
    pub tapes: Vec<TapePrerequisite>,
    pub keys: Vec<KeyPrerequisite>,
    pub verification: VerificationEvidence,
    pub destination: DestinationAssessment,
    /// Recovered datasets must not be mounted over live paths or published as
    /// shares automatically.
    pub automatic_publication_requested: bool,
}

impl RestoreRequest {
    pub fn safe_defaults(recovery_point_id: impl Into<String>, mode: RestoreMode) -> Self {
        Self {
            recovery_point_id: recovery_point_id.into(),
            mode,
            dependency_chain: Vec::new(),
            missing_dependencies: Vec::new(),
            tapes: Vec::new(),
            keys: Vec::new(),
            verification: VerificationEvidence::default(),
            destination: DestinationAssessment::default(),
            automatic_publication_requested: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum RestoreBlocker {
    MissingRecoveryDependency(String),
    TapeUnavailable(String),
    TapeLocationUnknown(String),
    NoQualifiedCompatibleDrive(String),
    RequiredKeyUnavailable(String),
    DestinationNotApproved,
    DestinationAclSemanticsUnsupported,
    IdentityMappingIncomplete,
    InsufficientDestinationCapacity {
        required_with_reserve: u64,
        available: u64,
    },
    DestructiveReplacementRequiresAdministratorReauthentication,
    DestructiveReplacementRequiresPreview,
    NativeDestinationMustBeNewOrExplicitlyReplaced,
    AutomaticNativePublicationForbidden,
    FileDigestEvidenceUnavailable,
    NativeStreamValidationUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum RestoreWarning {
    FullReadbackNotPassed,
    RestoreTestNotPassed,
    RecoveryKeyPackageNotTested(String),
    CapacityEstimateUnavailable,
    CapacityIsAnEstimate,
    ExistingConflictsWillBeSkipped(usize),
    ExistingConflictsWillBeRenamed(usize),
    NativeExtractionHasStreamLevelEvidenceOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestorePlan {
    pub recovery_point_id: String,
    pub dependency_chain: Vec<String>,
    pub required_tape_ids: Vec<String>,
    pub required_key_ids: Vec<String>,
    pub blockers: Vec<RestoreBlocker>,
    pub warnings: Vec<RestoreWarning>,
}

impl RestorePlan {
    pub fn ready(&self) -> bool {
        self.blockers.is_empty()
    }
}

pub fn preflight_restore(request: &RestoreRequest) -> RestorePlan {
    let mut blockers = Vec::new();
    let mut warnings = Vec::new();

    for dependency in &request.missing_dependencies {
        blockers.push(RestoreBlocker::MissingRecoveryDependency(
            dependency.clone(),
        ));
    }
    for tape in &request.tapes {
        if !tape.available {
            blockers.push(RestoreBlocker::TapeUnavailable(tape.tape_id.clone()));
        }
        if tape.confirmed_location.is_none() {
            blockers.push(RestoreBlocker::TapeLocationUnknown(tape.tape_id.clone()));
        }
        if tape.compatible_qualified_drive_ids.is_empty() {
            blockers.push(RestoreBlocker::NoQualifiedCompatibleDrive(
                tape.tape_id.clone(),
            ));
        }
    }
    for key in &request.keys {
        if !key.available {
            blockers.push(RestoreBlocker::RequiredKeyUnavailable(key.key_id.clone()));
        }
        if !key.recovery_package_tested {
            warnings.push(RestoreWarning::RecoveryKeyPackageNotTested(
                key.key_id.clone(),
            ));
        }
    }

    if !request.destination.approved {
        blockers.push(RestoreBlocker::DestinationNotApproved);
    }
    if !request.destination.acl_semantics_supported {
        blockers.push(RestoreBlocker::DestinationAclSemanticsUnsupported);
    }
    if !request.destination.identity_mapping_complete {
        blockers.push(RestoreBlocker::IdentityMappingIncomplete);
    }
    match (
        request.destination.estimated_required_bytes,
        request.destination.available_bytes,
    ) {
        (Some(required), Some(available)) => {
            let required_with_reserve =
                required.saturating_add(request.destination.required_free_reserve_bytes);
            if available < required_with_reserve {
                blockers.push(RestoreBlocker::InsufficientDestinationCapacity {
                    required_with_reserve,
                    available,
                });
            }
            warnings.push(RestoreWarning::CapacityIsAnEstimate);
        }
        _ => warnings.push(RestoreWarning::CapacityEstimateUnavailable),
    }

    if !request.destination.is_new_destination && request.destination.conflicts_found > 0 {
        match request.destination.conflict_policy {
            ConflictPolicy::Skip => warnings.push(RestoreWarning::ExistingConflictsWillBeSkipped(
                request.destination.conflicts_found,
            )),
            ConflictPolicy::Rename => warnings.push(
                RestoreWarning::ExistingConflictsWillBeRenamed(request.destination.conflicts_found),
            ),
            ConflictPolicy::Replace => {}
        }
    }

    // Selecting replacement for an existing destination is itself a
    // destructive intent. Do not rely on a possibly incomplete conflict scan
    // to decide whether reauthentication and preview are required.
    if !request.destination.is_new_destination
        && request.destination.conflict_policy == ConflictPolicy::Replace
    {
        if !request.destination.administrator_reauthenticated {
            blockers
                .push(RestoreBlocker::DestructiveReplacementRequiresAdministratorReauthentication);
        }
        if !request.destination.destructive_preview_accepted {
            blockers.push(RestoreBlocker::DestructiveReplacementRequiresPreview);
        }
    }

    if request.mode == RestoreMode::NativeDataset {
        if !request.destination.is_new_destination
            && request.destination.conflict_policy != ConflictPolicy::Replace
        {
            blockers.push(RestoreBlocker::NativeDestinationMustBeNewOrExplicitlyReplaced);
        }
        if request.automatic_publication_requested {
            blockers.push(RestoreBlocker::AutomaticNativePublicationForbidden);
        }
        if !request.verification.native_stream_validation_available {
            blockers.push(RestoreBlocker::NativeStreamValidationUnavailable);
        }
    }

    if request.mode == RestoreMode::FileArchive && !request.verification.per_file_digests_available
    {
        blockers.push(RestoreBlocker::FileDigestEvidenceUnavailable);
    }
    if request.mode == RestoreMode::FilesFromNative
        && !request.verification.per_file_digests_available
    {
        warnings.push(RestoreWarning::NativeExtractionHasStreamLevelEvidenceOnly);
    }
    if request.mode == RestoreMode::FilesFromNative
        && !request.verification.native_stream_validation_available
    {
        blockers.push(RestoreBlocker::NativeStreamValidationUnavailable);
    }
    if !request.verification.full_readback_passed {
        warnings.push(RestoreWarning::FullReadbackNotPassed);
    }
    if !request.verification.restore_test_passed {
        warnings.push(RestoreWarning::RestoreTestNotPassed);
    }

    blockers.sort();
    blockers.dedup();
    warnings.sort();
    warnings.dedup();

    let mut required_tape_ids: Vec<String> = request
        .tapes
        .iter()
        .map(|tape| tape.tape_id.clone())
        .collect();
    required_tape_ids.sort();
    required_tape_ids.dedup();
    let mut required_key_ids: Vec<String> =
        request.keys.iter().map(|key| key.key_id.clone()).collect();
    required_key_ids.sort();
    required_key_ids.dedup();

    RestorePlan {
        recovery_point_id: request.recovery_point_id.clone(),
        dependency_chain: request.dependency_chain.clone(),
        required_tape_ids,
        required_key_ids,
        blockers,
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready_file_request() -> RestoreRequest {
        RestoreRequest {
            recovery_point_id: "rp-1".into(),
            mode: RestoreMode::FileArchive,
            dependency_chain: vec!["full-1".into(), "rp-1".into()],
            missing_dependencies: vec![],
            tapes: vec![TapePrerequisite {
                tape_id: "TS0001".into(),
                available: true,
                confirmed_location: Some("Library room".into()),
                compatible_qualified_drive_ids: vec!["drive-a".into()],
            }],
            keys: vec![KeyPrerequisite {
                key_id: "key-2026".into(),
                available: true,
                recovery_package_tested: true,
            }],
            verification: VerificationEvidence {
                full_readback_passed: true,
                restore_test_passed: true,
                native_stream_validation_available: false,
                per_file_digests_available: true,
            },
            destination: DestinationAssessment {
                approved: true,
                is_new_destination: true,
                conflicts_found: 0,
                conflict_policy: ConflictPolicy::Skip,
                acl_semantics_supported: true,
                identity_mapping_complete: true,
                estimated_required_bytes: Some(100),
                available_bytes: Some(200),
                required_free_reserve_bytes: 50,
                administrator_reauthenticated: false,
                destructive_preview_accepted: false,
            },
            automatic_publication_requested: false,
        }
    }

    #[test]
    fn complete_file_restore_is_ready_and_preserves_plan_details() {
        let plan = preflight_restore(&ready_file_request());
        assert!(plan.ready(), "unexpected blockers: {:?}", plan.blockers);
        assert_eq!(plan.required_tape_ids, vec!["TS0001"]);
        assert_eq!(plan.required_key_ids, vec!["key-2026"]);
        assert_eq!(plan.dependency_chain, vec!["full-1", "rp-1"]);
        assert_eq!(plan.warnings, vec![RestoreWarning::CapacityIsAnEstimate]);
    }

    #[test]
    fn preflight_returns_all_actionable_missing_prerequisites() {
        let mut request = ready_file_request();
        request.missing_dependencies.push("inc-2".into());
        request.tapes[0].available = false;
        request.tapes[0].confirmed_location = None;
        request.tapes[0].compatible_qualified_drive_ids.clear();
        request.keys[0].available = false;
        request.destination.approved = false;
        request.destination.acl_semantics_supported = false;
        request.destination.identity_mapping_complete = false;
        request.destination.available_bytes = Some(120);

        let plan = preflight_restore(&request);
        assert!(!plan.ready());
        assert!(plan
            .blockers
            .contains(&RestoreBlocker::MissingRecoveryDependency("inc-2".into())));
        assert!(plan
            .blockers
            .contains(&RestoreBlocker::TapeUnavailable("TS0001".into())));
        assert!(plan
            .blockers
            .contains(&RestoreBlocker::RequiredKeyUnavailable("key-2026".into())));
        assert!(plan
            .blockers
            .contains(&RestoreBlocker::DestinationAclSemanticsUnsupported));
        assert!(plan
            .blockers
            .contains(&RestoreBlocker::InsufficientDestinationCapacity {
                required_with_reserve: 150,
                available: 120,
            }));
    }

    #[test]
    fn replacement_requires_reauthentication_and_preview() {
        let mut request = ready_file_request();
        request.destination.is_new_destination = false;
        request.destination.conflicts_found = 4;
        request.destination.conflict_policy = ConflictPolicy::Replace;

        let plan = preflight_restore(&request);
        assert!(plan.blockers.contains(
            &RestoreBlocker::DestructiveReplacementRequiresAdministratorReauthentication
        ));
        assert!(plan
            .blockers
            .contains(&RestoreBlocker::DestructiveReplacementRequiresPreview));

        request.destination.administrator_reauthenticated = true;
        request.destination.destructive_preview_accepted = true;
        assert!(preflight_restore(&request).ready());
    }

    #[test]
    fn native_restore_never_auto_publishes_and_requires_stream_validation() {
        let mut request = ready_file_request();
        request.mode = RestoreMode::NativeDataset;
        request.automatic_publication_requested = true;
        request.verification.per_file_digests_available = false;
        request.verification.native_stream_validation_available = false;

        let plan = preflight_restore(&request);
        assert!(plan
            .blockers
            .contains(&RestoreBlocker::AutomaticNativePublicationForbidden));
        assert!(plan
            .blockers
            .contains(&RestoreBlocker::NativeStreamValidationUnavailable));
        assert!(!plan
            .blockers
            .contains(&RestoreBlocker::FileDigestEvidenceUnavailable));
    }

    #[test]
    fn native_file_extraction_reports_stream_only_evidence_truthfully() {
        let mut request = ready_file_request();
        request.mode = RestoreMode::FilesFromNative;
        request.verification.per_file_digests_available = false;
        request.verification.native_stream_validation_available = true;
        let plan = preflight_restore(&request);
        assert!(plan.ready());
        assert!(plan
            .warnings
            .contains(&RestoreWarning::NativeExtractionHasStreamLevelEvidenceOnly));
    }

    #[test]
    fn native_file_extraction_requires_native_stream_validation() {
        let mut request = ready_file_request();
        request.mode = RestoreMode::FilesFromNative;
        request.verification.per_file_digests_available = false;
        request.verification.native_stream_validation_available = false;
        assert!(preflight_restore(&request)
            .blockers
            .contains(&RestoreBlocker::NativeStreamValidationUnavailable));
    }

    #[test]
    fn defaults_are_non_destructive_and_require_assessment() {
        let request = RestoreRequest::safe_defaults("rp", RestoreMode::FileArchive);
        assert!(request.destination.is_new_destination);
        assert_eq!(request.destination.conflict_policy, ConflictPolicy::Skip);
        assert!(!request.automatic_publication_requested);
        assert!(preflight_restore(&request)
            .blockers
            .contains(&RestoreBlocker::DestinationNotApproved));
    }
}
