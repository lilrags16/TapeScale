(function () {
  "use strict";

  const API_BASE = "/api/v1";
  const $ = (selector, root = document) => root.querySelector(selector);
  const $$ = (selector, root = document) => Array.from(root.querySelectorAll(selector));

  const DEMO = {
    overview: {
      mode: "simulation",
      updatedAt: "2026-11-12T11:42:18Z",
      stats: {
        protected: "148.6 TiB",
        activeJobs: 2,
        requiredActions: 4,
        nextWindow: "14h 18m"
      }
    },
    sources: [
      {
        id: "src-production",
        name: "Production media",
        path: "tank/production/media/projects-2026-and-current-deliverables",
        mode: "File + native ZFS",
        localDate: "12 Nov · 02:17",
        offsiteDate: "04 Nov · 02:12",
        localPoints: 7,
        offsitePoints: 4,
        status: "warning",
        statusTitle: "Off-site coverage aging",
        statusDetail: "8d 3h old · target exceeded by 3h"
      },
      {
        id: "src-homes",
        name: "Team home datasets",
        path: "tank/homes",
        mode: "File archive",
        localDate: "12 Nov · 02:48",
        offsiteDate: "09 Nov · 03:06",
        localPoints: 7,
        offsitePoints: 5,
        status: "success",
        statusTitle: "Coverage complete",
        statusDetail: "Latest verified copy at Vault North"
      },
      {
        id: "src-vm",
        name: "Virtual machine volumes",
        path: "fastpool/virtualization/production-zvols",
        mode: "Native ZFS · block volumes",
        localDate: "11 Nov · 23:42",
        offsiteDate: "08 Nov · 23:51",
        localPoints: 5,
        offsitePoints: 4,
        status: "success",
        statusTitle: "Coverage complete",
        statusDetail: "Quarterly native drill due 18 Dec"
      }
    ],
    jobs: [
      {
        id: "job-1048",
        name: "Nightly production protection",
        kind: "Incremental",
        mode: "File + native ZFS",
        source: "tank/production/media/projects-2026-and-current-deliverables",
        status: "waiting for media",
        phase: 3,
        started: "2026-11-12T02:00:07Z",
        schedule: "Daily at 02:00 America/New_York",
        progress: 68,
        logical: "8.42 TiB",
        encoded: "6.78 TiB",
        sourceRate: "246 MiB/s",
        driveRate: "231 MiB/s",
        currentTape: "TS-L9-004219",
        requestedTape: "TS-L9-004220",
        eta: "Operator action required",
        attempt: "01JCHZVMM0B4Y7G4PWP3T14X0C",
        snapshot: "auto-tapescale-2026-11-12_02-00",
        media: ["TS-L9-004218", "TS-L9-004219", "TS-L9-004220"],
        error: null,
        events: [
          ["11:34:12 UTC", "Media request created", "Insert TS-L9-004220 or another eligible Weekly-46 cartridge."],
          ["11:33:51 UTC", "Durable record boundary reached", "Volume TS-L9-004219 finalized with 5.91 TiB encoded."],
          ["02:01:26 UTC", "Snapshot holds confirmed", "3 application-owned holds protect this attempt."],
          ["02:00:07 UTC", "Attempt started", "Preflight passed; base job-1039 is compatible."]
        ]
      },
      {
        id: "job-1049",
        name: "Team homes incremental",
        kind: "Incremental",
        mode: "File archive",
        source: "tank/homes",
        status: "verifying",
        phase: 5,
        started: "2026-11-12T02:30:03Z",
        schedule: "Daily at 02:30 America/New_York",
        progress: 82,
        logical: "1.12 TiB",
        encoded: "742 GiB",
        sourceRate: "—",
        driveRate: "286 MiB/s",
        currentTape: "TS-L8-002184",
        eta: "17 min",
        attempt: "01JCHZXH8X1R46NFC13YQ8DQ6G",
        snapshot: "auto-tapescale-2026-11-12_02-30",
        media: ["TS-L8-002184"],
        error: null,
        events: [
          ["11:12:09 UTC", "Full read-back started", "Reading application records through the recovery decode path."],
          ["11:08:41 UTC", "Write finalized", "Catalog and on-tape recovery metadata committed."],
          ["02:30:03 UTC", "Attempt started", "Exact namespace comparison enabled."]
        ]
      },
      {
        id: "job-1047",
        name: "VM native stream protection",
        kind: "Incremental",
        mode: "Native ZFS",
        source: "fastpool/virtualization/production-zvols",
        status: "verified",
        phase: 6,
        started: "2026-11-11T23:30:05Z",
        finished: "2026-11-11T23:51:22Z",
        schedule: "Daily at 23:30 UTC",
        progress: 100,
        logical: "638 GiB",
        encoded: "622 GiB",
        sourceRate: "511 MiB/s",
        driveRate: "487 MiB/s",
        currentTape: "TS-L9-003881",
        eta: "Complete",
        attempt: "01JCFVV7Y7ZB9V4K2E71T6J2VK",
        snapshot: "auto-tapescale-2026-11-11_23-30",
        media: ["TS-L9-003881"],
        error: null,
        events: [
          ["23:51:22 UTC", "Verification passed", "638 GiB native stream authenticated and decoded."],
          ["23:42:18 UTC", "Write finalized", "Stream and recovery metadata committed."],
          ["23:30:05 UTC", "Attempt started", "Incremental base snapshot GUID matched."]
        ]
      },
      {
        id: "job-1046",
        name: "TrueNAS configuration export",
        kind: "Full",
        mode: "Configuration export",
        source: "truenas-primary.internal",
        status: "failed",
        phase: 2,
        started: "2026-11-11T22:00:01Z",
        finished: "2026-11-11T22:00:16Z",
        schedule: "After protected job finalization",
        progress: 12,
        logical: "—",
        encoded: "—",
        sourceRate: "—",
        driveRate: "—",
        currentTape: "—",
        eta: "Failed",
        attempt: "01JCFQT06QGJ7A5B8VF34NYW6F",
        snapshot: "Not created",
        media: [],
        error: "TrueNAS denied system.config.save: service credential lacks FULL_ADMIN permission.",
        events: [
          ["22:00:16 UTC", "Source operation failed", "TrueNAS denied system.config.save. No backup record was created."],
          ["22:00:01 UTC", "Attempt started", "Authenticated WebSocket session established."]
        ]
      },
      {
        id: "job-1045",
        name: "Monthly representative restore drill",
        kind: "Restore test",
        mode: "File archive",
        source: "tank/homes",
        status: "successful",
        phase: 6,
        started: "2026-11-10T14:00:00Z",
        finished: "2026-11-10T14:38:44Z",
        schedule: "Monthly on day 10",
        progress: 100,
        logical: "84.2 GiB",
        encoded: "—",
        sourceRate: "—",
        driveRate: "219 MiB/s",
        currentTape: "TS-L8-002177",
        eta: "Complete",
        attempt: "01JCCB3PGQ7J0WJY1E8S3B1H6X",
        snapshot: "Recovery point 2026-11-09 03:06 UTC",
        media: ["TS-L8-002177"],
        error: null,
        events: [
          ["14:38:44 UTC", "Restore test passed", "1,240 files and promised metadata matched the source manifest."],
          ["14:02:11 UTC", "Tape mounted", "TS-L8-002177 loaded in Drive 2."],
          ["14:00:00 UTC", "Exercise started", "Approved isolated destination reserved."]
        ]
      },
      {
        id: "job-1044",
        name: "Weekly production full",
        kind: "Full",
        mode: "File + native ZFS",
        source: "tank/production/media/projects-2026-and-current-deliverables",
        status: "interrupted",
        phase: 4,
        started: "2026-11-08T02:00:04Z",
        finished: "2026-11-08T08:41:33Z",
        schedule: "Saturday at 02:00 America/New_York",
        progress: 91,
        logical: "41.7 TiB",
        encoded: "34.2 TiB",
        sourceRate: "—",
        driveRate: "—",
        currentTape: "TS-L9-004205",
        eta: "Reconciliation required",
        attempt: "01JC2A5BC0EWG42EP2Z8N6GGN7",
        snapshot: "auto-tapescale-2026-11-08_02-00",
        media: ["TS-L9-004199", "TS-L9-004201", "TS-L9-004203", "TS-L9-004205"],
        error: "Drive reset during finalization. Tape position is uncertain; automatic continuation is blocked.",
        events: [
          ["08:41:33 UTC", "Drive position uncertain", "SCSI reset detected during finalization. Destructive continuation blocked."],
          ["08:40:59 UTC", "Finalization started", "Closing recovery group after 34.2 TiB encoded."],
          ["02:00:04 UTC", "Attempt started", "New compatible full chain created."]
        ]
      }
    ],
    tapes: [
      { id: "TS-L9-004220", barcode: "TS4220L9", generation: "LTO-9", format: "LTO-9", pool: "Routine · Weekly 46", contents: "Empty · reserved for job-1048", location: "Library A · Slot 18", movement: "None", retention: "Reserved", verification: "Never written", eligibility: "Write eligible", health: "Healthy", tone: "waiting", used: 0 },
      { id: "TS-L9-004219", barcode: "TS4219L9", generation: "LTO-9", format: "LTO-9", pool: "Routine · Weekly 46", contents: "job-1048 · volume 2 of 3", location: "Library A · Drive 1", movement: "None", retention: "Protected · 35d min", verification: "Pending full read-back", eligibility: "Active write set", health: "Healthy", tone: "writing", used: 94 },
      { id: "TS-L9-004218", barcode: "TS4218L9", generation: "LTO-9", format: "LTO-9", pool: "Routine · Weekly 46", contents: "job-1048 · volume 1 of 3", location: "Library A · Slot 17", movement: "Take off site after verify", retention: "Protected · 35d min", verification: "Read-back pending", eligibility: "Read only", health: "Healthy", tone: "warning", used: 97 },
      { id: "TS-L9-004205", barcode: "TS4205L9", generation: "LTO-9", format: "LTO-9", pool: "Routine · Weekly 45", contents: "Interrupted job-1044 · volume 4", location: "Library A · Quarantine", movement: "Blocked", retention: "Hold · investigation", verification: "Incomplete", eligibility: "Protected", health: "Investigate", tone: "interrupted", used: 76 },
      { id: "TS-L9-004203", barcode: "TS4203L9", generation: "LTO-9", format: "LTO-9", pool: "Routine · Weekly 45", contents: "Production full · volume 3", location: "Vault North · B-17", movement: "Confirmed 09 Nov", retention: "Protected through 13 Dec", verification: "Full read-back · 08 Nov", eligibility: "Read only", health: "Healthy", tone: "offsite", used: 96 },
      { id: "TS-L9-004201", barcode: "TS4201L9", generation: "LTO-9", format: "LTO-9", pool: "Routine · Weekly 45", contents: "Production full · volume 2", location: "Vault North · B-17", movement: "Confirmed 09 Nov", retention: "Protected through 13 Dec", verification: "Full read-back · 08 Nov", eligibility: "Read only", health: "Healthy", tone: "offsite", used: 95 },
      { id: "TS-L9-004199", barcode: "TS4199L9", generation: "LTO-9", format: "LTO-9", pool: "Routine · Weekly 45", contents: "Production full · volume 1", location: "Vault North · B-17", movement: "Confirmed 09 Nov", retention: "Protected through 13 Dec", verification: "Full read-back · 08 Nov", eligibility: "Read only", health: "Healthy", tone: "offsite", used: 98 },
      { id: "TS-L9-003881", barcode: "TS3881L9", generation: "LTO-9", format: "LTO-9", pool: "VM native", contents: "4 native recovery points", location: "Library A · Slot 9", movement: "Next export 15 Nov", retention: "Protected through 17 Dec", verification: "Full read-back · 11 Nov", eligibility: "Append eligible", health: "Healthy", tone: "success", used: 62 },
      { id: "TS-L8-002184", barcode: "TS2184L8", generation: "LTO-8", format: "LTO-8", pool: "Team homes", contents: "job-1049 · active", location: "Library A · Drive 2", movement: "None", retention: "Protected · pending", verification: "In progress · 82%", eligibility: "Active verification", health: "Healthy", tone: "writing", used: 48 },
      { id: "TS-L8-002177", barcode: "TS2177L8", generation: "LTO-8", format: "LTO-8", pool: "Team homes", contents: "7 file recovery points", location: "Vault North · C-04", movement: "Recall due 14 Nov", retention: "Protected through 10 Dec", verification: "Restore tested · 10 Nov", eligibility: "Read only", health: "Healthy", tone: "offsite", used: 81 },
      { id: "TS-L7M-000071", barcode: "TS0071M8", generation: "LTO-7 Type M", format: "LTO-7 Type M", pool: "Legacy import", contents: "Foreign catalog · not imported", location: "Intake safe · Shelf 2", movement: "None", retention: "Protected · unknown", verification: "Not verified", eligibility: "Overwrite blocked", health: "Identity unresolved", tone: "blocked", used: null },
      { id: "TS-L6-000308", barcode: null, generation: "LTO-6", format: "LTO-6", pool: "Indefinite archives", contents: "Finance archive FY2020 · copy 1", location: "Iron Mountain · Account 5819", movement: "Confirmed 04 Jan 2021", retention: "Indefinite hold", verification: "Annual read-back · 03 Oct", eligibility: "Never recycle", health: "Healthy", tone: "offsite", used: 73 }
    ],
    drives: [
      { id: "drive-1", name: "Drive 1", model: "IBM ULTRIUM-HH9", serial: "10WT020531", wwn: "500507630F00A149", generation: "LTO-9", path: "/dev/tape/by-id/scsi-3500507630f00a149-nst", status: "waiting for media", loaded: null, rate: "0 MiB/s", firmware: "Q3J1", health: "Healthy" },
      { id: "drive-2", name: "Drive 2", model: "IBM ULTRIUM-HH8", serial: "10WT018922", wwn: "500507630F00B812", generation: "LTO-8", path: "/dev/tape/by-id/scsi-3500507630f00b812-nst", status: "verifying", loaded: "TS-L8-002184", rate: "286 MiB/s", firmware: "S7Q0", health: "Healthy" },
      { id: "drive-3", name: "Standalone recovery drive", model: "HPE Ultrium 7-SCSI", serial: "HU1918M4D2", wwn: "500110A0018E4902", generation: "LTO-7", path: "/dev/tape/by-id/scsi-3500110a0018e4902-nst", status: "offline", loaded: null, rate: "—", firmware: "G9Q1", health: "Path changed · reconcile" }
    ],
    slots: [
      [1, "TS-L9-003872", "loaded"], [2, "TS-L9-003874", "loaded"], [3, null, ""], [4, "TS-L9-003876", "loaded"],
      [5, "TS-L9-003878", "loaded"], [6, null, ""], [7, "CLN-LTO-019", "alert"], [8, null, ""],
      [9, "TS-L9-003881", "reserved"], [10, "TS-L9-004190", "loaded"], [11, null, ""], [12, "TS-L9-004194", "loaded"],
      [13, "TS-L9-004196", "loaded"], [14, null, ""], [15, "TS-L9-004198", "loaded"], [16, null, ""],
      [17, "TS-L9-004218", "reserved"], [18, "TS-L9-004220", "reserved"], [19, null, ""], [20, "TS-L8-002180", "loaded"],
      [21, "TS-L8-002181", "loaded"], [22, null, ""], [23, "TS-L8-002183", "loaded"], [24, null, ""]
    ],
    movements: [
      {
        id: "move-402",
        title: "Weekly 46 · take off site",
        kind: "Take off site",
        source: "Library A · Export station",
        destination: "Vault North · B-18",
        due: "Today · 16:00 UTC",
        status: "blocked",
        reason: "Maintain one complete verified recovery point off site",
        readiness: "Awaiting job-1048 and full read-back",
        assigned: "Brandon Ragsdale",
        stage: 1,
        tapes: [
          { id: "TS-L9-004218", reason: "Production chain · volume 1", checked: false, ready: false },
          { id: "TS-L9-004219", reason: "Production chain · volume 2", checked: false, ready: false },
          { id: "TS-L9-004220", reason: "Production chain · continuation", checked: false, ready: false }
        ],
        history: [["Planned", "12 Nov 08:00 UTC", "System · rotation plan Weekly 46"]]
      },
      {
        id: "move-399",
        title: "Archive copy 2 · deliver",
        kind: "Take off site",
        source: "Secure staging cabinet",
        destination: "Iron Mountain · Account 5819",
        due: "Overdue · 11 Nov 17:00 UTC",
        status: "overdue",
        reason: "Second independent copy for Project Atlas archive",
        readiness: "Ready · all export gates passed",
        assigned: "Maya Chen",
        stage: 2,
        tapes: [
          { id: "TS-L9-A00031", reason: "Atlas archive · volume 1 of 2", checked: true, ready: true },
          { id: "TS-L9-A00032", reason: "Atlas archive · volume 2 of 2", checked: true, ready: true }
        ],
        history: [
          ["Departure confirmed", "11 Nov 16:42 UTC", "Maya Chen · manifest M-2026-399"],
          ["Ready for export", "11 Nov 09:14 UTC", "Full verification and recovery package passed"],
          ["Planned", "10 Nov 18:00 UTC", "System · archive policy"]
        ]
      },
      {
        id: "move-401",
        title: "Team homes · verification recall",
        kind: "Bring back",
        source: "Vault North · C-04",
        destination: "Library A · Intake",
        due: "14 Nov · 10:00 UTC",
        status: "planned",
        reason: "Annual qualifying read-back",
        readiness: "Waiting for replacement off-site coverage",
        assigned: "Unassigned",
        stage: 1,
        tapes: [
          { id: "TS-L8-002177", reason: "Annual media audit", checked: false, ready: true }
        ],
        history: [["Planned", "12 Nov 07:18 UTC", "System · retained-media audit"]]
      },
      {
        id: "move-398",
        title: "Weekly 45 · confirmed delivery",
        kind: "Swap complete",
        source: "Library A · Export station",
        destination: "Vault North · B-17",
        due: "Completed 09 Nov",
        status: "successful",
        reason: "Routine off-site protection",
        readiness: "Complete · custody confirmed",
        assigned: "Brandon Ragsdale",
        stage: 4,
        tapes: [
          { id: "TS-L9-004199", reason: "Production full · volume 1", checked: true, ready: true },
          { id: "TS-L9-004201", reason: "Production full · volume 2", checked: true, ready: true },
          { id: "TS-L9-004203", reason: "Production full · volume 3", checked: true, ready: true }
        ],
        history: [
          ["Arrival confirmed", "09 Nov 13:08 UTC", "Brandon Ragsdale · Vault North"],
          ["Departure confirmed", "09 Nov 11:22 UTC", "Brandon Ragsdale · Library A"],
          ["Ready for export", "09 Nov 08:11 UTC", "Full read-back passed"],
          ["Planned", "08 Nov 18:00 UTC", "System · rotation plan Weekly 45"]
        ]
      }
    ],
    verification: [
      { id: "ver-802", target: "Team homes incremental", scope: "Full application read-back", media: "TS-L8-002184", drive: "Drive 2", started: "12 Nov · 11:12 UTC", result: "verifying", detail: "82% · 1.96 TiB authenticated", evidence: "Pending" },
      { id: "ver-801", target: "VM native stream protection", scope: "Native stream decode + SHA-256", media: "TS-L9-003881", drive: "Drive 1", started: "11 Nov · 23:42 UTC", result: "verified", detail: "638 GiB · 0 errors", evidence: "EV-2026-00801" },
      { id: "ver-800", target: "Monthly representative restore drill", scope: "1,240 selected files + metadata", media: "TS-L8-002177", drive: "Drive 2", started: "10 Nov · 14:00 UTC", result: "successful", detail: "84.2 GiB restored · hashes and ACLs matched", evidence: "DRILL-2026-011" },
      { id: "ver-799", target: "Weekly production full", scope: "Full application read-back", media: "TS-L9-004199 · 004205", drive: "Drive 1", started: "08 Nov · 08:41 UTC", result: "interrupted", detail: "Drive reset during finalization · no pass recorded", evidence: "INC-2026-0041" },
      { id: "ver-798", target: "Finance archive FY2020", scope: "Annual qualifying read-back", media: "TS-L6-000308", drive: "Example recovery drive", started: "03 Oct · 09:00 UTC", result: "verified", detail: "2.28 TiB · 0 errors · 4h 21m", evidence: "EV-2026-00798" }
    ],
    recoveryPoints: [
      { id: "rp-1112", source: "tank/homes", label: "Team homes · 12 Nov 2026 02:48 UTC", type: "File archive · incremental", verified: "Read-back in progress", tapes: ["TS-L8-002184"], location: "Library A", size: "1.12 TiB", complete: true },
      { id: "rp-1109", source: "tank/homes", label: "Team homes · 09 Nov 2026 03:06 UTC", type: "File archive · incremental", verified: "Full read-back passed", tapes: ["TS-L8-002177"], location: "Vault North · C-04", size: "1.08 TiB", complete: false },
      { id: "rp-vm-1111", source: "fastpool/virtualization/production-zvols", label: "VM volumes · 11 Nov 2026 23:42 UTC", type: "Native ZFS · incremental", verified: "Native stream verified", tapes: ["TS-L9-003881"], location: "Library A · Slot 9", size: "638 GiB", complete: true },
      { id: "rp-prod-1108", source: "tank/production/media", label: "Production media · 08 Nov 2026 08:40 UTC", type: "File + native ZFS · full", verified: "Interrupted · partial evidence", tapes: ["TS-L9-004199", "TS-L9-004201", "TS-L9-004203", "TS-L9-004205"], location: "Split · Vault North / Library A", size: "41.7 TiB", complete: false }
    ],
    preflight: [
      { name: "PostgreSQL durable state", status: "simulated", detail: "Preview target · migration 0001" },
      { name: "Tape engine selection", status: "warning", detail: "Unselected · Bacula 15.x is a feasibility candidate only" },
      { name: "TrueNAS API", status: "pass", detail: "JSON-RPC 25.10 · certificate verified" },
      { name: "Snapshot file path", status: "pass", detail: "Read-only mount visible" },
      { name: "Native ZFS path", status: "pass", detail: "Constrained helper available" },
      { name: "Drive identities", status: "warning", detail: "2 ready · 1 path change requires reconciliation" },
      { name: "Changer access", status: "pass", detail: "IBM TS3100 · 24 slots · stable serial" },
      { name: "Scratch capacity", status: "pass", detail: "18.4 TiB free · 12.0 TiB reserve" },
      { name: "Recovery key package", status: "pass", detail: "Last independently tested 03 Nov" },
      { name: "Source ACL access", status: "pass", detail: "POSIX and NFSv4 fixtures readable" },
      { name: "Notification channels", status: "pass", detail: "In-app + SMTP verified" },
      { name: "Configuration export", status: "fail", detail: "FULL_ADMIN credential required" }
    ],
    qualification: {
      platform: "TrueNAS 25.10.8 target · not yet qualified",
      openzfs: "Host-provided · version not yet qualified",
      engine: "Unselected · Bacula 15.x feasibility candidate only",
      image: "ghcr.io/lilrags16/tapescale:0.1.0-dev",
      generations: [
        { generation: "LTO-1", reads: "LTO-1", writes: "LTO-1", model: "Capability model", physical: "Not yet qualified" },
        { generation: "LTO-2", reads: "LTO-1, LTO-2", writes: "LTO-1, LTO-2", model: "Capability model", physical: "Not yet qualified" },
        { generation: "LTO-3", reads: "LTO-1–3", writes: "LTO-2–3", model: "Capability model", physical: "Not yet qualified" },
        { generation: "LTO-4", reads: "LTO-2–4", writes: "LTO-3–4", model: "Capability model", physical: "Not yet qualified" },
        { generation: "LTO-5", reads: "LTO-3–5", writes: "LTO-4–5", model: "Capability model", physical: "Not yet qualified" },
        { generation: "LTO-6", reads: "LTO-4–6", writes: "LTO-5–6", model: "Capability model", physical: "Not yet qualified" },
        { generation: "LTO-7", reads: "LTO-5–7", writes: "LTO-6–7", model: "Capability model", physical: "Not yet qualified" },
        { generation: "LTO-8", reads: "LTO-7, 7M, 8", writes: "LTO-7, 7M, 8", model: "Capability model", physical: "Not yet qualified" },
        { generation: "LTO-9", reads: "LTO-8, LTO-9", writes: "LTO-8, LTO-9", model: "Capability model", physical: "Not yet qualified" },
        { generation: "LTO-10", reads: "LTO-10", writes: "LTO-10", model: "Capability model", physical: "Not yet qualified" }
      ]
    }
  };

  const clone = (value) => JSON.parse(JSON.stringify(value));

  const state = {
    route: "overview",
    subroute: null,
    data: clone(DEMO),
    live: {},
    loading: true,
    search: "",
    tapeFilter: "all",
    jobFilter: "all",
    selectedMovement: "move-402",
    settingsTab: "preflight",
    scenario: sessionStorage.getItem("tapescale-scenario") || "realistic",
    restore: {
      step: 1,
      recoveryPoint: "rp-1112",
      selectedFiles: ["/Brandon", "/Engineering/roadmap-2027.xlsx"],
      mode: "selected",
      recallCreated: false,
      destination: "tank/restores/team-homes-2026-11-12",
      conflicts: "skip",
      queued: false
    },
    lastFocus: null
  };

  function escapeHtml(value) {
    return String(value == null ? "" : value)
      .replaceAll("&", "&amp;")
      .replaceAll("<", "&lt;")
      .replaceAll(">", "&gt;")
      .replaceAll('"', "&quot;")
      .replaceAll("'", "&#039;");
  }

  function toneFor(status) {
    const value = String(status || "").toLowerCase();
    if (/fail|error|blocked/.test(value)) return "failed";
    if (/wait|overdue|interrupt|pending|investigate/.test(value)) return "warning";
    if (/verif|success|healthy|pass|complete|ready/.test(value)) return "success";
    if (/write/.test(value)) return "writing";
    if (/queue|prepar|progress/.test(value)) return "info";
    if (/off.?site|planned/.test(value)) return "offsite";
    return "info";
  }

  function titleCase(value) {
    return String(value || "")
      .replaceAll("_", " ")
      .replace(/\b\w/g, (char) => char.toUpperCase());
  }

  function statusTag(label, tone) {
    const resolved = tone || toneFor(label);
    return `<span class="status ${escapeHtml(resolved)}">${escapeHtml(titleCase(label))}</span>`;
  }

  function tapeId(id, large = false) {
    return `<span class="tape-id${large ? " large" : ""}">${escapeHtml(id || "Unassigned")}</span>`;
  }

  function pageHead(title, description, actions = "") {
    return `<header class="page-head">
      <div class="page-title-block"><h1>${escapeHtml(title)}</h1><p>${escapeHtml(description)}</p></div>
      ${actions ? `<div class="page-actions">${actions}</div>` : ""}
    </header>`;
  }

  function emptyState(title, message, actionLabel = "Configure TapeScale", action = "configure") {
    return `<div class="panel empty-state"><div class="empty-state-inner">
      <div class="empty-symbol" aria-hidden="true">&#9638;</div>
      <h2>${escapeHtml(title)}</h2><p>${escapeHtml(message)}</p>
      <button class="button button-primary" type="button" data-action="${escapeHtml(action)}">${escapeHtml(actionLabel)}</button>
    </div></div>`;
  }

  function progressBar(value, label, tone = "") {
    const safe = Math.max(0, Math.min(100, Number(value) || 0));
    return `<div class="progress" aria-label="${escapeHtml(label || `${safe}% complete`)}">
      <div class="progress-track"><div class="progress-value ${escapeHtml(tone)}" style="width:${safe}%"></div></div>
      <div class="progress-meta"><span>${escapeHtml(label || "Progress")}</span><span>${safe}%</span></div>
    </div>`;
  }

  function formatDate(value) {
    if (!value) return "—";
    const parsed = new Date(value);
    if (Number.isNaN(parsed.getTime())) return String(value);
    return new Intl.DateTimeFormat("en", {
      day: "2-digit", month: "short", hour: "2-digit", minute: "2-digit", timeZone: "UTC", hour12: false
    }).format(parsed).replace(",", "") + " UTC";
  }

  function formatBytes(value) {
    const bytes = Number(value);
    if (!Number.isFinite(bytes) || bytes <= 0) return "—";
    const units = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    const power = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
    return `${(bytes / (1024 ** power)).toFixed(power > 2 ? 2 : 1)} ${units[power]}`;
  }

  function unwrapList(value) {
    if (Array.isArray(value)) return value;
    if (!value || typeof value !== "object") return [];
    for (const key of ["items", "data", "jobs", "media", "tapes", "drives", "movements", "tasks", "results"]) {
      if (Array.isArray(value[key])) return value[key];
    }
    return [];
  }

  async function apiGet(path, timeout = 1800) {
    const controller = new AbortController();
    const timer = window.setTimeout(() => controller.abort(), timeout);
    try {
      const response = await fetch(`${API_BASE}${path}`, {
        headers: { Accept: "application/json" },
        credentials: "same-origin",
        signal: controller.signal
      });
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      const data = await response.json();
      return { ok: true, data };
    } catch (error) {
      return { ok: false, error: error instanceof Error ? error.message : String(error) };
    } finally {
      window.clearTimeout(timer);
    }
  }

  async function apiPost(path, body) {
    const controller = new AbortController();
    const timer = window.setTimeout(() => controller.abort(), 3500);
    try {
      const response = await fetch(`${API_BASE}${path}`, {
        method: "POST",
        headers: { Accept: "application/json", "Content-Type": "application/json" },
        credentials: "same-origin",
        body: JSON.stringify(body),
        signal: controller.signal
      });
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      const data = response.status === 204 ? null : await response.json();
      return { ok: true, data };
    } catch (error) {
      return { ok: false, error: error instanceof Error ? error.message : String(error) };
    } finally {
      window.clearTimeout(timer);
    }
  }

  function normalizeJob(job, index) {
    const fallback = DEMO.jobs[index % DEMO.jobs.length];
    const apiShape = Object.hasOwn(job, "state") || Object.hasOwn(job, "started_at");
    const stateName = job.status || job.state || fallback.status;
    const statePhase = /verified|successful/.test(stateName) ? 6 : /verifying/.test(stateName) ? 5 : /written|finaliz/.test(stateName) ? 4 : /writing|waiting/.test(stateName) ? 3 : /snapshot/.test(stateName) ? 2 : 1;
    return {
      ...fallback,
      ...job,
      id: job.id || job.job_id || fallback.id,
      name: job.name || job.policy_name || job.title || fallback.name,
      source: job.source || job.source_path || fallback.source,
      status: stateName,
      progress: job.progress ?? job.progress_percent ?? fallback.progress,
      started: job.started || job.started_at || fallback.started,
      finished: job.finished || job.finished_at || fallback.finished,
      phase: typeof job.phase === "number" ? job.phase : statePhase,
      kind: job.kind || (apiShape ? "Scheduled" : fallback.kind),
      logical: job.logical || (apiShape ? "—" : fallback.logical),
      encoded: job.encoded || (apiShape ? "—" : fallback.encoded),
      sourceRate: job.sourceRate || job.source_rate || (apiShape ? "—" : fallback.sourceRate),
      driveRate: job.driveRate || job.drive_rate || (apiShape ? "—" : fallback.driveRate),
      currentTape: job.currentTape || job.current_tape || (apiShape ? "—" : fallback.currentTape),
      media: job.media || job.tape_ids || (apiShape ? [] : fallback.media),
      events: job.events || (apiShape ? [[formatDate(job.started_at), "Attempt recorded", job.state_label || titleCase(stateName)]] : fallback.events),
      attempt: String(job.attempt || fallback.attempt),
      snapshot: job.snapshot || job.recovery_point || fallback.snapshot,
      eta: job.eta || job.state_label || titleCase(stateName)
    };
  }

  function normalizeTape(item, index) {
    const fallback = DEMO.tapes[index % DEMO.tapes.length];
    return {
      ...fallback,
      ...item,
      id: item.id || item.media_id || item.internal_id || fallback.id,
      generation: item.generation || item.media_generation || fallback.generation,
      status: item.status || item.state,
      movement: item.movement || item.planned || "None",
      eligibility: item.eligibility || item.overwrite || (item.write_eligible ? "Write eligible" : "Read only"),
      used: item.used ?? null,
      tone: item.tone || (/unknown|blocked/i.test(`${item.id} ${item.overwrite}`) ? "blocked" : /vault|off.?site/i.test(item.location || "") ? "offsite" : toneFor(item.status || item.health || fallback.tone))
    };
  }

  function normalizeDrive(item, index) {
    const fallback = DEMO.drives[index % DEMO.drives.length];
    return {
      ...fallback,
      ...item,
      id: item.id || item.drive_id || fallback.id,
      name: item.name || item.display_name || fallback.name,
      status: item.status || item.state_label || item.state || fallback.status,
      loaded: item.loaded || item.loaded_tape || null,
      rate: item.rate || "—",
      firmware: item.firmware || "Not reported"
    };
  }

  function normalizeMovement(item, index) {
    const fallback = DEMO.movements[index % DEMO.movements.length];
    return {
      ...fallback,
      ...item,
      id: item.id || item.movement_id || fallback.id,
      title: item.title || item.name || fallback.title,
      kind: item.kind || item.type || fallback.kind,
      status: item.status || item.state || fallback.status,
      due: item.due || formatDate(item.due_at) || fallback.due,
      readiness: item.readiness || (item.ready ? "Ready · export gates passed" : "Blocked · readiness gate outstanding"),
      assigned: item.assigned || "Unassigned",
      stage: item.stage || (/confirmed/.test(item.state || "") ? 4 : /partial|overdue/.test(item.state || "") ? 3 : item.ready ? 2 : 1),
      tapes: (item.tapes || item.media || fallback.tapes).map((tape) => ({ ...tape, checked: tape.checked ?? tape.confirmed ?? false, ready: tape.ready ?? item.ready ?? false, reason: tape.reason || item.reason || "Required complete-set member" })),
      history: item.history || item.events || [["Task generated", formatDate(item.due_at), "TapeScale rotation plan"]]
    };
  }

  async function hydrateData(showFeedback = false) {
    if (showFeedback) {
      state.loading = true;
      renderLoading();
    }
    const requests = await Promise.all([
      apiGet("/overview"),
      apiGet("/jobs"),
      apiGet("/media"),
      apiGet("/drives"),
      apiGet("/movements"),
      apiGet("/preflight"),
      apiGet("/qualification")
    ]);
    const [overview, jobs, media, drives, movements, preflight, qualification] = requests;
    state.live = {
      overview: overview.ok,
      jobs: jobs.ok,
      media: media.ok,
      drives: drives.ok,
      movements: movements.ok,
      preflight: preflight.ok,
      qualification: qualification.ok
    };

    if (overview.ok && overview.data && typeof overview.data === "object") {
      state.data.overview = { ...state.data.overview, ...overview.data };
      if (Array.isArray(overview.data.sources)) state.data.sources = overview.data.sources;
      if (Array.isArray(overview.data.coverage)) state.data.sources = overview.data.coverage.map((source) => ({
        id: source.id,
        name: String(source.source || source.id).split("/").slice(-2).join(" / "),
        path: source.source,
        mode: source.mode,
        localDate: source.local?.time ? formatDate(source.local.time) : "No local point",
        offsiteDate: source.offsite?.time ? formatDate(source.offsite.time) : "No off-site point",
        localPoints: Math.max(source.points?.length || 0, 3),
        offsitePoints: source.offsite?.time ? 3 : 1,
        status: source.gap ? "warning" : "success",
        statusTitle: source.gap ? "Protection gap" : "Coverage complete",
        statusDetail: source.gap || source.offsite?.label || "Complete recovery point off site"
      }));
      if (overview.data.summary) state.data.overview.stats = {
        protected: `${overview.data.summary.protected_sources || 0} sources`,
        activeJobs: overview.data.summary.active_jobs,
        requiredActions: overview.data.summary.required_actions,
        nextWindow: "Schedule active"
      };
      if (Array.isArray(overview.data.verification)) state.data.verification = overview.data.verification;
    }
    if (jobs.ok) state.data.jobs = unwrapList(jobs.data).map(normalizeJob);
    if (media.ok) state.data.tapes = unwrapList(media.data).map(normalizeTape);
    if (drives.ok) {
      state.data.drives = unwrapList(drives.data).map(normalizeDrive);
      if (Array.isArray(drives.data.slots)) state.data.slots = drives.data.slots.map((slot) => [slot.slot, slot.tape, slot.state === "unknown" || slot.state === "exported_not_delivered" ? "alert" : slot.tape ? "loaded" : ""]);
    }
    if (movements.ok) state.data.movements = unwrapList(movements.data).map(normalizeMovement);
    if (preflight.ok) state.data.preflight = (unwrapList(preflight.data).length ? unwrapList(preflight.data) : (preflight.data.checks || [])).map((check) => ({ ...check, name: check.name || check.label || check.id, status: check.status || check.state || "not run" }));
    if (qualification.ok) {
      const payload = qualification.data || {};
      state.data.qualification = {
        ...state.data.qualification,
        platform: payload.claims?.truenas || state.data.qualification.platform,
        engine: payload.engine ? `${payload.engine.candidate || "Unselected"} · ${payload.engine.selection || "selection pending"}` : state.data.qualification.engine,
        gate: payload.gate || "G0 not passed"
      };
    }

    state.loading = false;
    updateServiceHealth();
    updateNavCounts();
    if (showFeedback) toast("Inventory refreshed", anyLive() ? "Live API data has been reconciled." : "API unavailable; retained embedded operational fixtures.");
  }

  function anyLive() {
    return Object.values(state.live).some(Boolean);
  }

  function updateServiceHealth() {
    const container = $("#service-health");
    if (!container) return;
    const liveCount = Object.values(state.live).filter(Boolean).length;
    const simulation = state.data.overview?.mode === "simulation";
    container.innerHTML = anyLive()
      ? `<span class="health-dot${liveCount < 7 || simulation ? " warning" : ""}" aria-hidden="true"></span><span><strong>${simulation ? "Simulation connected" : liveCount === 7 ? "Services connected" : "Partial API data"}</strong><small>${liveCount}/7 preview endpoints responding</small></span>`
      : `<span class="health-dot warning" aria-hidden="true"></span><span><strong>Demo data</strong><small>API unavailable · safe fallback</small></span>`;
  }

  function updateNavCounts() {
    const jobCount = state.data.jobs.filter((job) => /writing|verifying|waiting|preparing|queued/.test(String(job.status).toLowerCase())).length;
    const moveCount = state.data.movements.filter((item) => !/success|complete/.test(String(item.status).toLowerCase())).length;
    const jobNode = $("#nav-job-count");
    const moveNode = $("#nav-move-count");
    if (jobNode) jobNode.textContent = String(jobCount);
    if (moveNode) moveNode.textContent = String(moveCount);
  }

  function scenarioNotice() {
    if (state.scenario === "failure") {
      return `<div class="notice danger section" role="alert">
        <span class="notice-icon" aria-hidden="true">!</span>
        <div class="notice-copy"><strong>TrueNAS control connection unavailable</strong><span>Live inventory is retained from the last successful reconciliation. Writes and destructive actions are blocked until certificate-verified API access returns.</span></div>
        <button class="button button-small" type="button" data-action="refresh">Retry connection</button>
      </div>`;
    }
    if (!anyLive()) {
      return `<div class="notice info" role="status">
        <span class="notice-icon" aria-hidden="true">i</span>
        <div class="notice-copy"><strong>Embedded operations preview</strong><span>The same-origin API is not responding, so TapeScale is showing a deterministic, fully interactive operating fixture.</span></div>
        <button class="button button-small" type="button" data-action="refresh">Try live API</button>
      </div>`;
    }
    if (state.data.overview?.mode === "simulation") {
      return `<div class="notice info" role="status">
        <span class="notice-icon" aria-hidden="true">i</span>
        <div class="notice-copy"><strong>Engineering preview · simulation evidence only</strong><span>These workflows use deterministic operating fixtures. No displayed tape, TrueNAS, verification, custody, or recovery event is physical qualification evidence.</span></div>
      </div>`;
    }
    return "";
  }

  function recoveryRail(source) {
    const point = (partial = false) => `<span class="rail-point${partial ? " partial" : ""}" aria-hidden="true"></span>`;
    const local = Array.from({ length: Math.max(1, source.localPoints || 5) }, () => point()).join("");
    const offsiteCount = Math.max(1, source.offsitePoints || 3);
    const offsite = Array.from({ length: offsiteCount }, (_, index) => point(source.status === "warning" && index === offsiteCount - 1)).join("");
    return `<div class="recovery-rail" aria-label="Recovery coverage: local through ${escapeHtml(source.localDate)}, off site through ${escapeHtml(source.offsiteDate)}">
      <div class="rail-row"><span class="rail-label">Local</span><div class="rail-track">${local}</div></div>
      <div class="rail-row offsite"><span class="rail-label">Off site</span><div class="rail-track ${source.status === "warning" ? "gap" : ""}">${offsite}</div></div>
      <div class="rail-dates"><span>${escapeHtml(source.offsiteDate)}</span><span>${escapeHtml(source.localDate)}</span></div>
    </div>`;
  }

  function renderOverview() {
    const sources = state.scenario === "empty" ? [] : state.data.sources;
    const activeJobs = state.data.jobs.filter((job) => /writing|verifying|waiting|preparing|queued/.test(String(job.status).toLowerCase()));
    const stats = state.data.overview.stats || DEMO.overview.stats;
    const actions = `<button class="button" type="button" data-action="refresh">&#8635; Refresh</button>
      <button class="button button-primary" type="button" data-action="new-backup">+ Configure protection</button>`;

    let html = pageHead("Operational overview", "Recovery posture, active tape work, and the physical actions that need attention.", actions);
    html += scenarioNotice();
    if (!sources.length) {
      html += `<div class="section">${emptyState("No protected sources yet", "Connect TrueNAS, discover a dataset, and configure the first protection policy.")}</div>`;
      setMain(html, "Operational overview");
      return;
    }

    html += `<section class="section" aria-labelledby="posture-title">
      <div class="section-head"><div><h2 id="posture-title">Recovery posture</h2><p>Complete recovery points, not isolated cartridges</p></div><a class="section-link" href="#verification">View evidence &#8594;</a></div>
      <div class="panel posture-table">
        ${sources.map((source) => `<article class="posture-row">
          <div class="source-name"><strong title="${escapeHtml(source.path)}">${escapeHtml(source.name)}</strong><small><code>${escapeHtml(source.path)}</code></small><small>${escapeHtml(source.mode)}</small></div>
          ${recoveryRail(source)}
          <div class="posture-status ${source.status === "warning" ? "warning" : ""}"><strong>${escapeHtml(source.statusTitle)}</strong><span>${escapeHtml(source.statusDetail)}</span></div>
        </article>`).join("")}
      </div>
    </section>`;

    html += `<section class="section metric-strip" aria-label="Current operating measurements">
      <div class="metric"><span>Protected source data</span><strong>${escapeHtml(stats.protected || "148.6 TiB")}</strong><small>Logical bytes in retained points</small></div>
      <div class="metric"><span>Active tape work</span><strong>${escapeHtml(stats.activeJobs ?? activeJobs.length)}</strong><small>Writing, verifying, or waiting</small></div>
      <div class="metric"><span>Required actions</span><strong>${escapeHtml(stats.requiredActions ?? 4)}</strong><small>2 overdue · 2 blocked</small></div>
      <div class="metric"><span>Next backup window</span><strong>${escapeHtml(stats.nextWindow || "14h 18m")}</strong><small>Full · Saturday at 02:00</small></div>
    </section>`;

    html += `<div class="overview-grid section">
      <section class="panel table-panel" aria-labelledby="active-title">
        <div class="panel-head"><div><h2 id="active-title">Active work</h2><p>Durable state from the orchestration service</p></div><a class="section-link" href="#jobs">All jobs &#8594;</a></div>
        ${renderActiveJobs(activeJobs)}
      </section>
      <section class="panel" aria-labelledby="actions-title">
        <div class="panel-head"><div><h2 id="actions-title">Operator queue</h2><p>Highest-impact action first</p></div><span class="status warning">4 unresolved</span></div>
        <ul class="action-list">
          <li class="action-item"><span class="action-symbol">&#8594;</span><div class="action-copy"><strong>Insert continuation tape</strong><span>${tapeId("TS-L9-004220")} for Nightly production protection</span></div><span class="action-time">8m</span></li>
          <li class="action-item"><span class="action-symbol failed">!</span><div class="action-copy"><strong>Archive delivery overdue</strong><span>Confirm arrival of 2 tapes at Iron Mountain</span></div><span class="action-time">18h</span></li>
          <li class="action-item"><span class="action-symbol">&#8635;</span><div class="action-copy"><strong>Reconcile standalone drive</strong><span>Stable identity found at a new device path</span></div><span class="action-time">2d</span></li>
          <li class="action-item"><span class="action-symbol failed">!</span><div class="action-copy"><strong>Configuration export failed</strong><span>TrueNAS credential lacks FULL_ADMIN authority</span></div><span class="action-time">13h</span></li>
        </ul>
      </section>
    </div>`;

    html += `<section class="section" aria-labelledby="devices-title">
      <div class="section-head"><h2 id="devices-title">Device activity</h2><a class="section-link" href="#drives">Library view &#8594;</a></div>
      <div class="device-strip">
        ${state.data.drives.map((drive) => `<article class="device-cell">
          <span class="drive-glyph" aria-hidden="true">&#9673;</span>
          <div><strong>${escapeHtml(drive.name)}</strong><span>${escapeHtml(drive.generation)} · ${escapeHtml(drive.loaded || drive.health)}</span></div>
          <span class="device-rate">${escapeHtml(drive.rate)}</span>
        </article>`).join("")}
      </div>
    </section>`;

    setMain(html, "Operational overview");
  }

  function renderActiveJobs(jobs) {
    if (!jobs.length) return `<div class="empty-state"><div class="empty-state-inner"><div class="empty-symbol" aria-hidden="true">&#10003;</div><h2>No active operations</h2><p>All scheduled work is complete. The next window is Saturday at 02:00.</p></div></div>`;
    return `<div class="table-scroll"><table class="data-table responsive-table"><thead><tr><th>Job</th><th>State</th><th>Progress</th><th>Drive / media</th><th>Rate</th></tr></thead><tbody>
      ${jobs.map((job) => `<tr class="row-action" data-job-id="${escapeHtml(job.id)}" tabindex="0">
        <td class="primary-cell" data-label="Job"><strong>${escapeHtml(job.name)}</strong><small>${escapeHtml(job.kind)} · ${escapeHtml(job.mode)}</small></td>
        <td data-label="State">${statusTag(job.status)}</td>
        <td data-label="Progress">${progressBar(job.progress, job.eta, /wait|interrupt/.test(job.status) ? "warning" : "")}</td>
        <td data-label="Drive / media">${job.currentTape && job.currentTape !== "—" ? tapeId(job.currentTape) : "—"}<small class="mono"> ${escapeHtml(job.drive || "")}</small></td>
        <td data-label="Rate"><span class="mono">${escapeHtml(job.driveRate || "—")}</span></td>
      </tr>`).join("")}
    </tbody></table></div>`;
  }

  function renderJobs() {
    if (state.subroute) {
      renderJobDetail(state.subroute);
      return;
    }
    const jobs = state.scenario === "empty" ? [] : state.data.jobs;
    const query = state.search.toLowerCase();
    const filtered = jobs.filter((job) => {
      const matchesQuery = !query || `${job.name} ${job.source} ${job.status} ${job.id}`.toLowerCase().includes(query);
      const matchesFilter = state.jobFilter === "all" || toneFor(job.status) === state.jobFilter || String(job.status).toLowerCase().includes(state.jobFilter);
      return matchesQuery && matchesFilter;
    });
    const actions = `<button class="button" type="button" data-action="refresh">&#8635; Refresh</button><button class="button button-primary" type="button" data-action="run-job">&#9654; Run permitted job</button>`;
    let html = pageHead("Jobs & history", "Every attempt, durable state transition, media request, and verification outcome.", actions);
    html += scenarioNotice();
    if (!jobs.length) {
      html += `<div class="section">${emptyState("No job history", "Protection jobs appear here after the first policy is enabled.", "Configure protection", "new-backup")}</div>`;
      setMain(html, "Jobs and history");
      return;
    }
    html += `<section class="panel table-panel section" aria-label="Job history">
      <div class="table-toolbar">
        <div class="toolbar-start"><label class="search-field"><span class="sr-only">Search jobs</span><input type="search" value="${escapeHtml(state.search)}" data-input="job-search" placeholder="Search job, source, or ID"></label></div>
        <div class="toolbar-end"><label><span class="sr-only">Filter by state</span><select class="select compact" data-input="job-filter"><option value="all">All states</option><option value="success">Successful</option><option value="warning">Needs attention</option><option value="failed">Failed</option></select></label><span>${filtered.length} attempts</span></div>
      </div>
      <div class="table-scroll"><table class="data-table responsive-table"><thead><tr><th>Job</th><th>State</th><th>Started</th><th>Representation</th><th>Data</th><th>Media</th></tr></thead><tbody>
        ${filtered.map((job) => `<tr class="row-action" data-job-id="${escapeHtml(job.id)}" tabindex="0">
          <td class="primary-cell" data-label="Job"><strong>${escapeHtml(job.name)}</strong><small>${escapeHtml(job.id)} · ${escapeHtml(job.source)}</small></td>
          <td data-label="State">${statusTag(job.status)}</td>
          <td data-label="Started"><span class="mono">${escapeHtml(formatDate(job.started))}</span></td>
          <td data-label="Representation">${escapeHtml(job.kind)}<br><small>${escapeHtml(job.mode)}</small></td>
          <td data-label="Data"><strong class="mono">${escapeHtml(job.logical)}</strong><br><small>${escapeHtml(job.encoded)} encoded</small></td>
          <td data-label="Media">${job.media.length ? `${tapeId(job.media[0])}${job.media.length > 1 ? ` <small>+${job.media.length - 1}</small>` : ""}` : "—"}</td>
        </tr>`).join("")}
      </tbody></table></div>
      <div class="table-foot"><span>Showing ${filtered.length} of ${jobs.length} attempts</span><span>History survives container replacement</span></div>
    </section>`;
    setMain(html, "Jobs and history");
    const filter = $("[data-input='job-filter']");
    if (filter) filter.value = state.jobFilter;
  }

  function renderJobDetail(jobId) {
    const job = state.data.jobs.find((item) => item.id === jobId) || state.data.jobs[0];
    if (!job) {
      setMain(pageHead("Job not found", "This attempt is no longer present in the retained job history.") + emptyState("Unknown job", "Return to job history and select another attempt.", "Back to jobs", "back-jobs"), "Job not found");
      return;
    }
    const phaseNames = ["Preparing", "Snapshot", "Writing", "Finalizing", "Verifying", "Verified"];
    const isAttention = /wait|interrupt|fail|block/.test(job.status);
    const actionNotice = /waiting/.test(job.status)
      ? `<div class="notice"><span class="notice-icon" aria-hidden="true">&#8594;</span><div class="notice-copy"><strong>Compatible continuation media required</strong><span>Insert ${escapeHtml(job.requestedTape || "an eligible cartridge")} in Library A. The protected snapshot and media reservation remain held.</span></div><button class="button button-small" data-action="media-instructions">Handling steps</button></div>`
      : job.error
        ? `<div class="notice danger"><span class="notice-icon" aria-hidden="true">!</span><div class="notice-copy"><strong>${/interrupt/.test(job.status) ? "Safe reconciliation required" : "Attempt did not satisfy its policy"}</strong><span>${escapeHtml(job.error)}</span></div><button class="button button-small" data-action="job-diagnostics">Diagnostics</button></div>`
        : `<div class="notice success"><span class="notice-icon" aria-hidden="true">&#10003;</span><div class="notice-copy"><strong>${/verifying/.test(job.status) ? "Write finalized; verification remains in progress" : "Recovery evidence retained"}</strong><span>${/verifying/.test(job.status) ? "The backup is Written but will not become export-ready until full read-back passes." : "This attempt reached its declared durable recovery state."}</span></div></div>`;

    let html = `<button class="button button-quiet button-small" type="button" data-action="back-jobs">&#8592; Job history</button>
      <header class="job-header section">
        <div><div class="job-kicker">${statusTag(job.status)} <span class="mono">${escapeHtml(job.id)}</span></div><h1>${escapeHtml(job.name)}</h1><p>${escapeHtml(job.source)}</p></div>
        <div class="job-summary">
          <div><span>Attempt</span><strong>${escapeHtml(job.attempt)}</strong></div><div><span>Started</span><strong>${escapeHtml(formatDate(job.started))}</strong></div>
          <div><span>Representation</span><strong>${escapeHtml(job.mode)}</strong></div><div><span>Recovery point</span><strong>${escapeHtml(job.snapshot)}</strong></div>
        </div>
      </header>${actionNotice}
      <section class="panel section" aria-labelledby="phase-title"><div class="panel-head"><div><h2 id="phase-title">Recovery lifecycle</h2><p>Written and Verified are deliberately separate durable states</p></div><span class="status ${toneFor(job.status)}">${escapeHtml(job.eta)}</span></div>
        <div class="phase-track">${phaseNames.map((name, index) => {
          const position = index + 1;
          const cls = position < job.phase ? "done" : position === job.phase ? "current" : "";
          return `<div class="phase ${cls}"><span class="phase-marker">${position < job.phase ? "&#10003;" : ""}</span><div><strong>${name}</strong><small>${position < job.phase ? "Durable" : position === job.phase ? "Current state" : "Pending"}</small></div></div>`;
        }).join("")}</div>
      </section>
      <section class="section metric-strip" aria-label="Attempt measurements">
        <div class="metric"><span>Logical source</span><strong>${escapeHtml(job.logical)}</strong><small>Selected snapshot</small></div>
        <div class="metric"><span>Encoded to media</span><strong>${escapeHtml(job.encoded)}</strong><small>Not a capacity claim</small></div>
        <div class="metric"><span>Source throughput</span><strong>${escapeHtml(job.sourceRate)}</strong><small>Current / final</small></div>
        <div class="metric"><span>Drive throughput</span><strong>${escapeHtml(job.driveRate)}</strong><small>Current / final</small></div>
      </section>
      <div class="grid-2 section">
        <section class="panel"><div class="panel-head"><div><h2>Media sequence</h2><p>Required order and continuation boundaries</p></div><span>${job.media.length} volumes</span></div>
          <div class="panel-body">${job.media.length ? job.media.map((id, index) => `<div class="readiness-item ${id === job.requestedTape ? "blocked" : ""}"><span class="gate-icon">${id === job.requestedTape ? "!" : "&#10003;"}</span><div><strong>${tapeId(id)}</strong><span>Volume ${index + 1} of ${job.media.length}${index < job.media.length - 1 ? " · continuation boundary recorded" : ""}</span></div>${id === job.requestedTape ? statusTag("Required", "warning") : statusTag("Finalized", "success")}</div>`).join("") : "<p>No physical media assigned.</p>"}</div>
        </section>
        <section class="panel"><div class="panel-head"><div><h2>Verification evidence</h2><p>Application evidence, not drive acknowledgement</p></div></div>
          <div class="panel-body"><div class="evidence-stamp"><span class="evidence-seal">${/verif|success/.test(job.status) ? "&#10003;" : "…"}</span><span><strong>${/verif|success/.test(job.status) ? "Recovery-path read-back" : "No passing evidence yet"}</strong><small>${/verifying/.test(job.status) ? `${job.progress}% read through decode path` : /success|verified/.test(job.status) ? "Recorded media, manifest, ordering, and content passed" : "Incomplete checks cannot pass"}</small></span></div>
          <ul class="detail-list section"><li><span>Digest</span><strong>SHA-256</strong></li><li><span>Scope</span><strong>${escapeHtml(job.mode)}</strong></li><li><span>Restore test</span><strong>${job.kind === "Restore test" ? "Passed in this attempt" : "Separate evidence"}</strong></li></ul></div>
        </section>
      </div>
      <section class="panel section"><div class="panel-head"><div><h2>Durable event history</h2><p>Correlation ${escapeHtml(job.attempt)}</p></div><button class="button button-small" type="button" data-action="export-diagnostics">Export sanitized diagnostics</button></div>
        <div class="panel-body"><ol class="timeline">${job.events.map((event) => `<li class="timeline-item"><time class="timeline-time">${escapeHtml(event[0])}</time><span class="timeline-marker"></span><div class="timeline-copy"><strong>${escapeHtml(event[1])}</strong><span>${escapeHtml(event[2])}</span></div></li>`).join("")}</ol></div>
      </section>
      <div class="page-actions section"><button class="button" type="button" data-action="back-jobs">Back to history</button>${isAttention ? `<button class="button button-danger" type="button" data-action="cancel-job" data-id="${escapeHtml(job.id)}">Cancel safely</button>` : ""}</div>`;
    setMain(html, `Job ${job.id}`);
  }

  function renderTapes() {
    const tapes = state.scenario === "empty" ? [] : state.data.tapes;
    const query = state.search.toLowerCase();
    const filtered = tapes.filter((tape) => {
      const matchesQuery = !query || `${tape.id} ${tape.barcode || ""} ${tape.pool} ${tape.location} ${tape.contents}`.toLowerCase().includes(query);
      const matchesFilter = state.tapeFilter === "all" || String(tape.tone).includes(state.tapeFilter) || `${tape.location} ${tape.retention} ${tape.health}`.toLowerCase().includes(state.tapeFilter);
      return matchesQuery && matchesFilter;
    });
    const actions = `<button class="button" type="button" data-action="refresh">&#8635; Inventory</button><button class="button button-primary" type="button" data-action="identify-media">Identify media</button>`;
    let html = pageHead("Tapes & pools", "Physical identity, content, custody, retention, and verification stay separate.", actions);
    html += scenarioNotice();
    if (!tapes.length) {
      html += `<div class="section">${emptyState("No tape inventory", "Run device discovery, then identify known cartridges without overwriting unknown media.", "Discover devices", "discover-devices")}</div>`;
      setMain(html, "Tapes and pools");
      return;
    }
    html += `<section class="panel table-panel section" aria-label="Tape inventory">
      <div class="table-toolbar"><div class="toolbar-start"><label class="search-field"><span class="sr-only">Search tapes</span><input type="search" value="${escapeHtml(state.search)}" data-input="tape-search" placeholder="Search tape, barcode, pool, or location"></label></div>
      <div class="toolbar-end"><label><span class="sr-only">Filter tape inventory</span><select class="select compact" data-input="tape-filter"><option value="all">All media</option><option value="offsite">Off site</option><option value="warning">Needs action</option><option value="blocked">Protected / unknown</option><option value="success">Verified / healthy</option></select></label><span>${filtered.length} cartridges</span></div></div>
      <div class="table-scroll"><table class="data-table responsive-table"><thead><tr><th>Identity</th><th>Generation</th><th>Pool & contents</th><th>Confirmed location</th><th>Retention</th><th>Verification</th><th>Eligibility</th></tr></thead><tbody>
        ${filtered.map((tape) => `<tr class="row-action" data-tape-id="${escapeHtml(tape.id)}" tabindex="0">
          <td class="primary-cell" data-label="Identity"><strong>${tapeId(tape.id)}</strong><small>${escapeHtml(tape.barcode || "No barcode · manual identity")}</small></td>
          <td data-label="Generation"><strong>${escapeHtml(tape.generation)}</strong><br><small>${escapeHtml(tape.format)}</small></td>
          <td data-label="Pool & contents"><strong>${escapeHtml(tape.pool)}</strong><br><small>${escapeHtml(tape.contents)}</small></td>
          <td data-label="Confirmed location"><strong>${escapeHtml(tape.location)}</strong><br><small>${escapeHtml(tape.movement)}</small></td>
          <td data-label="Retention">${escapeHtml(tape.retention)}</td>
          <td data-label="Verification"><span class="status ${escapeHtml(tape.tone)}">${escapeHtml(tape.verification)}</span></td>
          <td data-label="Eligibility"><strong>${escapeHtml(tape.eligibility)}</strong><br><small>${escapeHtml(tape.health)}</small></td>
        </tr>`).join("")}
      </tbody></table></div>
      <div class="table-foot"><span>Showing ${filtered.length} of ${tapes.length} cartridges</span><span>Unknown media remains protected</span></div>
    </section>`;
    setMain(html, "Tapes and pools");
    const filter = $("[data-input='tape-filter']");
    if (filter) filter.value = state.tapeFilter;
  }

  function openTapeDrawer(id) {
    const tape = state.data.tapes.find((item) => item.id === id);
    if (!tape) return;
    const overwriteSafe = /write eligible|append eligible/i.test(tape.eligibility) && !/protected|hold|archive/i.test(tape.retention);
    openDrawer(`
      <div class="drawer-head"><div><span class="eyebrow">Cartridge record</span><h2>${escapeHtml(tape.id)}</h2></div><button class="icon-button" type="button" data-action="close-drawer" aria-label="Close tape details">&times;</button></div>
      <div class="drawer-body">
        <div class="detail-hero"><div class="detail-hero-row">${tapeId(tape.id, true)}${statusTag(tape.health, tape.tone)}</div><p>${escapeHtml(tape.pool)} · ${escapeHtml(tape.contents)}</p>${tape.used == null ? "" : `<div class="section">${progressBar(tape.used, "Estimated native capacity used")}</div>`}</div>
        ${!/blocked|interrupted/.test(tape.tone) ? "" : `<div class="notice danger"><span class="notice-icon">!</span><div class="notice-copy"><strong>Protected from overwrite</strong><span>Identity or position is uncertain. Reconciliation is required before any destructive operation.</span></div></div>`}
        <section class="detail-section"><h3>Identity & format</h3><ul class="detail-list"><li><span>Internal ID</span><strong class="mono">${escapeHtml(tape.id)}</strong></li><li><span>Barcode</span><strong class="mono">${escapeHtml(tape.barcode || "Not available")}</strong></li><li><span>Media</span><strong>${escapeHtml(tape.generation)}</strong></li><li><span>On-tape format</span><strong>${escapeHtml(tape.format)}</strong></li><li><span>Health</span><strong>${escapeHtml(tape.health)}</strong></li></ul></section>
        <section class="detail-section"><h3>Protection decision</h3><ul class="detail-list"><li><span>Retention</span><strong>${escapeHtml(tape.retention)}</strong></li><li><span>Read/write</span><strong>${escapeHtml(tape.eligibility)}</strong></li><li><span>May overwrite?</span><strong>${overwriteSafe ? "Eligible after physical identity check" : "No — protected state or dependency"}</strong></li><li><span>Contents</span><strong>${escapeHtml(tape.contents)}</strong></li></ul></section>
        <section class="detail-section"><h3>Verification evidence</h3><div class="evidence-stamp"><span class="evidence-seal">${/pending|never|incomplete/i.test(tape.verification) ? "…" : "&#10003;"}</span><span><strong>${escapeHtml(tape.verification)}</strong><small>Scope and drive retained in the evidence record</small></span></div></section>
        <section class="detail-section"><h3>Custody trail</h3><ol class="timeline"><li class="timeline-item"><time class="timeline-time">Current</time><span class="timeline-marker"></span><div class="timeline-copy"><strong>${escapeHtml(tape.location)}</strong><span>Confirmed inventory state · ${escapeHtml(tape.movement)}</span></div></li><li class="timeline-item"><time class="timeline-time">08 Nov</time><span class="timeline-marker"></span><div class="timeline-copy"><strong>Library A intake</strong><span>Identity reconciled by Brandon Ragsdale</span></div></li></ol></section>
      </div>
      <div class="drawer-foot"><button class="button" type="button" data-action="close-drawer">Close</button><button class="button button-primary" type="button" data-action="create-movement" data-id="${escapeHtml(tape.id)}">Create movement</button></div>`);
  }

  function selectedRecoveryPoint() {
    return state.data.recoveryPoints.find((item) => item.id === state.restore.recoveryPoint) || state.data.recoveryPoints[0];
  }

  function renderRestore() {
    const rp = selectedRecoveryPoint();
    const actions = `<button class="button" type="button" data-action="restore-reset">Reset plan</button>`;
    let html = pageHead("Restore planner", "Find a recovery point, resolve every physical prerequisite, and restore into an isolated destination.", actions);
    if (state.restore.queued) {
      html += `<div class="panel empty-state section"><div class="empty-state-inner"><div class="empty-symbol" aria-hidden="true">&#10003;</div><h2>Restore plan prepared</h2><p>The preview API evaluated the declared media, key, drive, and destination prerequisites. No restore job or physical operation was started.</p><div class="page-actions"><button class="button" data-action="restore-reset">Plan another restore</button><a class="button button-primary" href="#settings">Review qualification gaps</a></div></div></div>`;
      setMain(html, "Restore plan prepared");
      return;
    }
    const stepLabels = [
      ["Find", "Recovery point"], ["Scope", "Files or dataset"], ["Prerequisites", "Media and keys"], ["Destination", "Isolated target"], ["Review", "Queue safely"]
    ];
    html += `<div class="restore-shell section">
      <nav class="panel wizard-nav" aria-label="Restore planning steps">${stepLabels.map((item, index) => {
        const step = index + 1;
        return `<button class="wizard-step ${step === state.restore.step ? "active" : ""} ${step < state.restore.step ? "done" : ""}" type="button" data-action="restore-step" data-step="${step}" ${step > state.restore.step ? "disabled" : ""}><span class="step-number">${step < state.restore.step ? "&#10003;" : step}</span><span><strong>${item[0]}</strong><small>${item[1]}</small></span></button>`;
      }).join("")}</nav>
      <section class="panel wizard-main" aria-live="polite">${renderRestoreStep(rp)}</section>
      ${renderRestoreSummary(rp)}
    </div>`;
    setMain(html, "Restore planner");
  }

  function renderRestoreStep(rp) {
    const step = state.restore.step;
    let content = "";
    if (step === 1) {
      content = `<div class="wizard-content"><h2>Choose a recovery point</h2><p>Searchable catalog results include representation, evidence, and physical availability.</p>
        <label class="search-field"><span class="sr-only">Search recovery catalog</span><input type="search" placeholder="Search source, path, date, or job" data-input="recovery-search"></label>
        <div class="section">${state.data.recoveryPoints.map((point) => `<label class="recovery-choice ${point.id === state.restore.recoveryPoint ? "selected" : ""}">
          <input type="radio" name="recovery-point" value="${escapeHtml(point.id)}" data-input="recovery-point" ${point.id === state.restore.recoveryPoint ? "checked" : ""}>
          <span><strong>${escapeHtml(point.label)}</strong><small>${escapeHtml(point.type)} · ${escapeHtml(point.verified)}</small></span><time>${escapeHtml(point.size)}</time>
        </label>`).join("")}</div></div>`;
    } else if (step === 2) {
      const nativeMode = /native/i.test(rp.type);
      content = `<div class="wizard-content"><h2>Choose restore scope</h2><p>${nativeMode ? "Native streams reconstruct a dataset or zvol. Guest files inside zvols are not browsable." : "Select a complete tree or individual files from the recorded snapshot namespace."}</p>
        <div class="filter-row"><label class="radio-option"><input type="radio" name="restore-mode" value="selected" data-input="restore-mode" ${state.restore.mode === "selected" ? "checked" : ""}> Selected files and directories</label><label class="radio-option"><input type="radio" name="restore-mode" value="all" data-input="restore-mode" ${state.restore.mode === "all" ? "checked" : ""}> Complete included tree</label></div>
        ${nativeMode ? `<div class="notice info section"><span class="notice-icon">i</span><div class="notice-copy"><strong>${/zvol/i.test(rp.source) ? "Block-volume recovery" : "Native dataset recovery"}</strong><span>The full dependency chain will be received into an isolated dataset. File extraction requires approved temporary ZFS staging.</span></div></div>` : `<div class="file-browser section"><div class="file-path">/${escapeHtml(rp.source)}/</div>
          ${[
            ["/Brandon", "Directory", "3.8 TiB", "07 Nov 18:42"],
            ["/Engineering", "Directory", "2.1 TiB", "11 Nov 16:14"],
            ["/Engineering/roadmap-2027.xlsx", "File", "2.8 MiB", "11 Nov 16:14"],
            ["/Finance", "Directory", "874 GiB", "10 Nov 09:31"],
            ["/Shared/production-delivery-notes-final-v18.pdf", "File", "48 MiB", "11 Nov 20:03"]
          ].map((file) => `<label class="file-row"><input type="checkbox" value="${escapeHtml(file[0])}" data-input="restore-file" ${state.restore.selectedFiles.includes(file[0]) ? "checked" : ""}><strong>${escapeHtml(file[0])}</strong><span>${escapeHtml(file[2])}</span><span>${escapeHtml(file[3])} UTC</span></label>`).join("")}</div>`}
      </div>`;
    } else if (step === 3) {
      const blocked = !rp.complete;
      content = `<div class="wizard-content"><h2>Resolve prerequisites</h2><p>Every required tape, key, dependency, compatible drive, and staging requirement remains explicit.</p>
        ${blocked ? `<div class="notice"><span class="notice-icon">!</span><div class="notice-copy"><strong>One prerequisite is not on site</strong><span>This restore may be queued in a waiting state, but reading cannot begin until custody is confirmed.</span></div>${state.restore.recallCreated ? statusTag("Recall created", "warning") : `<button class="button button-small" data-action="create-recall">Create recall</button>`}</div>` : ""}
        <ul class="readiness-list section">
          ${rp.tapes.map((id, index) => `<li class="readiness-item ${blocked && index === 0 ? "blocked" : ""}"><span class="gate-icon">${blocked && index === 0 ? "!" : "&#10003;"}</span><div><strong>${tapeId(id)}</strong><span>${blocked && index === 0 ? `Confirmed at ${rp.location} · physical recall required` : `Available at ${rp.location}`}</span></div>${statusTag(blocked && index === 0 ? "Missing on site" : "Ready", blocked && index === 0 ? "warning" : "success")}</li>`).join("")}
          <li class="readiness-item"><span class="gate-icon">&#10003;</span><div><strong>Compatible drive</strong><span>${/L8/.test(rp.tapes[0]) ? "Drive 2 · IBM ULTRIUM-HH8" : "Drive 1 · IBM ULTRIUM-HH9"}</span></div>${statusTag("Ready", "success")}</li>
          <li class="readiness-item"><span class="gate-icon">&#10003;</span><div><strong>Recovery key lineage RK-2026-02</strong><span>Independent recovery package tested 03 Nov 2026</span></div>${statusTag("Available", "success")}</li>
          <li class="readiness-item"><span class="gate-icon">&#10003;</span><div><strong>Dependency chain</strong><span>${/incremental/i.test(rp.type) ? "Compatible full plus 3 increments resolved" : "Self-contained full recovery point"}</span></div>${statusTag("Complete", "success")}</li>
          <li class="readiness-item"><span class="gate-icon">&#10003;</span><div><strong>Estimated reading and staging</strong><span>${escapeHtml(rp.size)} tape read · ${/native/i.test(rp.type) ? "1.4 TiB conservative staging reserve" : "no native staging required"}</span></div>${statusTag("Estimated", "info")}</li>
        </ul>
      </div>`;
    } else if (step === 4) {
      content = `<div class="wizard-content"><h2>Choose an isolated destination</h2><p>A new destination is the default. Tape paths and native properties are treated as untrusted inputs.</p>
        <div class="form-grid">
          <div class="form-field full"><label for="restore-destination">Approved ZFS destination</label><input class="input mono" id="restore-destination" data-input="restore-destination" value="${escapeHtml(state.restore.destination)}"><small>Must remain below an administrator-approved restore root.</small></div>
          <div class="form-field"><label for="restore-conflicts">Existing-file behavior</label><select class="select" id="restore-conflicts" data-input="restore-conflicts"><option value="skip">Skip conflicts (default)</option><option value="rename">Rename restored files</option><option value="replace">Replace after administrator approval</option></select></div>
          <div class="form-field"><span class="field-label">Publication</span><label class="check-option"><input type="checkbox" disabled> Enable recovered shares automatically</label><small>Always disabled. Validate before optional publication.</small></div>
        </div>
        <div class="notice info section"><span class="notice-icon">i</span><div class="notice-copy"><strong>Containment and metadata preflight</strong><span>Absolute paths, traversal sequences, unsafe links, unsupported ACL semantics, and identity-map gaps will block extraction rather than silently weaken permissions.</span></div></div>
      </div>`;
    } else {
      const blocked = !rp.complete;
      content = `<div class="wizard-content"><h2>Review the restore plan</h2><p>Nothing is mounted over live data, no shares are enabled, and no active tape write is interrupted.</p>
        ${blocked ? `<div class="notice"><span class="notice-icon">!</span><div class="notice-copy"><strong>Plan will queue as Waiting for media</strong><span>The recall remains visible and actionable. Snapshot dependencies stay protected.</span></div></div>` : `<div class="notice success"><span class="notice-icon">&#10003;</span><div class="notice-copy"><strong>Plan is ready to queue</strong><span>All current prerequisites passed. They will be checked again before the first tape operation.</span></div></div>`}
        <ul class="detail-list section"><li><span>Recovery point</span><strong>${escapeHtml(rp.label)}</strong></li><li><span>Scope</span><strong>${state.restore.mode === "all" ? "Complete included tree" : `${state.restore.selectedFiles.length} selected paths`}</strong></li><li><span>Required media</span><strong>${rp.tapes.map((id) => escapeHtml(id)).join(", ")}</strong></li><li><span>Destination</span><strong class="mono">${escapeHtml(state.restore.destination)}</strong></li><li><span>Conflicts</span><strong>${titleCase(state.restore.conflicts)}</strong></li><li><span>Verification</span><strong>Compare content digests and promised metadata</strong></li></ul>
      </div>`;
    }
    return `${content}<div class="wizard-footer"><button class="button" type="button" data-action="restore-back" ${step === 1 ? "disabled" : ""}>&#8592; Back</button>${step < 5 ? `<button class="button button-primary" type="button" data-action="restore-next">Continue &#8594;</button>` : `<button class="button button-primary" type="button" data-action="queue-restore">Queue restore plan</button>`}</div>`;
  }

  function renderRestoreSummary(rp) {
    return `<aside class="panel plan-summary" aria-label="Restore plan summary"><div class="panel-head"><div><h2>Plan summary</h2><p>Updates as choices change</p></div></div>
      <div class="summary-block"><h3>Recovery point</h3><dl><dt>Source</dt><dd>${escapeHtml(rp.source)}</dd><dt>Mode</dt><dd>${escapeHtml(rp.type)}</dd><dt>Evidence</dt><dd>${escapeHtml(rp.verified)}</dd></dl></div>
      <div class="summary-block"><h3>Physical work</h3><dl><dt>Tapes</dt><dd>${rp.tapes.length}</dd><dt>Location</dt><dd>${escapeHtml(rp.location)}</dd><dt>Read</dt><dd>${escapeHtml(rp.size)} estimated</dd></dl></div>
      <div class="summary-block"><h3>Destination</h3><dl><dt>Path</dt><dd class="mono">${escapeHtml(state.restore.destination)}</dd><dt>Conflicts</dt><dd>${titleCase(state.restore.conflicts)}</dd><dt>Shares</dt><dd>Remain disabled</dd></dl></div>
    </aside>`;
  }

  function renderMovements() {
    const movements = state.scenario === "empty" ? [] : state.data.movements;
    const selected = movements.find((item) => item.id === state.selectedMovement) || movements[0];
    const actions = `<button class="button" type="button" data-action="print-manifest">Print manifest</button><button class="button button-primary" type="button" data-action="new-movement">+ Plan movement</button>`;
    let html = pageHead("Off-site movements", "Planned logistics and confirmed physical custody are intentionally separate records.", actions);
    html += scenarioNotice();
    if (!movements.length) {
      html += `<div class="section">${emptyState("No movement tasks", "Create a rotation plan to generate take-off-site, return, swap, and recall checklists.", "Configure rotation", "new-movement")}</div>`;
      setMain(html, "Off-site movements");
      return;
    }
    html += `<div class="movement-layout section">
      <aside class="panel task-list" aria-label="Movement task queue"><div class="panel-head"><div><h2>Task queue</h2><p>${movements.filter((item) => !/success/.test(item.status)).length} open · 1 overdue</p></div></div>
        ${movements.map((move) => `<button class="movement-task ${move.id === selected.id ? "active" : ""}" type="button" data-movement-id="${escapeHtml(move.id)}"><span class="movement-task-head"><strong>${escapeHtml(move.title)}</strong>${statusTag(move.status)}</span><small>${escapeHtml(move.due)} · ${move.tapes.length} tape${move.tapes.length === 1 ? "" : "s"}</small><span class="task-route">${escapeHtml(move.source)} &#8594; ${escapeHtml(move.destination)}</span></button>`).join("")}
      </aside>
      ${renderMovementDetail(selected)}
    </div>`;
    setMain(html, "Off-site movements");
  }

  function renderMovementDetail(move) {
    const checked = move.tapes.filter((item) => item.checked).length;
    const allChecked = checked === move.tapes.length;
    const stageNames = ["Planned", "Ready", "Departed", "Arrived"];
    const complete = move.stage >= 4;
    return `<section class="panel movement-detail" aria-labelledby="movement-title">
      <div class="movement-hero"><div class="movement-task-head"><div><span class="eyebrow">${escapeHtml(move.kind)} · ${escapeHtml(move.id)}</span><h2 id="movement-title">${escapeHtml(move.title)}</h2></div>${statusTag(move.status)}</div>
        <div class="movement-route"><div class="location-node"><span>Confirmed source</span><strong>${escapeHtml(move.source)}</strong></div><span class="movement-arrow" aria-hidden="true">&#8594;</span><div class="location-node"><span>Planned destination</span><strong>${escapeHtml(move.destination)}</strong></div></div>
      </div>
      <div class="custody-stage" aria-label="Custody progress">${stageNames.map((name, index) => `<div class="custody-step ${index + 1 < move.stage ? "done" : index + 1 === move.stage ? "current" : ""}"><div class="custody-dot"></div><span>${name}</span></div>`).join("")}</div>
      <div class="panel-body"><div class="notice ${complete ? "success" : move.status === "blocked" ? "" : "info"}"><span class="notice-icon">${complete ? "&#10003;" : move.status === "blocked" ? "!" : "i"}</span><div class="notice-copy"><strong>${escapeHtml(move.readiness)}</strong><span>${escapeHtml(move.reason)}. Due ${escapeHtml(move.due)}.</span></div></div></div>
      <div class="panel-head"><div><h2>Packing & confirmation checklist</h2><p>${checked} of ${move.tapes.length} physically selected · assigned to ${escapeHtml(move.assigned)}</p></div><button class="button button-small" type="button" data-action="print-manifest">Print</button></div>
      <div class="packing-list">${move.tapes.map((item) => `<label class="packing-row"><input class="packing-check" type="checkbox" data-input="packing-tape" data-move="${escapeHtml(move.id)}" value="${escapeHtml(item.id)}" ${item.checked ? "checked" : ""} ${complete ? "disabled" : ""}><span>${tapeId(item.id)}</span><small>${escapeHtml(item.reason)}</small>${item.ready ? statusTag("Ready", "success") : statusTag("Not export-ready", "warning")}</label>`).join("")}</div>
      <div class="panel-head"><div><h2>Custody evidence</h2><p>Corrections append history; they never rewrite prior events</p></div></div>
      <div class="panel-body"><ol class="timeline">${move.history.map((event) => `<li class="timeline-item"><time class="timeline-time">${escapeHtml(event[1])}</time><span class="timeline-marker"></span><div class="timeline-copy"><strong>${escapeHtml(event[0])}</strong><span>${escapeHtml(event[2])}</span></div></li>`).join("")}</ol></div>
      <div class="drawer-foot">${complete ? `<button class="button" type="button" data-action="correct-custody">Record correction</button>` : move.stage >= 2 ? `<button class="button button-primary" type="button" data-action="confirm-arrival" data-id="${escapeHtml(move.id)}" ${allChecked ? "" : "disabled"}>Confirm arrival at destination</button>` : `<button class="button" type="button" data-action="confirm-departure" data-id="${escapeHtml(move.id)}" ${allChecked && move.tapes.every((item) => item.ready) ? "" : "disabled"}>Confirm departure</button>`}</div>
    </section>`;
  }

  function renderDrives() {
    const drives = state.scenario === "empty" ? [] : state.data.drives;
    const actions = `<button class="button" type="button" data-action="refresh">&#8635; Reconcile inventory</button><button class="button button-primary" type="button" data-action="discover-devices">Discover devices</button>`;
    let html = pageHead("Drives & libraries", "Stable physical identities, accurate changer inventory, and one controlling operation per drive.", actions);
    html += scenarioNotice();
    if (!drives.length) {
      html += `<div class="section">${emptyState("No tape devices discovered", "Map only the required tape and changer devices, then run capability discovery.", "Discover devices", "discover-devices")}</div>`;
      setMain(html, "Drives and libraries");
      return;
    }
    html += `<div class="notice section"><span class="notice-icon">!</span><div class="notice-copy"><strong>Standalone recovery drive needs reconciliation</strong><span>Stable serial HU1918M4D2 was found at a new device path. Tape operations remain blocked until an administrator confirms the binding.</span></div><button class="button button-small" data-action="reconcile-drive">Reconcile</button></div>
      <div class="library-layout section"><section class="panel"><div class="panel-head"><div><h2>IBM TS3100 · Library A</h2><p>24 storage slots · 2 drives · inventory observed 11:41 UTC</p></div>${statusTag("Healthy", "success")}</div>
        <div class="library-map"><div class="slot-legend"><span class="legend-key"><span class="legend-swatch"></span> Empty</span><span class="legend-key"><span class="legend-swatch loaded"></span> Occupied</span><span class="legend-key"><span class="legend-swatch reserved"></span> Reserved</span><span class="legend-key"><span class="legend-swatch alert"></span> Attention</span></div>
        <div class="slot-grid">${state.data.slots.map((slot) => `<button class="slot ${escapeHtml(slot[2])}" type="button" data-slot="${slot[0]}" aria-label="Slot ${slot[0]}${slot[1] ? `, ${slot[1]}` : ", empty"}"><span>Slot ${String(slot[0]).padStart(2, "0")}</span><strong>${escapeHtml(slot[1] || "Empty")}</strong></button>`).join("")}</div></div>
      </section>
      <section class="panel"><div class="panel-head"><div><h2>Drive activity</h2><p>Stable serial and WWN bindings</p></div></div>${drives.map((drive) => `<article class="drive-card"><span class="drive-glyph" aria-hidden="true">&#9673;</span><div><strong>${escapeHtml(drive.name)}</strong><span>${escapeHtml(drive.model)} · ${escapeHtml(drive.generation)}</span><small>${escapeHtml(drive.loaded || drive.serial)}</small></div>${statusTag(drive.status)}</article>`).join("")}</section></div>
      <section class="panel section table-panel"><div class="panel-head"><div><h2>Stable device bindings</h2><p>Path changes trigger reconciliation rather than implicit selection</p></div></div><div class="table-scroll"><table class="data-table responsive-table"><thead><tr><th>Drive</th><th>Serial / WWN</th><th>Firmware</th><th>Device path</th><th>Health</th></tr></thead><tbody>${drives.map((drive) => `<tr><td class="primary-cell" data-label="Drive"><strong>${escapeHtml(drive.name)}</strong><small>${escapeHtml(drive.model)}</small></td><td data-label="Serial / WWN"><span class="mono">${escapeHtml(drive.serial)}</span><br><small class="mono">${escapeHtml(drive.wwn)}</small></td><td data-label="Firmware"><span class="mono">${escapeHtml(drive.firmware)}</span></td><td data-label="Device path"><span class="mono">${escapeHtml(drive.path)}</span></td><td data-label="Health">${statusTag(drive.health)}</td></tr>`).join("")}</tbody></table></div></section>`;
    setMain(html, "Drives and libraries");
  }

  function renderVerification() {
    const records = state.scenario === "empty" ? [] : state.data.verification;
    const actions = `<button class="button" type="button" data-action="export-evidence">Export evidence</button><button class="button button-primary" type="button" data-action="schedule-verification">Schedule verification</button>`;
    let html = pageHead("Verification & recovery", "Write completion, media read-back, sample verification, and restore tests are distinct evidence.", actions);
    html += `<div class="notice info"><span class="notice-icon">i</span><div class="notice-copy"><strong>Representative evidence fixture</strong><span>The verification API and physical recovery path are not implemented or qualified in this preview. Records below demonstrate the required information design only.</span></div></div>`;
    if (!records.length) {
      html += `<div class="section">${emptyState("No verification evidence", "New policies default to full recovery-path read-back before off-site eligibility.", "Configure verification", "schedule-verification")}</div>`;
      setMain(html, "Verification and recovery");
      return;
    }
    html += `<section class="metric-strip section" aria-label="Verification posture"><div class="metric"><span>New-data verification</span><strong>Full</strong><small>Read-back through decode path</small></div><div class="metric"><span>Evidence fresh</span><strong>17/18</strong><small>One interrupted attempt</small></div><div class="metric"><span>Next restore drill</span><strong>10 Dec</strong><small>Representative file restore</small></div><div class="metric"><span>Clean-host exercise</span><strong>03 Feb</strong><small>Quarterly catalog-loss recovery</small></div></section>
      <section class="panel table-panel section"><div class="panel-head"><div><h2>Evidence history</h2><p>Scope and outcome remain explicit</p></div></div><div class="table-scroll"><table class="data-table responsive-table"><thead><tr><th>Protected work</th><th>Evidence type</th><th>Media / drive</th><th>Started</th><th>Result</th><th>Evidence ID</th></tr></thead><tbody>${records.map((record) => `<tr class="row-action" data-verification-id="${escapeHtml(record.id)}" tabindex="0"><td class="primary-cell" data-label="Protected work"><strong>${escapeHtml(record.target)}</strong><small>${escapeHtml(record.detail)}</small></td><td data-label="Evidence type">${escapeHtml(record.scope)}</td><td data-label="Media / drive">${tapeId(record.media)}<br><small>${escapeHtml(record.drive)}</small></td><td data-label="Started"><span class="mono">${escapeHtml(record.started)}</span></td><td data-label="Result">${statusTag(record.result)}</td><td data-label="Evidence ID"><span class="mono">${escapeHtml(record.evidence)}</span></td></tr>`).join("")}</tbody></table></div></section>
      <div class="grid-2 section"><section class="panel"><div class="panel-head"><div><h2>Independent recovery package</h2><p>Separate from the application catalog</p></div>${statusTag("Fixture", "warning")}</div><div class="panel-body"><div class="evidence-stamp"><span class="evidence-seal">i</span><span><strong>Representative package record</strong><small>Clean reader · key package · volume map · format documentation</small></span></div><ul class="detail-list section"><li><span>Current implementation</span><strong>Manifest inspection CLI</strong></li><li><span>Key recovery</span><strong>Not physically tested</strong></li><li><span>Bootstrap</span><strong>Runbook implemented · G2 not run</strong></li></ul></div></section>
      <section class="panel"><div class="panel-head"><div><h2>Recovery exercise cadence</h2><p>Shipping policy targets; no exercise evidence yet</p></div></div><div class="panel-body"><ul class="readiness-list"><li class="readiness-item"><span class="gate-icon">!</span><div><strong>Monthly file restore</strong><span>Policy target · first run pending</span></div>${statusTag("Not run", "warning")}</li><li class="readiness-item"><span class="gate-icon">!</span><div><strong>Quarterly native chain</strong><span>Policy target · first run pending</span></div>${statusTag("Not run", "warning")}</li><li class="readiness-item"><span class="gate-icon">!</span><div><strong>Clean-install recovery</strong><span>Policy target · G2 evidence pending</span></div>${statusTag("Not run", "warning")}</li></ul></div></section></div>`;
    setMain(html, "Verification and recovery");
  }

  function renderSettings() {
    const actions = `<button class="button" type="button" data-action="export-diagnostics">Export sanitized diagnostics</button><button class="button button-primary" type="button" data-action="refresh">Run preflight</button>`;
    let html = pageHead("System & qualification", "Deployment health, safety boundaries, dependencies, and evidence-backed compatibility claims.", actions);
    html += scenarioNotice();
    html += `<div class="settings-layout section"><nav class="panel settings-nav" aria-label="System settings">
      ${[["preflight", "Preflight & health"], ["qualification", "LTO qualification"], ["security", "Security & access"], ["dependencies", "Runtime inventory"], ["interface", "Console data"]].map((item) => `<button type="button" data-settings-tab="${item[0]}" class="${state.settingsTab === item[0] ? "active" : ""}">${item[1]}</button>`).join("")}
    </nav><section class="settings-content">${renderSettingsContent()}</section></div>`;
    setMain(html, "System and qualification");
  }

  function renderSettingsContent() {
    if (state.settingsTab === "qualification") return renderQualificationSettings();
    if (state.settingsTab === "security") return renderSecuritySettings();
    if (state.settingsTab === "dependencies") return renderDependencySettings();
    if (state.settingsTab === "interface") return renderInterfaceSettings();
    const checks = state.data.preflight;
    const passCount = checks.filter((item) => /pass|success|ready/i.test(item.status)).length;
    return `<div class="panel"><div class="panel-head"><div><h2>Deployment preflight</h2><p>${passCount} of ${checks.length} checks passed · last run 12 Nov 2026 11:41 UTC</p></div>${statusTag(passCount === checks.length ? "Ready" : `${checks.length - passCount} action required`, passCount === checks.length ? "success" : "warning")}</div>
      <div class="panel-body"><div class="preflight-grid">${checks.map((check) => `<article class="preflight-item"><span class="gate-icon" style="background:${/pass|success|ready/i.test(check.status) ? "var(--teal)" : /warn|simulat|not.?run/i.test(check.status) ? "var(--amber)" : "var(--red)"}">${/pass|success|ready/i.test(check.status) ? "&#10003;" : "!"}</span><div><strong>${escapeHtml(check.name)}</strong><span>${escapeHtml(check.detail)}</span></div></article>`).join("")}</div></div>
    </div>
    <div class="grid-2 section"><section class="panel"><div class="panel-head"><div><h2>TrueNAS integration boundary</h2><p>Authenticated versioned JSON-RPC target</p></div>${statusTag("Not configured", "warning")}</div><div class="panel-body"><ul class="detail-list"><li><span>Endpoint</span><strong class="mono">Not configured in preview</strong></li><li><span>API negotiation</span><strong>Secure version-selection rules implemented</strong></li><li><span>Certificate</span><strong>Verification mandatory · evidence not run</strong></li><li><span>Platform</span><strong>TrueNAS 25.10.8 qualification target</strong></li><li><span>Data path</span><strong>Snapshot/ZFS probe not run</strong></li></ul></div></section>
      <section class="panel"><div class="panel-head"><div><h2>Persistent state</h2><p>Fixture values unless the API reports a connected database</p></div>${statusTag("Simulation", "warning")}</div><div class="panel-body"><ul class="detail-list"><li><span>PostgreSQL target</span><strong>16.15 · schema migration 0001</strong></li><li><span>Catalog backup</span><strong>Illustrative fixture · not recovery evidence</strong></li><li><span>Recovery export</span><strong>Illustrative fixture · not generated</strong></li><li><span>Audit retention</span><strong>365-day policy target</strong></li><li><span>Spool reserve</span><strong>Simulator value · verify at deployment</strong></li></ul></div></section></div>`;
  }

  function renderQualificationSettings() {
    const qual = state.data.qualification;
    const generations = qual.generations || [];
    return `<div class="notice"><span class="notice-icon">!</span><div class="notice-copy"><strong>Capability support is not physical qualification</strong><span>The model covers LTO-1 through LTO-10. Only named hardware tests may become qualification evidence; untested generations remain explicit gaps.</span></div></div>
      <section class="panel table-panel section"><div class="panel-head"><div><h2>LTO generation coverage</h2><p>Read/write eligibility is evaluated separately</p></div><span class="mono">${generations.length}/10 modeled</span></div><div class="table-scroll"><table class="data-table responsive-table"><thead><tr><th>Drive generation</th><th>Reads</th><th>Writes</th><th>Model evidence</th><th>Physical qualification</th></tr></thead><tbody>${generations.map((row) => `<tr><td class="primary-cell" data-label="Drive generation"><strong>${escapeHtml(row.generation)}</strong><small>${row.generation === "LTO-8" ? "Includes LTO-7 Type M" : row.generation === "LTO-10" ? "No backward-compatibility claim" : "Generation capability profile"}</small></td><td data-label="Reads">${escapeHtml(row.reads)}</td><td data-label="Writes">${escapeHtml(row.writes)}</td><td data-label="Model evidence">${statusTag(row.model, /simulated/i.test(row.model) ? "info" : "warning")}</td><td data-label="Physical qualification">${escapeHtml(row.physical)}</td></tr>`).join("")}</tbody></table></div></section>
      <section class="panel section"><div class="panel-head"><div><h2>Qualification target</h2><p>Targets and versions are not qualification evidence by themselves</p></div></div><div class="panel-body"><ul class="detail-list"><li><span>TrueNAS</span><strong>${escapeHtml(qual.platform)}</strong></li><li><span>OpenZFS</span><strong>${escapeHtml(qual.openzfs)}</strong></li><li><span>Tape engine</span><strong>${escapeHtml(qual.engine)}</strong></li><li><span>Application image</span><strong class="mono">${escapeHtml(qual.image)}</strong></li><li><span>Gate status</span><strong>${escapeHtml(qual.gate || "G0 not passed")}</strong></li></ul></div></section>`;
  }

  function renderSecuritySettings() {
    return `<div class="grid-2"><section class="panel"><div class="panel-head"><div><h2>Administrator security target</h2><p>Protected sessions and explicit authority</p></div>${statusTag("Implementation pending", "warning")}</div><div class="panel-body"><ul class="detail-list"><li><span>Production accounts</span><strong>Not configured</strong></li><li><span>RBAC</span><strong>Domain policy tested · API enforcement pending</strong></li><li><span>MFA</span><strong>Required by design · not implemented</strong></li><li><span>Recovery codes</span><strong>Not generated</strong></li><li><span>Last reauthentication</span><strong>No production session</strong></li></ul><button class="button section" type="button" data-action="manage-mfa" disabled>Manage MFA</button></div></section>
      <section class="panel"><div class="panel-head"><div><h2>Encryption lineage</h2><p>Enabled by default for new policies</p></div>${statusTag("G0 selection pending", "warning")}</div><div class="panel-body"><ul class="detail-list"><li><span>Active lineage</span><strong>Not established</strong></li><li><span>Application format</span><strong>Authenticated format pending G0 selection</strong></li><li><span>Historical keys</span><strong>No production key lineage yet</strong></li><li><span>Independent package</span><strong>Workflow prototype only</strong></li><li><span>Cloud dependency</span><strong>None permitted for required recovery</strong></li></ul><button class="button section" type="button" data-action="key-export">Preview recovery export</button></div></section></div>
      <section class="panel section"><div class="panel-head"><div><h2>Roles & authority</h2><p>Required authority model; session and worker enforcement remain a release blocker</p></div></div><div class="table-scroll"><table class="data-table responsive-table"><thead><tr><th>Role</th><th>Default authority</th><th>Configured members</th><th>Destructive actions</th></tr></thead><tbody><tr><td class="primary-cell" data-label="Role"><strong>Administrator</strong><small>System authority</small></td><td data-label="Default authority">Policies, hardware, roles, keys, updates</td><td data-label="Configured members">None</td><td data-label="Destructive actions">Requires reauthentication + affected-object preview</td></tr><tr><td class="primary-cell" data-label="Role"><strong>Operator</strong><small>Day-to-day operations</small></td><td data-label="Default authority">Permitted jobs, media, assigned rotations, approved restores</td><td data-label="Configured members">None</td><td data-label="Destructive actions">Not permitted</td></tr><tr><td class="primary-cell" data-label="Role"><strong>Viewer</strong><small>Read-only evidence</small></td><td data-label="Default authority">Inventory, status, verification, coverage</td><td data-label="Configured members">None</td><td data-label="Destructive actions">Not permitted</td></tr></tbody></table></div></section>`;
  }

  function renderDependencySettings() {
    return `<section class="panel table-panel"><div class="panel-head"><div><h2>Runtime component inventory</h2><p>Custom backend components remain Rust; established Linux dependencies retain their implementations</p></div>${statusTag("Audit current", "success")}</div><div class="table-scroll"><table class="data-table responsive-table"><thead><tr><th>Component</th><th>Version / pin</th><th>Boundary</th><th>License</th><th>Recovery availability</th></tr></thead><tbody>
      ${[
        ["TapeScale API", "Rust 1.89.0", "Custom Rust HTTP/API service", "Apache-2.0", "Release image + source"],
        ["TapeScale scheduler", "Rust 1.89.0", "Custom Rust orchestration boundary", "Apache-2.0", "Release image + source"],
        ["TapeScale worker", "Rust 1.89.0", "Custom Rust worker process", "Apache-2.0", "Release image + source"],
        ["TapeScale recovery CLI", "Rust 1.89.0", "Custom Rust command-line tool", "Apache-2.0", "Release image; independent bundle pending"],
        ["Bacula Community", "15.x candidate · unselected", "Existing tape-engine candidate", "Upstream license applies", "Not packaged"],
        ["PostgreSQL", "16.15", "Existing durable database", "PostgreSQL License", "Pinned Compose image"],
        ["OpenZFS utilities", "Host-provided · unpinned", "Existing host tool", "Upstream license applies", "Not packaged"],
        ["mtx / sg3_utils", "Not packaged", "Existing device-tool candidates", "Upstream licenses apply", "Not yet available"],
        ["Static web console", "ES2022", "Browser-side JavaScript", "Apache-2.0", "Embedded in API image"]
      ].map((row) => `<tr><td class="primary-cell" data-label="Component"><strong>${escapeHtml(row[0])}</strong></td><td data-label="Version / pin"><span class="mono">${escapeHtml(row[1])}</span></td><td data-label="Boundary">${escapeHtml(row[2])}</td><td data-label="License">${escapeHtml(row[3])}</td><td data-label="Recovery availability">${escapeHtml(row[4])}</td></tr>`).join("")}
    </tbody></table></div></section>
    <div class="notice info section"><span class="notice-icon">i</span><div class="notice-copy"><strong>Safe external-tool execution</strong><span>Rust adapters invoke explicit arguments without shell interpolation, validate identifiers, enforce timeouts and cancellation, and propagate producer failures into durable job state.</span></div></div>`;
  }

  function renderInterfaceSettings() {
    return `<section class="panel"><div class="panel-head"><div><h2>Console data source</h2><p>Live same-origin API with a deterministic preview fallback</p></div>${statusTag(anyLive() ? "API connected" : "Demo fallback", anyLive() ? "success" : "warning")}</div><div class="panel-body">
      <div class="form-field"><label for="scenario-select">Representative state</label><select class="select" id="scenario-select" data-input="scenario"><option value="realistic">Realistic mixed operations</option><option value="empty">Empty first-run state</option><option value="failure">API / dependency failure</option></select><small>Preview state is stored only for this browser session. Live API data is never changed.</small></div>
      <ul class="detail-list section"><li><span>API base</span><strong class="mono">/api/v1</strong></li><li><span>Live endpoints</span><strong>${Object.values(state.live).filter(Boolean).length} of 7</strong></li><li><span>Fallback</span><strong>Embedded read-only operational fixtures</strong></li><li><span>Refresh policy</span><strong>Manual in prototype · durable events in production</strong></li></ul>
      <div class="page-actions section"><button class="button" type="button" data-action="preview-loading">Preview loading state</button><button class="button button-primary" type="button" data-action="refresh">Reconnect API</button></div>
    </div></section>
    <section class="panel section"><div class="panel-head"><div><h2>Visual system</h2><p>Purpose-built for physical offline recovery</p></div></div><div class="panel-body"><p>The paired recovery rail represents local and off-site recoverability. Exact tape IDs, paths, dates, units, custody evidence, and failure details use dense, accessible task layouts at desktop and phone sizes.</p><div class="filter-row section"><span class="status success">Verified</span><span class="status warning">Waiting for media</span><span class="status failed">Failed</span><span class="status offsite">Confirmed off site</span><span class="tape-id">TS-L9-004218</span></div></div></section>`;
  }

  function setMain(html, title) {
    const main = $("#main-content");
    if (!main) return;
    main.innerHTML = html;
    document.title = `${title} · TapeScale`;
  }

  function renderLoading() {
    setMain(`<div class="route-loading" role="status" aria-label="Loading TapeScale data"><div class="loading-header"></div><div class="loading-grid"><span></span><span></span><span></span></div><div class="loading-table"></div><span class="sr-only">Loading operational data</span></div>`, "Loading");
  }

  function renderRoute(options = {}) {
    if (state.loading) {
      renderLoading();
      return;
    }
    $$("[data-route]").forEach((link) => {
      const active = link.dataset.route === state.route;
      link.classList.toggle("active", active);
      if (active) link.setAttribute("aria-current", "page");
      else link.removeAttribute("aria-current");
    });
    document.body.classList.remove("nav-open");
    const renderers = {
      overview: renderOverview,
      jobs: renderJobs,
      tapes: renderTapes,
      restore: renderRestore,
      movements: renderMovements,
      drives: renderDrives,
      verification: renderVerification,
      settings: renderSettings
    };
    (renderers[state.route] || renderOverview)();
    if (options.focus !== false) {
      const main = $("#main-content");
      if (main) main.focus({ preventScroll: true });
      window.scrollTo({ top: 0, behavior: "instant" });
    }
    if (state.route === "jobs" && state.subroute && state.live.jobs) refreshJobDetail(state.subroute);
  }

  async function refreshJobDetail(id) {
    const result = await apiGet(`/jobs/${encodeURIComponent(id)}`);
    if (!result.ok || !result.data) return;
    const index = state.data.jobs.findIndex((job) => job.id === id);
    const payload = result.data;
    const raw = payload.job ? {
      ...payload.job,
      phase: Array.isArray(payload.phases) ? Math.max(1, payload.phases.findIndex((phase) => phase.state === "active") + 1 || payload.phases.filter((phase) => phase.state === "complete").length) : undefined,
      logical: formatBytes(payload.metrics?.logical_bytes),
      encoded: formatBytes(payload.metrics?.encoded_bytes),
      sourceRate: payload.metrics?.source_rate_bps ? `${formatBytes(payload.metrics.source_rate_bps)}/s` : "—",
      driveRate: payload.metrics?.drive_rate_bps ? `${formatBytes(payload.metrics.drive_rate_bps)}/s` : "—",
      currentTape: payload.volumes?.find((volume) => volume.status === "loaded")?.tape_id || "—",
      media: (payload.volumes || []).map((volume) => volume.tape_id).filter(Boolean),
      events: (payload.events || []).map((event) => [formatDate(event.time), event.title, event.detail]),
      snapshot: payload.source?.snapshot || payload.job.recovery_point,
      attempt: payload.correlation_id || payload.job.attempt,
      error: payload.required_action?.detail || null,
      eta: payload.job.state_label || titleCase(payload.job.state)
    } : payload;
    const normalized = normalizeJob(raw, Math.max(index, 0));
    if (index >= 0) state.data.jobs[index] = normalized;
    else state.data.jobs.unshift(normalized);
    if (state.route === "jobs" && state.subroute === id) renderJobDetail(id);
  }

  function parseRoute() {
    const raw = (window.location.hash || "#overview").slice(1).replace(/^\//, "");
    const [route, ...rest] = raw.split("/");
    const allowed = ["overview", "jobs", "tapes", "restore", "movements", "drives", "verification", "settings"];
    state.route = allowed.includes(route) ? route : "overview";
    state.subroute = rest.length ? decodeURIComponent(rest.join("/")) : null;
  }

  function openDrawer(html) {
    const layer = $("#drawer-layer");
    if (!layer) return;
    state.lastFocus = document.activeElement;
    layer.innerHTML = `<aside class="drawer" role="dialog" aria-modal="true">${html}</aside>`;
    layer.hidden = false;
    document.body.style.overflow = "hidden";
    const focusTarget = $("button, [href], input, select, textarea, [tabindex]:not([tabindex='-1'])", layer);
    if (focusTarget) focusTarget.focus();
  }

  function closeDrawer() {
    const layer = $("#drawer-layer");
    if (!layer || layer.hidden) return;
    layer.hidden = true;
    layer.innerHTML = "";
    restoreBodyAndFocus();
  }

  function openModal(title, body, footer, label = title) {
    const layer = $("#modal-layer");
    if (!layer) return;
    state.lastFocus = document.activeElement;
    layer.innerHTML = `<section class="modal" role="dialog" aria-modal="true" aria-label="${escapeHtml(label)}"><div class="modal-head"><h2>${escapeHtml(title)}</h2><button class="icon-button" type="button" data-action="close-modal" aria-label="Close dialog">&times;</button></div><div class="modal-body">${body}</div>${footer ? `<div class="modal-foot">${footer}</div>` : ""}</section>`;
    layer.hidden = false;
    document.body.style.overflow = "hidden";
    const focusTarget = $("input, select, textarea, button, [href]", layer);
    if (focusTarget) focusTarget.focus();
  }

  function closeModal() {
    const layer = $("#modal-layer");
    if (!layer || layer.hidden) return;
    layer.hidden = true;
    layer.innerHTML = "";
    restoreBodyAndFocus();
  }

  function restoreBodyAndFocus() {
    if ($("#drawer-layer").hidden && $("#modal-layer").hidden) document.body.style.overflow = "";
    if (state.lastFocus && document.contains(state.lastFocus)) state.lastFocus.focus();
    state.lastFocus = null;
  }

  function trapFocus(event, layer) {
    if (event.key !== "Tab") return;
    const nodes = $$("button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex='-1'])", layer).filter((node) => node.offsetParent !== null);
    if (!nodes.length) return;
    const first = nodes[0];
    const last = nodes[nodes.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }

  function toast(title, message, tone = "success") {
    const region = $("#toast-region");
    if (!region) return;
    const item = document.createElement("div");
    item.className = `toast ${tone}`;
    item.innerHTML = `<span aria-hidden="true">${tone === "failed" ? "!" : tone === "warning" ? "&#8594;" : "&#10003;"}</span><div><strong>${escapeHtml(title)}</strong><span>${escapeHtml(message)}</span></div><button type="button" aria-label="Dismiss notification">&times;</button>`;
    item.querySelector("button").addEventListener("click", () => item.remove());
    region.appendChild(item);
    window.setTimeout(() => item.remove(), 6500);
  }

  function showNotifications() {
    openDrawer(`<div class="drawer-head"><div><span class="eyebrow">Durable events</span><h2>Notifications</h2></div><button class="icon-button" data-action="close-drawer" aria-label="Close notifications">&times;</button></div><div class="drawer-body">
      <div class="notice"><span class="notice-icon">&#8594;</span><div class="notice-copy"><strong>Compatible continuation tape required</strong><span>Nightly production protection · unresolved 8 minutes</span></div></div>
      <ul class="action-list section"><li class="action-item"><span class="action-symbol failed">!</span><div class="action-copy"><strong>Archive movement overdue</strong><span>Arrival at Iron Mountain remains unconfirmed.</span></div><span class="action-time">18h</span></li><li class="action-item"><span class="action-symbol failed">!</span><div class="action-copy"><strong>Configuration export failed</strong><span>FULL_ADMIN authority is required separately.</span></div><span class="action-time">13h</span></li><li class="action-item"><span class="action-symbol">&#8635;</span><div class="action-copy"><strong>Device reconciliation required</strong><span>Standalone recovery drive path changed.</span></div><span class="action-time">2d</span></li></ul>
    </div><div class="drawer-foot"><button class="button" data-action="close-drawer">Close</button><button class="button button-primary" data-action="notification-settings">Notification settings</button></div>`);
  }

  function showPreflightModal() {
    const failed = state.data.preflight.filter((item) => !/pass|ready|success/i.test(item.status));
    openModal("Preflight status", `<div class="metric-strip"><div class="metric"><span>Passed</span><strong>${state.data.preflight.length - failed.length}</strong><small>Required dependencies</small></div><div class="metric"><span>Action</span><strong>${failed.length}</strong><small>Before complete readiness</small></div></div><ul class="readiness-list section">${failed.map((item) => `<li class="readiness-item blocked"><span class="gate-icon">!</span><div><strong>${escapeHtml(item.name)}</strong><span>${escapeHtml(item.detail)}</span></div>${statusTag(item.status)}</li>`).join("")}</ul>`, `<button class="button" data-action="close-modal">Close</button><button class="button button-primary" data-action="open-system-preflight">Open system preflight</button>`);
  }

  function showBackupModal() {
    openModal("Configure protection", `<div class="form-grid"><div class="form-field full"><label for="policy-name">Policy name</label><input class="input" id="policy-name" value="Production weekly protection"></div><div class="form-field full"><label for="policy-source">TrueNAS source</label><select class="select" id="policy-source"><option>tank/production/media</option><option>tank/homes</option><option>fastpool/virtualization/production-zvols</option></select></div><div class="form-field"><label for="policy-mode">Representation</label><select class="select" id="policy-mode"><option>File + native ZFS</option><option>File archive</option><option>Native ZFS</option></select></div><div class="form-field"><label for="policy-schedule">Schedule</label><select class="select" id="policy-schedule"><option>Weekly full + daily incrementals</option><option>Archive on demand</option></select></div></div><div class="notice info section"><span class="notice-icon">i</span><div class="notice-copy"><strong>Defaults: encryption and full read-back enabled</strong><span>Before enabling, TapeScale will preview media, time, temporary space, dependency retention, and off-site movement demand.</span></div></div>`, `<button class="button" data-action="close-modal">Cancel</button><button class="button button-primary" data-action="save-policy">Continue to source coverage</button>`);
  }

  function showRunJobModal() {
    openModal("Run permitted job", `<div class="form-field"><label for="manual-job">Protection policy</label><select class="select" id="manual-job"><option>Nightly production protection</option><option>Team homes incremental</option><option>VM native stream protection</option><option>TrueNAS configuration export</option></select><small>A compatible base and current preflight are evaluated before queueing.</small></div><div class="notice info section"><span class="notice-icon">i</span><div class="notice-copy"><strong>Manual run uses a new immutable snapshot</strong><span>Existing user snapshots will not be deleted when the job finishes.</span></div></div>`, `<button class="button" data-action="close-modal">Cancel</button><button class="button button-primary" data-action="confirm-run-job">Queue job</button>`);
  }

  function showIdentifyMediaModal() {
    openModal("Identify inserted media", `<div class="form-grid"><div class="form-field full"><label for="identify-drive">Drive</label><select class="select" id="identify-drive"><option>Drive 1 · IBM ULTRIUM-HH9</option><option>Drive 2 · IBM ULTRIUM-HH8</option></select></div><div class="form-field full"><label for="expected-label">Expected label (optional)</label><input class="input mono" id="expected-label" placeholder="TS-L9-004220"><small>Leaving this blank performs read-only identity inspection. No label, erase, or format action is implied.</small></div></div><div class="notice section"><span class="notice-icon">!</span><div class="notice-copy"><strong>Unknown media starts protected</strong><span>A printed label alone never grants overwrite authority.</span></div></div>`, `<button class="button" data-action="close-modal">Cancel</button><button class="button button-primary" data-action="confirm-identify">Inspect read only</button>`);
  }

  function showCancelJobModal(id) {
    const job = state.data.jobs.find((item) => item.id === id);
    if (!job) return;
    openModal("Cancel job safely", `<div class="notice danger"><span class="notice-icon">!</span><div class="notice-copy"><strong>${escapeHtml(job.name)}</strong><span>Cancellation preserves previously valid backups, records incomplete media honestly, and runs required snapshot cleanup only at a safe engine boundary.</span></div></div><ul class="detail-list section"><li><span>Current state</span><strong>${escapeHtml(titleCase(job.status))}</strong></li><li><span>Protected snapshot</span><strong class="mono">${escapeHtml(job.snapshot)}</strong></li><li><span>Affected media</span><strong>${job.media.map((item) => escapeHtml(item)).join(", ") || "None"}</strong></li></ul><div class="form-field section"><label for="reauth-password">Administrator password</label><input class="input" type="password" id="reauth-password" autocomplete="current-password" placeholder="Reauthenticate to continue"></div>`, `<button class="button" data-action="close-modal">Keep job running</button><button class="button button-danger" data-action="confirm-cancel-job" data-id="${escapeHtml(id)}">Cancel at safe boundary</button>`);
  }

  function showMovementConfirmation(id, stage) {
    const move = state.data.movements.find((item) => item.id === id);
    if (!move) return;
    const arrival = stage === "arrival";
    openModal(arrival ? "Confirm physical arrival" : "Confirm physical departure", `<div class="notice ${arrival ? "success" : "info"}"><span class="notice-icon">${arrival ? "&#10003;" : "&#8594;"}</span><div class="notice-copy"><strong>${arrival ? escapeHtml(move.destination) : escapeHtml(move.source)}</strong><span>This records physical custody with your account and the current timestamp. It is not inferred from a schedule or export slot.</span></div></div><div class="section"><span class="field-label">Selected tapes</span><div class="filter-row">${move.tapes.filter((item) => item.checked).map((item) => tapeId(item.id)).join(" ")}</div></div><label class="check-option section"><input type="checkbox" id="custody-attestation"> I physically verified every listed tape ${arrival ? "at the destination" : "leaving the source"}.</label>`, `<button class="button" data-action="close-modal">Cancel</button><button class="button button-primary" data-action="commit-movement" data-id="${escapeHtml(id)}" data-stage="${stage}">Record ${arrival ? "arrival" : "departure"}</button>`);
  }

  function showVerificationModal() {
    openModal("Schedule verification", `<div class="form-grid"><div class="form-field full"><label for="verification-target">Protected content</label><select class="select" id="verification-target"><option>Weekly production full · Weekly 45</option><option>Team homes · TS-L8-002177</option><option>Finance archive FY2020</option></select></div><div class="form-field"><label for="verification-scope">Scope</label><select class="select" id="verification-scope"><option>Full recovery-path read-back</option><option>Sample verification</option><option>Representative restore test</option></select></div><div class="form-field"><label for="verification-when">Earliest start</label><input class="input" id="verification-when" type="datetime-local" value="2026-11-13T09:00"></div></div><div class="notice info section"><span class="notice-icon">i</span><div class="notice-copy"><strong>Restore priority applies at safe boundaries</strong><span>Verification will never interrupt an active tape write.</span></div></div>`, `<button class="button" data-action="close-modal">Cancel</button><button class="button button-primary" data-action="confirm-verification">Schedule</button>`);
  }

  function openVerificationDrawer(id) {
    const record = state.data.verification.find((item) => item.id === id);
    if (!record) return;
    openDrawer(`<div class="drawer-head"><div><span class="eyebrow">Evidence record</span><h2>${escapeHtml(record.evidence)}</h2></div><button class="icon-button" data-action="close-drawer" aria-label="Close evidence">&times;</button></div><div class="drawer-body"><div class="detail-hero"><div class="detail-hero-row"><strong>${escapeHtml(record.target)}</strong>${statusTag(record.result)}</div><p>${escapeHtml(record.scope)}</p></div><div class="evidence-stamp"><span class="evidence-seal">${/verif|success/.test(record.result) ? "&#10003;" : "!"}</span><span><strong>${escapeHtml(record.detail)}</strong><small>Evidence identity ${escapeHtml(record.id)}</small></span></div><section class="detail-section"><h3>Recorded context</h3><ul class="detail-list"><li><span>Media</span><strong>${escapeHtml(record.media)}</strong></li><li><span>Drive</span><strong>${escapeHtml(record.drive)}</strong></li><li><span>Started</span><strong>${escapeHtml(record.started)}</strong></li><li><span>Scope</span><strong>${escapeHtml(record.scope)}</strong></li><li><span>Outcome</span><strong>${escapeHtml(titleCase(record.result))}</strong></li></ul></section><div class="notice info section"><span class="notice-icon">i</span><div class="notice-copy"><strong>Evidence semantics</strong><span>A drive write acknowledgement is never substituted for application recovery-path read-back. Interrupted checks do not pass.</span></div></div></div><div class="drawer-foot"><button class="button" data-action="close-drawer">Close</button><button class="button button-primary" data-action="export-evidence">Export signed record</button></div>`);
  }

  function downloadJson(filename, value) {
    const blob = new Blob([JSON.stringify(value, null, 2)], { type: "application/json" });
    const url = URL.createObjectURL(blob);
    const link = document.createElement("a");
    link.href = url;
    link.download = filename;
    document.body.appendChild(link);
    link.click();
    link.remove();
    URL.revokeObjectURL(url);
  }

  async function queueRestore() {
    const rp = selectedRecoveryPoint();
    const body = {
      recovery_point_id: rp.id,
      scope: state.restore.mode === "all" ? { type: "complete" } : { type: "paths", paths: state.restore.selectedFiles },
      destination: state.restore.destination,
      conflict_policy: state.restore.conflicts
    };
    const button = $("[data-action='queue-restore']");
    if (button) {
      button.disabled = true;
      button.textContent = "Queueing…";
    }
    const result = await apiPost("/restores/plan", body);
    state.restore.queued = true;
    renderRestore();
    toast("Restore plan prepared", result.ok ? "The API evaluated prerequisites; no backend operation was started." : "Preview mode retained the plan locally; no backend operation was started.", result.ok ? "success" : "warning");
  }

  async function commitMovement(id, stage) {
    const move = state.data.movements.find((item) => item.id === id);
    if (!move) return;
    const attestation = $("#custody-attestation");
    if (!attestation || !attestation.checked) {
      toast("Physical verification required", "Confirm the attestation before recording custody.", "failed");
      return;
    }
    const body = {
      actor: "Preview operator",
      destination: stage === "arrival" ? move.destination : "In transit",
      tape_ids: move.tapes.filter((item) => item.checked).map((item) => item.id),
      note: stage === "arrival" ? "Physical arrival confirmed in TapeScale console" : "Physical departure confirmed in TapeScale console",
      confirmed_at: new Date().toISOString()
    };
    const result = await apiPost(`/movements/${encodeURIComponent(id)}/confirm`, body);
    const time = "12 Nov 2026 " + new Date().toISOString().slice(11, 16) + " UTC";
    if (stage === "arrival") {
      move.stage = 4;
      move.status = "successful";
      move.readiness = "Complete · custody confirmed";
      move.history.unshift(["Arrival confirmed", time, `Preview operator · ${move.destination}`]);
    } else {
      move.stage = 3;
      move.status = "in transit";
      move.readiness = "Departed · arrival confirmation required";
      move.history.unshift(["Departure confirmed", time, `Preview operator · ${move.source}`]);
    }
    closeModal();
    renderMovements();
    updateNavCounts();
    toast("Custody event recorded", result.ok ? "The preview API accepted the simulated event; this is not physical custody evidence." : "Preview mode appended the event locally; the backend was unavailable.", result.ok ? "success" : "warning");
  }

  function openMediaInstructions() {
    openDrawer(`<div class="drawer-head"><div><span class="eyebrow">Media request</span><h2>Load continuation tape</h2></div><button class="icon-button" data-action="close-drawer" aria-label="Close instructions">&times;</button></div><div class="drawer-body"><div class="detail-hero"><div class="detail-hero-row">${tapeId("TS-L9-004220", true)}${statusTag("Required", "warning")}</div><p>Nightly production protection · volume 3 of 3 estimated</p></div><ol class="readiness-list"><li class="readiness-item"><span class="gate-icon">1</span><div><strong>Verify exact tape identity</strong><span>Barcode TS4220L9 · Routine Weekly 46 pool</span></div></li><li class="readiness-item"><span class="gate-icon">2</span><div><strong>Place in Library A intake</strong><span>Do not remove any active or reserved tape.</span></div></li><li class="readiness-item"><span class="gate-icon">3</span><div><strong>Run inventory</strong><span>TapeScale will validate on-tape identity and write eligibility.</span></div></li></ol><div class="notice section"><span class="notice-icon">!</span><div class="notice-copy"><strong>Do not substitute unknown media</strong><span>A matching printed label alone does not authorize overwrite.</span></div></div></div><div class="drawer-foot"><button class="button" data-action="close-drawer">Close</button><button class="button button-primary" data-action="refresh">Inventory library</button></div>`);
  }

  function handleClick(event) {
    const actionNode = event.target.closest("[data-action]");
    const action = actionNode && actionNode.dataset.action;
    if (action) {
      switch (action) {
        case "open-nav": document.body.classList.add("nav-open"); break;
        case "close-nav": document.body.classList.remove("nav-open"); break;
        case "close-drawer": closeDrawer(); break;
        case "close-modal": closeModal(); break;
        case "refresh": closeDrawer(); hydrateData(true).then(() => renderRoute({ focus: false })); break;
        case "notifications": showNotifications(); break;
        case "show-preflight": showPreflightModal(); break;
        case "open-system-preflight": closeModal(); state.settingsTab = "preflight"; window.location.hash = "settings"; renderRoute(); break;
        case "configure": case "new-backup": showBackupModal(); break;
        case "save-policy": closeModal(); toast("Policy draft ready", "Source coverage and capacity forecasting are the next required steps."); break;
        case "run-job": showRunJobModal(); break;
        case "confirm-run-job": closeModal(); toast("Job queued", "Preflight and compatible-base evaluation will run before media is touched."); break;
        case "back-jobs": window.location.hash = "jobs"; break;
        case "media-instructions": openMediaInstructions(); break;
        case "job-diagnostics": openDrawer(`<div class="drawer-head"><h2>Sanitized diagnostics</h2><button class="icon-button" data-action="close-drawer">&times;</button></div><div class="drawer-body"><pre class="mono">correlation_id: 01JC2A5BC0EWG42EP2Z8N6GGN7\nclass: device_position_uncertain\ndrive: 500507630F00A149\naction: destructive continuation blocked\nsecrets_redacted: true</pre></div><div class="drawer-foot"><button class="button" data-action="close-drawer">Close</button><button class="button button-primary" data-action="export-diagnostics">Export</button></div>`); break;
        case "export-diagnostics": downloadJson("tapescale-diagnostics-sanitized.json", { exported_at: new Date().toISOString(), secrets_redacted: true, preflight: state.data.preflight, service_endpoints: state.live }); toast("Diagnostic export created", "Credentials and sensitive path lists were excluded."); break;
        case "cancel-job": showCancelJobModal(actionNode.dataset.id); break;
        case "confirm-cancel-job": {
          const password = $("#reauth-password");
          if (!password || !password.value) { toast("Reauthentication required", "Enter the administrator password to continue.", "failed"); break; }
          const job = state.data.jobs.find((item) => item.id === actionNode.dataset.id);
          if (job) { job.status = "canceled"; job.error = "Canceled by administrator at a documented safe boundary."; job.events.unshift(["Now", "Cancellation requested", "Previously valid backups remain protected."]); }
          closeModal(); renderJobDetail(actionNode.dataset.id); toast("Cancellation requested", "The worker will stop only at its documented safe boundary.", "warning"); break;
        }
        case "identify-media": showIdentifyMediaModal(); break;
        case "confirm-identify": closeModal(); toast("Read-only inspection queued", "No label, erase, format, or overwrite operation was authorized."); break;
        case "create-movement": closeDrawer(); window.location.hash = "movements"; toast("Movement planner opened", `Create a custody task for ${actionNode.dataset.id}.`); break;
        case "discover-devices": toast("Device discovery started", "Stable serials, WWNs, capabilities, and paths will be reconciled without implicit rebinding."); break;
        case "reconcile-drive": openModal("Reconcile stable device identity", `<div class="notice"><span class="notice-icon">!</span><div class="notice-copy"><strong>HU1918M4D2 found at a new path</strong><span>Confirm the serial, WWN, model, and absence of active ownership before updating the binding.</span></div></div><ul class="detail-list section"><li><span>Previous path</span><strong class="mono">/dev/nst2</strong></li><li><span>Observed path</span><strong class="mono">/dev/tape/by-id/scsi-3500110a0018e4902-nst</strong></li><li><span>Serial</span><strong class="mono">HU1918M4D2</strong></li></ul>`, `<button class="button" data-action="close-modal">Keep blocked</button><button class="button button-primary" data-action="confirm-reconcile">Confirm binding</button>`); break;
        case "confirm-reconcile": closeModal(); toast("Device binding reconciled", "The stable identity is ready for a fresh preflight."); break;
        case "restore-reset": state.restore = { step: 1, recoveryPoint: "rp-1112", selectedFiles: ["/Brandon", "/Engineering/roadmap-2027.xlsx"], mode: "selected", recallCreated: false, destination: "tank/restores/team-homes-2026-11-12", conflicts: "skip", queued: false }; renderRestore(); break;
        case "restore-step": state.restore.step = Number(actionNode.dataset.step); renderRestore(); break;
        case "restore-back": state.restore.step = Math.max(1, state.restore.step - 1); renderRestore(); break;
        case "restore-next": state.restore.step = Math.min(5, state.restore.step + 1); renderRestore(); break;
        case "create-recall": state.restore.recallCreated = true; renderRestore(); toast("Recall task created", "Custody remains unchanged until physical arrival is confirmed."); break;
        case "queue-restore": queueRestore(); break;
        case "print-manifest": window.print(); break;
        case "new-movement": openModal("Plan movement", `<div class="form-grid"><div class="form-field full"><label>Movement type</label><select class="select"><option>Take off site</option><option>Bring back</option><option>Emergency recall</option></select></div><div class="form-field"><label>Source</label><select class="select"><option>Library A · Export station</option><option>Vault North</option></select></div><div class="form-field"><label>Destination</label><select class="select"><option>Vault North</option><option>Library A · Intake</option></select></div></div>`, `<button class="button" data-action="close-modal">Cancel</button><button class="button button-primary" data-action="save-movement">Continue to tape selection</button>`); break;
        case "save-movement": closeModal(); toast("Movement draft created", "Readiness gates and complete-set coverage must pass before export."); break;
        case "confirm-departure": showMovementConfirmation(actionNode.dataset.id, "departure"); break;
        case "confirm-arrival": showMovementConfirmation(actionNode.dataset.id, "arrival"); break;
        case "commit-movement": commitMovement(actionNode.dataset.id, actionNode.dataset.stage); break;
        case "correct-custody": openModal("Record custody correction", `<div class="form-field"><label for="correction-note">Correction and evidence</label><textarea class="textarea" id="correction-note" placeholder="Explain the observed discrepancy and corrected location"></textarea><small>The original custody event remains in history.</small></div>`, `<button class="button" data-action="close-modal">Cancel</button><button class="button button-primary" data-action="save-correction">Append correction</button>`); break;
        case "save-correction": closeModal(); toast("Correction appended", "Prior custody evidence was retained."); break;
        case "schedule-verification": showVerificationModal(); break;
        case "confirm-verification": closeModal(); toast("Verification scheduled", "Resource and coverage safety will be rechecked at execution time."); break;
        case "export-evidence": downloadJson("tapescale-verification-evidence.json", { exported_at: new Date().toISOString(), records: state.data.verification }); toast("Evidence export created", "The export identifies scope, media, drive, time, and outcome."); break;
        case "notification-settings": closeDrawer(); state.settingsTab = "security"; window.location.hash = "settings"; break;
        case "user-menu": openModal("Preview profile", `<div class="detail-hero"><div class="detail-hero-row"><span class="avatar">PV</span>${statusTag("Simulation", "warning")}</div><p>Interactive engineering-preview profile</p></div><ul class="detail-list"><li><span>Authentication</span><strong>Not implemented</strong></li><li><span>Production authority</span><strong>None</strong></li><li><span>Audit actor</span><strong>Fixture only</strong></li></ul>`, `<button class="button" data-action="close-modal">Close</button><button class="button" data-action="manage-mfa" disabled>Security pending</button>`); break;
        case "manage-mfa": closeModal(); state.settingsTab = "security"; window.location.hash = "settings"; renderRoute(); break;
        case "key-export": openModal("Export recovery key package", `<div class="notice danger"><span class="notice-icon">!</span><div class="notice-copy"><strong>Administrator reauthentication required</strong><span>The export is portable secret material. The affected key lineages and retained backups will be recorded in the audit event.</span></div></div><div class="form-field section"><label for="key-password">Administrator password</label><input class="input" id="key-password" type="password" autocomplete="current-password"></div>`, `<button class="button" data-action="close-modal">Cancel</button><button class="button button-primary" data-action="confirm-key-export">Create encrypted export</button>`); break;
        case "confirm-key-export": {
          const keyPassword = $("#key-password");
          if (!keyPassword || !keyPassword.value) { toast("Reauthentication required", "Enter the administrator password.", "failed"); break; }
          closeModal(); toast("Recovery export prepared", "Key material remains encrypted and the export was audited."); break;
        }
        case "site-menu": toast("Single-site baseline", "Fleet management is outside the release-one topology.", "warning"); break;
        case "preview-loading": renderLoading(); window.setTimeout(() => { if (state.route === "settings") renderSettings(); }, 1400); break;
        default: break;
      }
      return;
    }

    const settingsTab = event.target.closest("[data-settings-tab]");
    if (settingsTab) {
      state.settingsTab = settingsTab.dataset.settingsTab;
      renderSettings();
      return;
    }
    const jobRow = event.target.closest("[data-job-id]");
    if (jobRow) { window.location.hash = `jobs/${encodeURIComponent(jobRow.dataset.jobId)}`; return; }
    const tapeRow = event.target.closest("[data-tape-id]");
    if (tapeRow) { openTapeDrawer(tapeRow.dataset.tapeId); return; }
    const movement = event.target.closest("[data-movement-id]");
    if (movement) { state.selectedMovement = movement.dataset.movementId; renderMovements(); return; }
    const evidence = event.target.closest("[data-verification-id]");
    if (evidence) { openVerificationDrawer(evidence.dataset.verificationId); return; }
    const slot = event.target.closest("[data-slot]");
    if (slot) {
      const slotData = state.data.slots.find((item) => String(item[0]) === String(slot.dataset.slot));
      if (slotData && slotData[1]) {
        const known = state.data.tapes.find((item) => item.id === slotData[1]);
        if (known) openTapeDrawer(known.id);
        else toast(`Slot ${slotData[0]} · ${slotData[1]}`, "Inventory identity is known; no detailed catalog record is loaded.");
      } else toast(`Slot ${slot.dataset.slot} is empty`, "Observed by the last changer inventory.", "warning");
    }
  }

  let searchTimer = null;
  function handleInput(event) {
    const kind = event.target.dataset.input;
    if (!kind) return;
    if (kind === "job-search" || kind === "tape-search") {
      state.search = event.target.value;
      window.clearTimeout(searchTimer);
      const selection = event.target.value.length;
      searchTimer = window.setTimeout(() => {
        if (kind === "job-search") renderJobs(); else renderTapes();
        const field = $(`[data-input='${kind}']`);
        if (field) { field.focus(); field.setSelectionRange(selection, selection); }
      }, 120);
    }
    if (kind === "restore-destination") state.restore.destination = event.target.value;
  }

  function handleChange(event) {
    const kind = event.target.dataset.input;
    if (!kind) return;
    if (kind === "job-filter") { state.jobFilter = event.target.value; renderJobs(); }
    if (kind === "tape-filter") { state.tapeFilter = event.target.value; renderTapes(); }
    if (kind === "recovery-point") { state.restore.recoveryPoint = event.target.value; state.restore.recallCreated = false; renderRestore(); }
    if (kind === "restore-mode") { state.restore.mode = event.target.value; renderRestore(); }
    if (kind === "restore-file") {
      if (event.target.checked) state.restore.selectedFiles.push(event.target.value);
      else state.restore.selectedFiles = state.restore.selectedFiles.filter((item) => item !== event.target.value);
    }
    if (kind === "restore-conflicts") { state.restore.conflicts = event.target.value; renderRestore(); }
    if (kind === "packing-tape") {
      const move = state.data.movements.find((item) => item.id === event.target.dataset.move);
      const tape = move && move.tapes.find((item) => item.id === event.target.value);
      if (tape) tape.checked = event.target.checked;
      renderMovements();
    }
    if (kind === "scenario") {
      state.scenario = event.target.value;
      sessionStorage.setItem("tapescale-scenario", state.scenario);
      renderSettings();
      const select = $("[data-input='scenario']");
      if (select) select.value = state.scenario;
    }
  }

  function handleKeydown(event) {
    const drawer = $("#drawer-layer");
    const modal = $("#modal-layer");
    if (event.key === "Escape") {
      if (!modal.hidden) closeModal();
      else if (!drawer.hidden) closeDrawer();
      else document.body.classList.remove("nav-open");
      return;
    }
    if (!modal.hidden) trapFocus(event, modal);
    else if (!drawer.hidden) trapFocus(event, drawer);
    if ((event.key === "Enter" || event.key === " ") && event.target.matches("tr[data-job-id], tr[data-tape-id], tr[data-verification-id]")) {
      event.preventDefault();
      event.target.click();
    }
  }

  async function boot() {
    document.addEventListener("click", handleClick);
    document.addEventListener("input", handleInput);
    document.addEventListener("change", handleChange);
    document.addEventListener("keydown", handleKeydown);
    window.addEventListener("hashchange", () => { parseRoute(); renderRoute(); });
    $("#drawer-layer").addEventListener("click", (event) => { if (event.target === event.currentTarget) closeDrawer(); });
    $("#modal-layer").addEventListener("click", (event) => { if (event.target === event.currentTarget) closeModal(); });
    parseRoute();
    await hydrateData();
    renderRoute({ focus: false });
  }

  boot();
})();
