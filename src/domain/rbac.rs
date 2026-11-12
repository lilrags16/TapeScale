//! Product role and destructive-action policy.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    Administrator,
    Operator,
    Viewer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    ViewStatus,
    ViewInventory,
    ViewVerificationEvidence,
    RunPermittedJob,
    CancelPermittedJob,
    HandleAssignedMedia,
    ConfirmAssignedCustody,
    RestoreToApprovedDestination,
    ConfigurePolicy,
    ConfigureHardware,
    ManageAccountsAndRoles,
    ManageKeys,
    ExportRecoveryKeys,
    ReduceRetention,
    EraseOrRelabelMedia,
    OverrideMediaProtection,
    RestoreOverExistingData,
    InstallUpdate,
}

impl Action {
    pub const ALL: [Self; 18] = [
        Self::ViewStatus,
        Self::ViewInventory,
        Self::ViewVerificationEvidence,
        Self::RunPermittedJob,
        Self::CancelPermittedJob,
        Self::HandleAssignedMedia,
        Self::ConfirmAssignedCustody,
        Self::RestoreToApprovedDestination,
        Self::ConfigurePolicy,
        Self::ConfigureHardware,
        Self::ManageAccountsAndRoles,
        Self::ManageKeys,
        Self::ExportRecoveryKeys,
        Self::ReduceRetention,
        Self::EraseOrRelabelMedia,
        Self::OverrideMediaProtection,
        Self::RestoreOverExistingData,
        Self::InstallUpdate,
    ];

    pub fn requires_reauthentication(self) -> bool {
        matches!(
            self,
            Self::ExportRecoveryKeys
                | Self::ReduceRetention
                | Self::EraseOrRelabelMedia
                | Self::OverrideMediaProtection
                | Self::RestoreOverExistingData
        )
    }

    pub fn requires_affected_object_preview(self) -> bool {
        matches!(
            self,
            Self::ReduceRetention
                | Self::EraseOrRelabelMedia
                | Self::OverrideMediaProtection
                | Self::RestoreOverExistingData
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthorizationDecision {
    pub allowed_by_role: bool,
    pub reauthentication_required: bool,
    pub affected_object_preview_required: bool,
}

pub fn authorize(role: Role, action: Action) -> AuthorizationDecision {
    use Action as A;

    let allowed_by_role = match role {
        Role::Administrator => true,
        Role::Operator => matches!(
            action,
            A::ViewStatus
                | A::ViewInventory
                | A::ViewVerificationEvidence
                | A::RunPermittedJob
                | A::CancelPermittedJob
                | A::HandleAssignedMedia
                | A::ConfirmAssignedCustody
                | A::RestoreToApprovedDestination
        ),
        Role::Viewer => matches!(
            action,
            A::ViewStatus | A::ViewInventory | A::ViewVerificationEvidence
        ),
    };

    AuthorizationDecision {
        allowed_by_role,
        reauthentication_required: allowed_by_role && action.requires_reauthentication(),
        affected_object_preview_required: allowed_by_role
            && action.requires_affected_object_preview(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn administrator_role_is_necessary_but_critical_actions_need_more_proof() {
        for action in Action::ALL {
            let decision = authorize(Role::Administrator, action);
            assert!(decision.allowed_by_role, "administrator denied {action:?}");
        }

        let destructive = authorize(Role::Administrator, Action::OverrideMediaProtection);
        assert!(destructive.reauthentication_required);
        assert!(destructive.affected_object_preview_required);

        let ordinary = authorize(Role::Administrator, Action::ViewStatus);
        assert!(!ordinary.reauthentication_required);
        assert!(!ordinary.affected_object_preview_required);
    }

    #[test]
    fn operators_can_operate_but_not_change_protection_policy() {
        for action in [
            Action::RunPermittedJob,
            Action::HandleAssignedMedia,
            Action::ConfirmAssignedCustody,
            Action::RestoreToApprovedDestination,
        ] {
            assert!(authorize(Role::Operator, action).allowed_by_role);
        }

        for action in [
            Action::ConfigurePolicy,
            Action::ManageKeys,
            Action::ReduceRetention,
            Action::EraseOrRelabelMedia,
            Action::OverrideMediaProtection,
            Action::RestoreOverExistingData,
        ] {
            assert!(!authorize(Role::Operator, action).allowed_by_role);
        }
    }

    #[test]
    fn viewers_have_no_mutating_authority() {
        for action in Action::ALL {
            let expected = matches!(
                action,
                Action::ViewStatus | Action::ViewInventory | Action::ViewVerificationEvidence
            );
            assert_eq!(authorize(Role::Viewer, action).allowed_by_role, expected);
        }
    }

    #[test]
    fn key_export_requires_reauthentication_but_not_an_object_destruction_preview() {
        let decision = authorize(Role::Administrator, Action::ExportRecoveryKeys);
        assert!(decision.allowed_by_role);
        assert!(decision.reauthentication_required);
        assert!(!decision.affected_object_preview_required);
    }
}
