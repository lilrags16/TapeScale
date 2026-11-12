//! Durable job-state semantics.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum JobState {
    Queued,
    Preparing,
    WaitingForMedia,
    Writing,
    Written,
    Verifying,
    Verified,
    Blocked,
    Interrupted,
    Failed,
    Canceled,
}

impl JobState {
    pub const ALL: [Self; 11] = [
        Self::Queued,
        Self::Preparing,
        Self::WaitingForMedia,
        Self::Writing,
        Self::Written,
        Self::Verifying,
        Self::Verified,
        Self::Blocked,
        Self::Interrupted,
        Self::Failed,
        Self::Canceled,
    ];

    /// A complete write claim is valid only after all required data and
    /// finalization operations have succeeded.
    pub fn has_complete_write(self) -> bool {
        matches!(self, Self::Written | Self::Verifying | Self::Verified)
    }

    /// Verification is a separate claim from write completion.
    pub fn has_passed_verification(self) -> bool {
        self == Self::Verified
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Verified | Self::Failed | Self::Canceled)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidTransition {
    pub from: JobState,
    pub to: JobState,
}

pub fn transition_allowed(from: JobState, to: JobState) -> bool {
    use JobState as S;

    matches!(
        (from, to),
        (S::Queued, S::Preparing | S::Canceled)
            | (
                S::Preparing,
                S::WaitingForMedia
                    | S::Writing
                    | S::Blocked
                    | S::Interrupted
                    | S::Failed
                    | S::Canceled
            )
            | (
                S::WaitingForMedia,
                S::Preparing | S::Writing | S::Blocked | S::Interrupted | S::Failed | S::Canceled
            )
            | (
                S::Writing,
                S::WaitingForMedia | S::Written | S::Interrupted | S::Failed | S::Canceled
            )
            | (S::Written, S::Verifying | S::Canceled)
            | (
                S::Verifying,
                S::Verified | S::Interrupted | S::Failed | S::Canceled
            )
            | (
                S::Blocked,
                S::Preparing | S::WaitingForMedia | S::Interrupted | S::Failed | S::Canceled
            )
            | (
                S::Interrupted,
                S::Preparing | S::Blocked | S::Failed | S::Canceled
            )
    )
}

pub fn require_transition(from: JobState, to: JobState) -> Result<(), InvalidTransition> {
    if transition_allowed(from, to) {
        Ok(())
    } else {
        Err(InvalidTransition { from, to })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn happy_path_requires_distinct_write_and_verification_steps() {
        let path = [
            JobState::Queued,
            JobState::Preparing,
            JobState::Writing,
            JobState::Written,
            JobState::Verifying,
            JobState::Verified,
        ];
        for states in path.windows(2) {
            assert!(transition_allowed(states[0], states[1]));
        }
        assert!(!transition_allowed(JobState::Writing, JobState::Verified));
        assert!(!transition_allowed(JobState::Written, JobState::Verified));
    }

    #[test]
    fn waiting_for_continuation_media_can_resume_writing() {
        assert!(transition_allowed(
            JobState::Writing,
            JobState::WaitingForMedia
        ));
        assert!(transition_allowed(
            JobState::WaitingForMedia,
            JobState::Writing
        ));
    }

    #[test]
    fn interrupted_verification_cannot_be_reported_as_passed() {
        assert!(transition_allowed(
            JobState::Verifying,
            JobState::Interrupted
        ));
        assert!(!JobState::Interrupted.has_passed_verification());
        assert!(!transition_allowed(
            JobState::Interrupted,
            JobState::Verified
        ));
    }

    #[test]
    fn terminal_states_cannot_be_reopened() {
        for state in [JobState::Verified, JobState::Failed, JobState::Canceled] {
            assert!(state.is_terminal());
            for destination in JobState::ALL {
                assert!(!transition_allowed(state, destination));
            }
        }
    }

    #[test]
    fn only_finalized_states_claim_a_complete_write() {
        for state in JobState::ALL {
            assert_eq!(
                state.has_complete_write(),
                matches!(
                    state,
                    JobState::Written | JobState::Verifying | JobState::Verified
                )
            );
        }
    }

    #[test]
    fn self_transitions_are_rejected_to_keep_history_meaningful() {
        for state in JobState::ALL {
            assert_eq!(
                require_transition(state, state),
                Err(InvalidTransition {
                    from: state,
                    to: state
                })
            );
        }
    }
}
