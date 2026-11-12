//! Realistic deterministic fixtures used by the engineering-preview console.
//!
//! Every response is marked as simulation data so it cannot be mistaken for qualification evidence.

use serde_json::{json, Value};

pub fn overview() -> Value {
    json!({
        "mode": "simulation",
        "evidence_scope": "Control-plane demonstration only; no physical tape claims.",
        "generated_at": "2026-11-12T09:41:00Z",
        "site": {
            "name": "Primary NAS · Studio A",
            "timezone": "America/Chicago",
            "status": "attention",
            "preflight": "3 actions required"
        },
        "summary": {
            "latest_local": "2026-11-12T02:17:00Z",
            "latest_complete_offsite": "2026-11-04T02:12:00Z",
            "protected_sources": 6,
            "active_jobs": 2,
            "required_actions": 4
        },
        "coverage": [
            {
                "id": "src-production",
                "source": "tank/production/media/projects-2026-and-current-deliverables",
                "mode": "File + native ZFS",
                "local": {"time": "2026-11-12T02:17:00Z", "status": "verified", "label": "Verified 7h ago"},
                "offsite": {"time": "2026-11-04T02:12:00Z", "status": "overdue", "label": "8d 7h old"},
                "target_days": 8,
                "gap": "Off-site source-data age exceeds target by 7 hours",
                "points": [
                    {"time": "2026-11-09T02:03:00Z", "kind": "full", "location": "local", "verified": true},
                    {"time": "2026-11-10T02:08:00Z", "kind": "incremental", "location": "local", "verified": true},
                    {"time": "2026-11-11T02:11:00Z", "kind": "incremental", "location": "local", "verified": true},
                    {"time": "2026-11-12T02:17:00Z", "kind": "incremental", "location": "local", "verified": true}
                ]
            },
            {
                "id": "src-finance",
                "source": "tank/business/finance-and-legal-records",
                "mode": "File",
                "local": {"time": "2026-11-12T02:22:00Z", "status": "verified", "label": "Verified 7h ago"},
                "offsite": {"time": "2026-11-09T04:31:00Z", "status": "healthy", "label": "Verified off-site"},
                "target_days": 8,
                "gap": null,
                "points": []
            },
            {
                "id": "src-vm",
                "source": "tank/virtualization/production-zvols",
                "mode": "Native ZFS · block volumes",
                "local": {"time": "2026-11-11T02:36:00Z", "status": "written", "label": "Read-back pending"},
                "offsite": {"time": "2026-11-02T04:44:00Z", "status": "overdue", "label": "Incomplete set"},
                "target_days": 8,
                "gap": "Verification must complete before export readiness",
                "points": []
            }
        ],
        "active_jobs": [
            {
                "id": "job-nightly-production",
                "name": "Nightly production protection",
                "source": "tank/production/media/projects-2026-and-current-deliverables",
                "state": "waiting_for_media",
                "state_label": "Waiting for media",
                "phase": "Writing · volume 4 of 5",
                "logical_bytes": 14774687498240_u64,
                "encoded_bytes": 9126805504000_u64,
                "progress": 76,
                "source_rate": "612 MiB/s",
                "drive_rate": "—",
                "current_tape": "TS-L9-004218",
                "waiting": "18m 42s",
                "required_action": "Load one eligible LTO-9 tape from Weekly-45"
            },
            {
                "id": "job-quarterly-native",
                "name": "Quarterly native restore exercise",
                "source": "tank/virtualization/production-zvols",
                "state": "verifying",
                "state_label": "Verifying",
                "phase": "Native receive validation",
                "logical_bytes": 4398046511104_u64,
                "encoded_bytes": 3958241859993_u64,
                "progress": 64,
                "source_rate": "—",
                "drive_rate": "286 MiB/s",
                "current_tape": "TS-L9-003971",
                "waiting": "0s",
                "required_action": null
            }
        ],
        "actions": [
            {
                "id": "act-media-1", "severity": "urgent", "kind": "media_request",
                "title": "Load continuation tape", "detail": "Nightly production needs one reconciled LTO-9 tape. The snapshot hold remains protected.",
                "action_label": "Open media request", "href": "#/jobs/job-nightly-production", "due_at": "2026-11-12T09:22:00Z",
                "tape_ids": []
            },
            {
                "id": "act-move-1", "severity": "warning", "kind": "movement",
                "title": "Off-site delivery overdue", "detail": "Weekly-45 is ready; 5 of 6 tapes are confirmed at Vault North.",
                "action_label": "Finish delivery", "href": "#/movements", "due_at": "2026-11-12T08:00:00Z",
                "tape_ids": ["TS-L9-004214", "TS-L9-004215", "TS-L9-004216", "TS-L9-004217", "TS-L9-004218", "TS-L9-004219"]
            },
            {
                "id": "act-reconcile-1", "severity": "critical", "kind": "device",
                "title": "Drive position is uncertain", "detail": "IBM ULT3580-TD9 · serial 10WT090114 changed device path after reconnect. Writes are blocked.",
                "action_label": "Reconcile drive", "href": "#/drives", "due_at": null, "tape_ids": []
            },
            {
                "id": "act-key-1", "severity": "warning", "kind": "recovery_package",
                "title": "Recovery package check due", "detail": "Key package KP-2026-09 has not been independently opened in 83 days.",
                "action_label": "Review assurance", "href": "#/verification", "due_at": "2026-11-15T00:00:00Z", "tape_ids": []
            }
        ],
        "drives": drives()["drives"].clone()
    })
}

