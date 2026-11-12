//! Append-only physical custody projection.
//!
//! Device observations and planned movements are deliberately separate from
//! operator-confirmed physical custody.

use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum PhysicalLocation {
    OnSite(String),
    OffSite(String),
    InTransit { from: String, to: String },
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceObservation {
    InDrive(String),
    InSlot { library: String, slot: u32 },
    InExportSlot { library: String, slot: u32 },
    AbsentFromLibrary(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CustodyEventKind {
    MovePlanned {
        from: PhysicalLocation,
        to: PhysicalLocation,
    },
    DepartureConfirmed {
        from: PhysicalLocation,
        destination_name: String,
    },
    ArrivalConfirmed {
        at: PhysicalLocation,
    },
    DirectMoveConfirmed {
        from: PhysicalLocation,
        to: PhysicalLocation,
    },
    DeviceObserved(DeviceObservation),
    /// Corrections append a new fact while retaining the incorrect event.
    Correction {
        corrects_event_id: String,
        confirmed_location: PhysicalLocation,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustodyEvent {
    pub id: String,
    pub tape_id: String,
    pub sequence: u64,
    pub actor_id: String,
    pub kind: CustodyEventKind,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum CustodyProblem {
    DuplicateEventId(String),
    DuplicateSequence(u64),
    ClaimedSourceDoesNotMatch {
        event_id: String,
        claimed: PhysicalLocation,
        actual: PhysicalLocation,
    },
    CorrectionTargetNotFound {
        event_id: String,
        target_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustodyProjection {
    pub confirmed: PhysicalLocation,
    pub latest_observation: Option<DeviceObservation>,
    pub pending_destination: Option<PhysicalLocation>,
    pub applied_event_ids: Vec<String>,
    pub problems: Vec<CustodyProblem>,
}

pub fn project_custody(
    tape_id: &str,
    initial: PhysicalLocation,
    events: &[CustodyEvent],
) -> CustodyProjection {
    let mut relevant: Vec<&CustodyEvent> = events
        .iter()
        .filter(|event| event.tape_id == tape_id)
        .collect();
    relevant.sort_by_key(|event| event.sequence);

    let mut projection = CustodyProjection {
        confirmed: initial,
        latest_observation: None,
        pending_destination: None,
        applied_event_ids: Vec::new(),
        problems: Vec::new(),
    };
    let mut ids = BTreeSet::new();
    let mut sequences = BTreeSet::new();

    for event in relevant {
        if !ids.insert(event.id.clone()) {
            projection
                .problems
                .push(CustodyProblem::DuplicateEventId(event.id.clone()));
            continue;
        }
        if !sequences.insert(event.sequence) {
            projection
                .problems
                .push(CustodyProblem::DuplicateSequence(event.sequence));
            continue;
        }

        let mut applied = true;
        match &event.kind {
            CustodyEventKind::MovePlanned { to, .. } => {
                projection.pending_destination = Some(to.clone());
            }
            CustodyEventKind::DepartureConfirmed {
                from,
                destination_name,
            } => {
                if &projection.confirmed != from {
                    projection
                        .problems
                        .push(CustodyProblem::ClaimedSourceDoesNotMatch {
                            event_id: event.id.clone(),
                            claimed: from.clone(),
                            actual: projection.confirmed.clone(),
                        });
                    applied = false;
                } else {
                    projection.confirmed = PhysicalLocation::InTransit {
                        from: location_name(from),
                        to: destination_name.clone(),
                    };
                }
            }
            CustodyEventKind::ArrivalConfirmed { at } => {
                projection.confirmed = at.clone();
                projection.pending_destination = None;
            }
            CustodyEventKind::DirectMoveConfirmed { from, to } => {
                if &projection.confirmed != from {
                    projection
                        .problems
                        .push(CustodyProblem::ClaimedSourceDoesNotMatch {
                            event_id: event.id.clone(),
                            claimed: from.clone(),
                            actual: projection.confirmed.clone(),
                        });
                    applied = false;
                } else {
                    projection.confirmed = to.clone();
                    projection.pending_destination = None;
                }
            }
            CustodyEventKind::DeviceObserved(observation) => {
                projection.latest_observation = Some(observation.clone());
            }
            CustodyEventKind::Correction {
                corrects_event_id,
                confirmed_location,
            } => {
                if !projection.applied_event_ids.contains(corrects_event_id) {
                    projection
                        .problems
                        .push(CustodyProblem::CorrectionTargetNotFound {
                            event_id: event.id.clone(),
                            target_id: corrects_event_id.clone(),
                        });
                    applied = false;
                } else {
                    projection.confirmed = confirmed_location.clone();
                    projection.pending_destination = None;
                }
            }
        }

        if applied {
            projection.applied_event_ids.push(event.id.clone());
        }
    }

    projection.problems.sort();
    projection
}

fn location_name(location: &PhysicalLocation) -> String {
    match location {
        PhysicalLocation::OnSite(name) | PhysicalLocation::OffSite(name) => name.clone(),
        PhysicalLocation::InTransit { from, to } => format!("in transit from {from} to {to}"),
        PhysicalLocation::Unknown => "unknown".into(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageResult {
    pub complete: bool,
    pub missing_or_elsewhere: Vec<String>,
}

pub fn recovery_set_at_location(
    required_tape_ids: &[String],
    locations: &BTreeMap<String, PhysicalLocation>,
    required_location: &PhysicalLocation,
) -> CoverageResult {
    let mut missing_or_elsewhere: Vec<String> = required_tape_ids
        .iter()
        .filter(|tape_id| locations.get(*tape_id) != Some(required_location))
        .cloned()
        .collect();
    missing_or_elsewhere.sort();
    missing_or_elsewhere.dedup();

    CoverageResult {
        complete: missing_or_elsewhere.is_empty(),
        missing_or_elsewhere,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(id: &str, sequence: u64, kind: CustodyEventKind) -> CustodyEvent {
        CustodyEvent {
            id: id.into(),
            tape_id: "tape-1".into(),
            sequence,
            actor_id: "operator-1".into(),
            kind,
        }
    }

    fn vault() -> PhysicalLocation {
        PhysicalLocation::OffSite("North vault".into())
    }

    fn library_room() -> PhysicalLocation {
        PhysicalLocation::OnSite("Library room".into())
    }

    #[test]
    fn planned_move_and_export_slot_do_not_invent_off_site_delivery() {
        let events = vec![
            event(
                "planned",
                1,
                CustodyEventKind::MovePlanned {
                    from: library_room(),
                    to: vault(),
                },
            ),
            event(
                "observed",
                2,
                CustodyEventKind::DeviceObserved(DeviceObservation::InExportSlot {
                    library: "changer-1".into(),
                    slot: 1,
                }),
            ),
        ];
        let projection = project_custody("tape-1", library_room(), &events);
        assert_eq!(projection.confirmed, library_room());
        assert_eq!(projection.pending_destination, Some(vault()));
        assert!(matches!(
            projection.latest_observation,
            Some(DeviceObservation::InExportSlot { .. })
        ));
    }

    #[test]
    fn confirmed_departure_and_arrival_advance_custody() {
        let events = vec![
            event(
                "departed",
                1,
                CustodyEventKind::DepartureConfirmed {
                    from: library_room(),
                    destination_name: "North vault".into(),
                },
            ),
            event(
                "arrived",
                2,
                CustodyEventKind::ArrivalConfirmed { at: vault() },
            ),
        ];
        let projection = project_custody("tape-1", library_room(), &events);
        assert_eq!(projection.confirmed, vault());
        assert!(projection.problems.is_empty());
    }

    #[test]
    fn incorrect_source_is_detected_and_does_not_move_the_tape() {
        let events = [event(
            "bad-move",
            1,
            CustodyEventKind::DirectMoveConfirmed {
                from: PhysicalLocation::OnSite("Wrong building".into()),
                to: vault(),
            },
        )];
        let projection = project_custody("tape-1", library_room(), &events);
        assert_eq!(projection.confirmed, library_room());
        assert!(matches!(
            projection.problems.as_slice(),
            [CustodyProblem::ClaimedSourceDoesNotMatch { .. }]
        ));
    }

    #[test]
    fn correction_keeps_the_original_event_in_history() {
        let events = vec![
            event(
                "arrival",
                1,
                CustodyEventKind::ArrivalConfirmed { at: vault() },
            ),
            event(
                "correction",
                2,
                CustodyEventKind::Correction {
                    corrects_event_id: "arrival".into(),
                    confirmed_location: library_room(),
                },
            ),
        ];
        let projection = project_custody("tape-1", PhysicalLocation::Unknown, &events);
        assert_eq!(projection.confirmed, library_room());
        assert_eq!(projection.applied_event_ids, vec!["arrival", "correction"]);
    }

    #[test]
    fn partial_delivery_never_counts_as_complete_coverage() {
        let mut locations = BTreeMap::new();
        locations.insert("tape-1".into(), vault());
        locations.insert("tape-2".into(), library_room());
        let required = vec!["tape-1".into(), "tape-2".into()];

        let result = recovery_set_at_location(&required, &locations, &vault());
        assert!(!result.complete);
        assert_eq!(result.missing_or_elsewhere, vec!["tape-2"]);

        locations.insert("tape-2".into(), vault());
        assert!(recovery_set_at_location(&required, &locations, &vault()).complete);
    }
}
