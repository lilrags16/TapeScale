//! Cartridge overwrite authorization.

use super::lto::{can_write, DriveGeneration, MediaFormat};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityState {
    /// Physical, on-tape, and catalog identities agree.
    Confirmed,
    Unknown,
    Foreign,
    Mismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaHealth {
    Healthy,
    Degraded,
    Quarantined,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reservation {
    None,
    ActiveJob,
    Export,
    Restore,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProtectionClaims {
    pub retained_recovery_points: usize,
    pub dependent_recovery_points: usize,
    pub explicit_holds: usize,
    pub indefinite_archives: usize,
}

impl ProtectionClaims {
    pub fn total(&self) -> usize {
        self.retained_recovery_points
            + self.dependent_recovery_points
            + self.explicit_holds
            + self.indefinite_archives
    }

    pub fn is_empty(&self) -> bool {
        self.total() == 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverwriteIntent {
    AutomaticReuse,
    /// An exceptional destructive action. Logical protection may only be
    /// overridden when every approval field is present.
    AdministratorException {
        reauthenticated: bool,
        affected_objects_previewed: bool,
        audit_reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverwriteContext {
    pub identity: IdentityState,
    pub physical_state_reconciled: bool,
    pub engine_state_reconciled: bool,
    pub health: MediaHealth,
    pub is_worm: bool,
    pub reservation: Reservation,
    pub protection: ProtectionClaims,
    pub policy_allows_reuse: bool,
    pub drive: DriveGeneration,
    pub media_format: MediaFormat,
    pub intent: OverwriteIntent,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum OverwriteBlocker {
    IdentityUnknown,
    ForeignMedia,
    IdentityMismatch,
    PhysicalStateUnreconciled,
    EngineStateUnreconciled,
    MediaNotHealthy,
    PhysicalWormMedia,
    ReservedForActiveJob,
    ReservedForExport,
    ReservedForRestore,
    DriveCannotWriteMediaFormat,
    PolicyDisallowsReuse,
    RetainedRecoveryPoints,
    DependentRecoveryPoints,
    ExplicitHolds,
    IndefiniteArchive,
    AdministratorReauthenticationRequired,
    AffectedObjectPreviewRequired,
    AuditReasonRequired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverwriteDecision {
    pub allowed: bool,
    pub blockers: Vec<OverwriteBlocker>,
    /// Number of logical protection claims knowingly overridden by a complete
    /// administrator exception. Physical safety blockers are never overridable.
    pub overridden_claims: usize,
}

pub fn evaluate_overwrite(context: &OverwriteContext) -> OverwriteDecision {
    let mut blockers = Vec::new();

    match context.identity {
        IdentityState::Confirmed => {}
        IdentityState::Unknown => blockers.push(OverwriteBlocker::IdentityUnknown),
        IdentityState::Foreign => blockers.push(OverwriteBlocker::ForeignMedia),
        IdentityState::Mismatch => blockers.push(OverwriteBlocker::IdentityMismatch),
    }
    if !context.physical_state_reconciled {
        blockers.push(OverwriteBlocker::PhysicalStateUnreconciled);
    }
    if !context.engine_state_reconciled {
        blockers.push(OverwriteBlocker::EngineStateUnreconciled);
    }
    if context.health != MediaHealth::Healthy {
        blockers.push(OverwriteBlocker::MediaNotHealthy);
    }
    if context.is_worm {
        blockers.push(OverwriteBlocker::PhysicalWormMedia);
    }
    match context.reservation {
        Reservation::None => {}
        Reservation::ActiveJob => blockers.push(OverwriteBlocker::ReservedForActiveJob),
        Reservation::Export => blockers.push(OverwriteBlocker::ReservedForExport),
        Reservation::Restore => blockers.push(OverwriteBlocker::ReservedForRestore),
    }
    if !can_write(context.drive, context.media_format) {
        blockers.push(OverwriteBlocker::DriveCannotWriteMediaFormat);
    }
    if !context.policy_allows_reuse {
        blockers.push(OverwriteBlocker::PolicyDisallowsReuse);
    }

    let mut overridden_claims = 0;
    if !context.protection.is_empty() {
        match &context.intent {
            OverwriteIntent::AutomaticReuse => {
                add_protection_blockers(&mut blockers, &context.protection)
            }
            OverwriteIntent::AdministratorException {
                reauthenticated,
                affected_objects_previewed,
                audit_reason,
            } => {
                let mut complete = true;
                if !reauthenticated {
                    blockers.push(OverwriteBlocker::AdministratorReauthenticationRequired);
                    complete = false;
                }
                if !affected_objects_previewed {
                    blockers.push(OverwriteBlocker::AffectedObjectPreviewRequired);
                    complete = false;
                }
                if audit_reason.trim().is_empty() {
                    blockers.push(OverwriteBlocker::AuditReasonRequired);
                    complete = false;
                }
                if complete {
                    overridden_claims = context.protection.total();
                } else {
                    add_protection_blockers(&mut blockers, &context.protection);
                }
            }
        }
    }

    blockers.sort();
    blockers.dedup();
    OverwriteDecision {
        allowed: blockers.is_empty(),
        blockers,
        overridden_claims,
    }
}

fn add_protection_blockers(blockers: &mut Vec<OverwriteBlocker>, claims: &ProtectionClaims) {
    if claims.retained_recovery_points > 0 {
        blockers.push(OverwriteBlocker::RetainedRecoveryPoints);
    }
    if claims.dependent_recovery_points > 0 {
        blockers.push(OverwriteBlocker::DependentRecoveryPoints);
    }
    if claims.explicit_holds > 0 {
        blockers.push(OverwriteBlocker::ExplicitHolds);
    }
    if claims.indefinite_archives > 0 {
        blockers.push(OverwriteBlocker::IndefiniteArchive);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn safe_context() -> OverwriteContext {
        OverwriteContext {
            identity: IdentityState::Confirmed,
            physical_state_reconciled: true,
            engine_state_reconciled: true,
            health: MediaHealth::Healthy,
            is_worm: false,
            reservation: Reservation::None,
            protection: ProtectionClaims::default(),
            policy_allows_reuse: true,
            drive: DriveGeneration::Lto8,
            media_format: MediaFormat::Lto8,
            intent: OverwriteIntent::AutomaticReuse,
        }
    }

    #[test]
    fn safe_expired_media_is_eligible() {
        assert_eq!(
            evaluate_overwrite(&safe_context()),
            OverwriteDecision {
                allowed: true,
                blockers: vec![],
                overridden_claims: 0,
            }
        );
    }

    #[test]
    fn unknown_media_is_protected_even_when_everything_else_looks_safe() {
        let mut context = safe_context();
        context.identity = IdentityState::Unknown;
        let decision = evaluate_overwrite(&context);
        assert!(!decision.allowed);
        assert!(decision
            .blockers
            .contains(&OverwriteBlocker::IdentityUnknown));
    }

    #[test]
    fn decision_reports_every_detected_safety_problem() {
        let mut context = safe_context();
        context.identity = IdentityState::Mismatch;
        context.physical_state_reconciled = false;
        context.engine_state_reconciled = false;
        context.health = MediaHealth::Quarantined;
        context.is_worm = true;
        context.reservation = Reservation::Export;
        context.policy_allows_reuse = false;
        context.drive = DriveGeneration::Lto10;
        context.media_format = MediaFormat::Lto9;

        let decision = evaluate_overwrite(&context);
        assert!(!decision.allowed);
        assert_eq!(decision.blockers.len(), 8);
        assert!(decision
            .blockers
            .contains(&OverwriteBlocker::PhysicalWormMedia));
        assert!(decision
            .blockers
            .contains(&OverwriteBlocker::DriveCannotWriteMediaFormat));
    }

    #[test]
    fn automatic_reuse_never_discards_a_dependency_or_archive() {
        let mut context = safe_context();
        context.protection = ProtectionClaims {
            dependent_recovery_points: 2,
            indefinite_archives: 1,
            ..ProtectionClaims::default()
        };
        let decision = evaluate_overwrite(&context);
        assert!(!decision.allowed);
        assert!(decision
            .blockers
            .contains(&OverwriteBlocker::DependentRecoveryPoints));
        assert!(decision
            .blockers
            .contains(&OverwriteBlocker::IndefiniteArchive));
    }

    #[test]
    fn incomplete_exception_does_not_hide_the_underlying_claims() {
        let mut context = safe_context();
        context.protection.retained_recovery_points = 1;
        context.intent = OverwriteIntent::AdministratorException {
            reauthenticated: false,
            affected_objects_previewed: true,
            audit_reason: "Emergency capacity".into(),
        };
        let decision = evaluate_overwrite(&context);
        assert!(!decision.allowed);
        assert!(decision
            .blockers
            .contains(&OverwriteBlocker::AdministratorReauthenticationRequired));
        assert!(decision
            .blockers
            .contains(&OverwriteBlocker::RetainedRecoveryPoints));
        assert_eq!(decision.overridden_claims, 0);
    }

    #[test]
    fn complete_exception_can_override_logical_but_not_physical_protection() {
        let mut context = safe_context();
        context.protection = ProtectionClaims {
            retained_recovery_points: 1,
            explicit_holds: 1,
            ..ProtectionClaims::default()
        };
        context.intent = OverwriteIntent::AdministratorException {
            reauthenticated: true,
            affected_objects_previewed: true,
            audit_reason: "Approved destruction test".into(),
        };
        let decision = evaluate_overwrite(&context);
        assert!(decision.allowed);
        assert_eq!(decision.overridden_claims, 2);

        context.is_worm = true;
        let decision = evaluate_overwrite(&context);
        assert!(!decision.allowed);
        assert!(decision
            .blockers
            .contains(&OverwriteBlocker::PhysicalWormMedia));
    }
}