pub fn jobs() -> Value {
    json!({
        "mode": "simulation",
        "jobs": [
            {"id":"job-nightly-production","name":"Nightly production protection","source":"tank/production/media/projects-2026-and-current-deliverables","state":"waiting_for_media","state_label":"Waiting for media","started_at":"2026-11-12T02:00:14Z","progress":76,"mode":"File + native ZFS","attempt":1,"recovery_point":"2026-11-12 02:17 UTC"},
            {"id":"job-quarterly-native","name":"Quarterly native restore exercise","source":"tank/virtualization/production-zvols","state":"verifying","state_label":"Verifying","started_at":"2026-11-12T05:15:00Z","progress":64,"mode":"Native ZFS","attempt":1,"recovery_point":"2026-11-09 02:04 UTC"},
            {"id":"job-finance-daily","name":"Finance daily recovery point","source":"tank/business/finance-and-legal-records","state":"verified","state_label":"Verified","started_at":"2026-11-12T02:00:03Z","finished_at":"2026-11-12T03:08:19Z","progress":100,"mode":"File","attempt":1,"recovery_point":"2026-11-12 02:22 UTC"},
            {"id":"job-vm-readback","name":"VM weekly full read-back","source":"tank/virtualization/production-zvols","state":"interrupted","state_label":"Interrupted · reconcile","started_at":"2026-11-11T11:02:00Z","progress":42,"mode":"Native ZFS","attempt":2,"recovery_point":"2026-11-09 02:04 UTC"},
            {"id":"job-archive","name":"2026 campaign masters archive","source":"tank/production/archive-candidates/campaign-masters","state":"failed","state_label":"Failed · source unreadable","started_at":"2026-11-10T18:30:00Z","finished_at":"2026-11-10T18:34:11Z","progress":3,"mode":"File · two copies","attempt":1,"recovery_point":null}
        ]
    })
}

pub fn job_detail(id: &str) -> Option<Value> {
    let jobs = jobs();
    let job = jobs["jobs"]
        .as_array()?
        .iter()
        .find(|job| job["id"] == id)?
        .clone();
    Some(json!({
        "mode": "simulation",
        "job": job,
        "correlation_id": "019a77b2-3b1c-7d58-81c6-8525ee624da9",
        "required_action": if id == "job-nightly-production" { json!({
            "title":"Load one eligible continuation tape",
            "detail":"Use a reconciled, writable LTO-9 tape assigned to Weekly-45. Do not insert TS-L9-003504; it contains a retained dependency.",
            "acceptable_formats":["LTO-9"], "snapshot_hold":"Protected", "waiting_since":"2026-11-12T09:22:18Z"
        }) } else { Value::Null },
        "phases": [
            {"id":"preparing","label":"Preparing","state":"complete","durable":true,"time":"02:00:14"},
            {"id":"snapshot","label":"Snapshot","state":"complete","durable":true,"time":"02:01:02"},
            {"id":"writing","label":"Writing","state":if id == "job-nightly-production"{"active"}else{"complete"},"durable":false,"time":"02:01:08"},
            {"id":"finalizing","label":"Finalizing","state":"pending","durable":false},
            {"id":"verifying","label":"Verifying","state":"pending","durable":false},
            {"id":"verified","label":"Verified","state":"pending","durable":false}
        ],
        "metrics": {
            "logical_bytes":14774687498240_u64, "encoded_bytes":9126805504000_u64,
            "source_rate_bps":641728512_u64, "drive_rate_bps":0, "estimated_media_use":4.76,
            "waiting_seconds":1122, "estimate":true
        },
        "volumes": [
            {"order":1,"tape_id":"TS-L9-004214","status":"finalized","encoded_bytes":2144047674163_u64,"boundary":"records 1–18,402"},
            {"order":2,"tape_id":"TS-L9-004215","status":"finalized","encoded_bytes":2143190530048_u64,"boundary":"records 18,403–36,711"},
            {"order":3,"tape_id":"TS-L9-004216","status":"finalized","encoded_bytes":2145629187741_u64,"boundary":"records 36,712–55,203"},
            {"order":4,"tape_id":"TS-L9-004218","status":"loaded","encoded_bytes":1564800000000_u64,"boundary":"records 55,204–68,919"},
            {"order":5,"tape_id":null,"status":"required","encoded_bytes":null,"boundary":"continuation pending"}
        ],
        "source": {"snapshot":"auto-2026-11-12_02-00","snapshot_guid":"13840597284395021172","logical_root":"/tank/production/media","consistency":"Filesystem / crash-consistent","scope_hash":"sha256:19a7…8f0c","hold":"tapescale:019a77b2"},
        "verification": {"state":"not_started","label":"Not yet verified","scope":"Full application read-back required","restore_test":null},
        "events": [
            {"time":"2026-11-12T02:00:14Z","kind":"state","title":"Run created","detail":"Scheduled occurrence 2026-11-12 · attempt 1"},
            {"time":"2026-11-12T02:01:02Z","kind":"snapshot","title":"Immutable snapshot acquired","detail":"GUID 13840597284395021172 · hold applied"},
            {"time":"2026-11-12T02:01:08Z","kind":"state","title":"Writing started","detail":"Drive TS-DRV-02 · fencing token 8842"},
            {"time":"2026-11-12T09:22:18Z","kind":"media","title":"Continuation tape required","detail":"Volume 4 finalized at a proven record boundary; snapshot hold retained"}
        ]
    }))
}

pub fn media() -> Value {
    json!({
        "mode":"simulation",
        "media":[
            {"id":"TS-L9-004214","barcode":"T9004214","generation":"LTO-9","format":"LTO-9","pool":"Weekly-45","contents":"Full + 3 increments","location":"Vault North · Shelf B-17","planned":"—","retention":"Protected until 2026-12-17","holds":["Dependency"],"verification":"Full · 2026-11-09","read_eligible":true,"write_eligible":false,"health":"healthy","overwrite":"Blocked · retained recovery point"},
            {"id":"TS-L9-004215","barcode":"T9004215","generation":"LTO-9","format":"LTO-9","pool":"Weekly-45","contents":"Full continuation","location":"Vault North · Shelf B-17","planned":"—","retention":"Protected until 2026-12-17","holds":["Dependency"],"verification":"Full · 2026-11-09","read_eligible":true,"write_eligible":false,"health":"healthy","overwrite":"Blocked · retained dependency"},
            {"id":"TS-L9-004216","barcode":"T9004216","generation":"LTO-9","format":"LTO-9","pool":"Weekly-45","contents":"Full continuation","location":"Vault North · intake","planned":"Shelf B-17","retention":"Protected until 2026-12-17","holds":[],"verification":"Full · 2026-11-09","read_eligible":true,"write_eligible":false,"health":"healthy","overwrite":"Blocked · retained content"},
            {"id":"TS-L9-004217","barcode":"T9004217","generation":"LTO-9","format":"LTO-9","pool":"Weekly-45","contents":"Incremental records","location":"Vault North · Shelf B-17","planned":"—","retention":"Protected until 2026-12-17","holds":[],"verification":"Full · 2026-11-11","read_eligible":true,"write_eligible":false,"health":"healthy","overwrite":"Blocked · retained content"},
            {"id":"TS-L9-004218","barcode":"T9004218","generation":"LTO-9","format":"LTO-9","pool":"Weekly-46","contents":"Active write · volume 4","location":"Drive TS-DRV-01","planned":"Vault North","retention":"Reserved by active job","holds":["Active write"],"verification":"Not yet eligible","read_eligible":false,"write_eligible":false,"health":"healthy","overwrite":"Blocked · active reservation"},
            {"id":"TS-L8-002884","barcode":"T8002884","generation":"LTO-8","format":"LTO-8","pool":"Scratch","contents":"Empty · reconciled","location":"Library A · slot 18","planned":"—","retention":"Eligible","holds":[],"verification":"Blank identity read · 2026-11-12","read_eligible":true,"write_eligible":true,"health":"healthy","overwrite":"Allowed for matching policy"},
            {"id":"UNKNOWN-7F2A","barcode":"—","generation":"Unknown","format":"Foreign / unknown","pool":"Unassigned","contents":"Identity unresolved","location":"Library A · slot 23","planned":"—","retention":"Protected","holds":["Unknown identity"],"verification":"Never","read_eligible":false,"write_eligible":false,"health":"attention","overwrite":"Blocked · identity unresolved"},
            {"id":"TS-L7M-000112","barcode":"M8000112","generation":"LTO-7 Type M","format":"LTO-7 Type M","pool":"Legacy weekly","contents":"2 retained recovery points","location":"On-site fire safe","planned":"Verification recall 2026-11-19","retention":"Protected until 2027-01-03","holds":[],"verification":"Full · 2025-11-18","read_eligible":true,"write_eligible":false,"health":"verification_due","overwrite":"Blocked · retained content"}
        ]
    })
}

pub fn drives() -> Value {
    json!({
        "mode":"simulation",
        "library":{"name":"Library A","model":"IBM TS4300","serial":"78A0K91","observed_at":"2026-11-12T09:40:52Z","state":"attention","slots":24,"io_slots":3},
        "drives":[
            {"id":"TS-DRV-01","model":"IBM ULT3580-TD9","serial":"10WT090113","wwn":"500507630E12A001","generation":"LTO-9","path":"/dev/nst0","state":"waiting_for_media","state_label":"Waiting for media","loaded_tape":null,"operation":"Nightly production · volume 5","health":"healthy","qualified":false},
            {"id":"TS-DRV-02","model":"IBM ULT3580-TD9","serial":"10WT090114","wwn":"500507630E12A002","generation":"LTO-9","path":"/dev/nst2 (was /dev/nst1)","state":"reconciliation_required","state_label":"Reconciliation required","loaded_tape":"TS-L9-003971","operation":"Blocked after reconnect","health":"position_uncertain","qualified":false}
        ],
        "slots":[
            {"slot":1,"kind":"storage","tape":"TS-L9-004214","state":"occupied"},
            {"slot":2,"kind":"storage","tape":"TS-L9-004215","state":"occupied"},
            {"slot":3,"kind":"storage","tape":null,"state":"empty"},
            {"slot":18,"kind":"storage","tape":"TS-L8-002884","state":"occupied"},
            {"slot":23,"kind":"storage","tape":"UNKNOWN-7F2A","state":"unknown"},
            {"slot":25,"kind":"io","tape":"TS-L9-004216","state":"exported_not_delivered"},
            {"slot":26,"kind":"io","tape":null,"state":"empty"},
            {"slot":27,"kind":"io","tape":null,"state":"empty"}
        ]
    })
}

pub fn movements() -> Value {
    json!({
        "mode":"simulation",
        "tasks":[
            {"id":"move-weekly-45","type":"take_off_site","state":"overdue","state_label":"Overdue · incomplete delivery","title":"Deliver Weekly-45 to Vault North","source":"Studio A · Library A","destination":"Vault North · Shelf B-17","due_at":"2026-11-12T08:00:00Z","reason":"Maintain one complete verified off-site recovery point","ready":true,"confirmed_count":5,"total_count":6,"tapes":[
                {"id":"TS-L9-004214","confirmed":true},{"id":"TS-L9-004215","confirmed":true},{"id":"TS-L9-004216","confirmed":false},{"id":"TS-L9-004217","confirmed":true},{"id":"TS-L9-004218","confirmed":true},{"id":"TS-L9-004219","confirmed":true}
            ],"gates":[{"label":"Writes finalized","state":"pass"},{"label":"Recovery metadata committed","state":"pass"},{"label":"Full read-back passed","state":"pass"},{"label":"Recovery keys independently checked","state":"pass"},{"label":"All tapes physically confirmed","state":"block"}]},
            {"id":"move-recall-l7m","type":"verification_recall","state":"planned","state_label":"Planned","title":"Recall LTO-7M set for annual read-back","source":"On-site fire safe","destination":"Studio A · Library A","due_at":"2026-11-19T16:00:00Z","reason":"Annual retained-media audit","ready":false,"confirmed_count":0,"total_count":2,"tapes":[{"id":"TS-L7M-000112","confirmed":false},{"id":"TS-L7M-000113","confirmed":false}],"gates":[{"label":"Replacement off-site coverage remains","state":"pass"},{"label":"Compatible drive reserved","state":"block"}]}
        ],
        "custody_events":[
            {"time":"2026-11-12T07:42:11Z","tape":"TS-L9-004214","event":"Arrival confirmed","location":"Vault North · Shelf B-17","actor":"Maya Chen"},
            {"time":"2026-11-12T07:43:02Z","tape":"TS-L9-004215","event":"Arrival confirmed","location":"Vault North · Shelf B-17","actor":"Maya Chen"},
            {"time":"2026-11-12T07:47:36Z","tape":"TS-L9-004216","event":"Marked missing from delivery","location":"Studio A · I/O slot 1 (observed)","actor":"Maya Chen"}
        ]
    })
}

pub fn restore_plan(destination: Option<&str>, key_available: bool) -> Value {
    let destination = destination.unwrap_or("tank/restores/2026-11-12-production");
    let mut blockers = vec![json!({
        "code":"missing_tape", "title":"One tape must be recalled", "detail":"TS-L9-004216 is at Vault North · intake", "action":"Create recall task"
    })];
    if !key_available {
        blockers.push(json!({
            "code":"missing_key", "title":"Recovery key package is unavailable", "detail":"Key lineage KL-2026-09 is required", "action":"Import recovery package"
        }));
    }
    json!({
        "mode":"simulation",
        "plan_id":"plan-019a7801",
        "state":if blockers.is_empty(){"ready"}else{"blocked"},
        "recovery_point":{"id":"rp-2026-11-09-production","time":"2026-11-09T02:03:00Z","source":"tank/production/media/projects-2026-and-current-deliverables","representation":"File","verification":"Full read-back passed 2026-11-09T13:42:00Z"},
        "selection":{"kind":"files","paths":["/campaigns/winter-launch/final-deliverables/","/contracts/talent-release-042.pdf"],"file_count":1842,"logical_bytes":128849018880_u64},
        "dependencies":[{"id":"rp-full-2026-11-09","kind":"Full","status":"available"},{"id":"rp-inc-2026-11-10","kind":"Incremental","status":"available"},{"id":"rp-inc-2026-11-11","kind":"Incremental","status":"available"}],
        "tapes":[{"id":"TS-L9-004214","location":"Vault North · Shelf B-17","status":"recall_required","order":1},{"id":"TS-L9-004215","location":"Vault North · Shelf B-17","status":"recall_required","order":2},{"id":"TS-L9-004216","location":"Vault North · intake","status":"missing","order":3}],
        "drives":[{"id":"TS-DRV-01","compatible":true,"qualified":false,"detail":"LTO-9 compatible; physical qualification not completed"}],
        "key":{"lineage":"KL-2026-09","available":key_available,"independently_verified":"2026-09-04T16:22:00Z"},
        "destination":{"path":destination,"kind":"new_directory","conflict_policy":"skip","approved":true,"available_bytes":2199023255552_u64},
        "estimates":{"tape_reads":3,"active_read_time_seconds":11820,"staging_bytes":0,"label":"Estimate based on observed read rate"},
        "warnings":[{"code":"identity_mapping","title":"Numeric UID/GID mapping will be preserved","detail":"Confirm matching directory identities before publishing restored data."}],
        "blockers":blockers
    })
}

pub fn preflight(database_connected: bool) -> Value {
    json!({
        "mode":"simulation",
        "status":"attention",
        "run_at":"2026-11-12T09:40:52Z",
        "checks":[
            {"id":"database","label":"Durable PostgreSQL state","state":if database_connected{"pass"}else{"simulated"},"detail":if database_connected{"Connected; migrations current"}else{"Demo mode uses in-memory fixtures; production requires PostgreSQL"}},
            {"id":"engine","label":"Tape engine and format","state":"block","detail":"Simulator active. G0 engine and durable on-tape format are not selected."},
            {"id":"truenas","label":"TrueNAS 25.10.8 API and stream path","state":"not_run","detail":"Secure WebSocket/API and actual ZFS data path require the qualification host."},
            {"id":"devices","label":"Stable tape/changer identities","state":"simulated","detail":"Two simulated LTO-9 drives; no physical discovery evidence."},
            {"id":"keys","label":"Independent recovery keys","state":"warning","detail":"Demo key package is 83 days since its last open test."},
            {"id":"scratch","label":"Spool/staging reserve","state":"pass","detail":"2.00 TiB available; 1.25 TiB reserve configured."},
            {"id":"media","label":"Compatible writable media","state":"warning","detail":"One eligible LTO-8 tape; current LTO-9 job still needs media."}
        ]
    })
}

pub fn qualification() -> Value {
    let implemented = [
        2, 5, 10, 12, 13, 18, 19, 20, 21, 22, 23, 25, 27, 28, 29, 30, 31,
    ];
    let cases: Vec<Value> = (1..=31)
        .map(|number| {
            let status = if implemented.contains(&number) {
                "simulated"
            } else {
                "not_run"
            };
            let reason = if status == "simulated" {
                "Automated control-plane coverage exists; physical portions remain unqualified."
            } else {
                "Requires designated TrueNAS/tape fixtures or a later implementation gate."
            };
            json!({"id":format!("AT-{number:02}"),"status":status,"reason":reason})
        })
        .collect();
    json!({
        "release":"0.1.0 engineering preview",
        "gate":"G0-ready / not G0-passed",
        "engine":{"candidate":"Bacula Community 15.x","selection":"pending physical gate","adapter":"deterministic simulator","format":"not selected"},
        "claims":{"software_tests":"Implemented where listed","physical_lto":"No generations qualified","truenas":"25.10.8 target; not qualified on this host"},
        "cases":cases
    })
}
