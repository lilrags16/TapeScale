# Qualification and acceptance ledger

This ledger prevents implementation progress from being confused with physical qualification.

## Status vocabulary

- **Implemented**: the relevant code path exists and has automated evidence in this repository. This does not imply hardware qualification unless the evidence column names physical hardware.
- **Simulated**: deterministic model or fake-engine evidence exists. It proves control-plane behavior only.
- **Not run**: required evidence has not been executed or retained.
- **Blocked**: a named prerequisite prevents execution and is release-blocking.
- **Qualified**: reserved for reviewed evidence from the named TrueNAS, engine, tape, changer, media, and recovery environment. Nothing in the initial repository is Qualified.

Do not change an item to Qualified without a dated report containing fixture, exact software/hardware, steps, expected and observed results, retained artifacts, reviewer, and unresolved limitations.

## Current ledger

This is an initial engineering ledger. Automated test references should be added as the corresponding code lands; physical columns remain Not run until laboratory execution.

| ID | Acceptance case | Software status | Physical/operational evidence | Release note |
|---|---|---|---|---|
| AT-01 | Clean installation and preflight | Implemented/Simulated | Not run | SAS discovery, identity reconciliation, gated motion, and a pinned G0.1 `btape` harness exist; named TrueNAS execution and remaining dependencies are pending. |
| AT-02 | LTO-1 through LTO-10 compatibility | Simulated | Not run | Rules can be unit-tested; no generation is physically qualified. |
| AT-03 | Manual multi-volume job | Simulated | Not run | Requires physical EOM and wrong continuation media. |
| AT-04 | Automated inventory and movement | Simulated | Not run | Requires a named changer and import/export-slot evidence. |
| AT-05 | Concurrent compatible drives | Implemented/Simulated | Not run | Durable per-drive motion/qualification leases, media reservations, and fencing are tested in PostgreSQL; two-drive hardware and engine routing remain unproven. |
| AT-06 | Snapshot protection and cleanup | Simulated | Not run | Requires real TrueNAS snapshots, holds, hooks, and pressure cases. |
| AT-07 | File content and metadata round trip | Implemented (G0.2 subset) | Not run | G0.2 compares paths, SHA-256 content, and declared manifest metadata after a Bacula restore; representative POSIX and TrueNAS NFSv4 ACL fixtures remain required. |
| AT-08 | Native dataset reconstruction | Not run | Not run | Requires actual `zfs send`/`receive` path and alternate pool. |
| AT-09 | Explicit zvol coverage | Not run | Not run | UI/model treatment alone is insufficient. |
| AT-10 | Exact selected recovery point | Implemented/Simulated | Not run | G0.2 binds a normalized read-only snapshot selection, Bacula JobId, cartridge, and restore comparison; broader namespace/snapshot automation remains. |
| AT-11 | Object larger than one cartridge | Simulated | Not run | Requires file and native-stream physical spanning. |
| AT-12 | Cancellation and host/process failure | Implemented/Simulated | Not run | Motion, destructive-test, G0.2, and restore-rehearsal work is durable and fenced; a running rehearsal has an explicit observed Bacula cancellation path. Controlled power/process interruption on physical media remains. |
| AT-13 | Unknown media/device loss/position | Implemented/Simulated | Not run | Missing/path-changed SAS devices, stale identity, protected/foreign/WORM inventory, and unverifiable postconditions are blocked. Restore rehearsals re-probe the exact identity and rely on Bacula volume-header/catalog matching without exposing a tape-write primitive. Physical wrong-media/reset/disconnect evidence remains. |
| AT-14 | Compression modes restore | Not run | Not run | Each supported resolved pipeline requires byte-exact restore evidence. |
| AT-15 | Corrupt/reordered/truncated content | Simulated | Not run | Record/authentication fault injection must be tied to selected engine. |
| AT-16 | Media read-back and restore tests | Implemented (G0.2 subset); restarted source-independent file-backed restore runs in CI | Not run | CI proves label, Full backup, `Level=Data`, initial restore, daemon shutdown/restart, the original source made unavailable, a second restore, byte comparison, and both catalog relationships without hardware. The attended physical rehearsal path is implemented, but named SAS/LTO execution evidence is still required. |
| AT-17 | Loss of original host and catalog | Not run | Not run | Requires clean replacement installation and independently recovered keys. |
| AT-18 | Independent keys and rotation | Simulated | Not run | Real encrypted historical backups and portable-key recovery remain. |
| AT-19 | Dependencies/shared tapes/archives | Implemented/Simulated | Not run | Dependency and overwrite-gate tests are control-plane evidence. |
| AT-20 | Overlap/downtime/local time | Simulated | Not run | Scheduler model requires full timezone and restart suite. |
| AT-21 | Confirmed and missed exchanges | Implemented/Simulated | Not run | Custody workflow does not prove physical movement. |
| AT-22 | Minimum recovery coverage | Simulated | Not run | Emergency recall and replacement-delivery cases remain operational exercises. |
| AT-23 | Roles and safe destinations | Implemented/Simulated | Not run | Add API authorization and adversarial filesystem evidence. |
| AT-24 | Container replacement/migration | Not run | Not run | Requires previous-version database and active-job drain exercise. |
| AT-25 | Backpressure/exhausted capacity | Simulated | Not run | Real spool, destination, snapshot, and staging pressure remain. |
| AT-26 | Representative workloads | Not run | Not run | Targets must be measured, not invented. |
| AT-27 | Source errors/existing destinations | Implemented/Simulated | Not run | G0.2 rejects writable/changed sources; the later rehearsal does not need the source and only creates a new server-generated isolated destination. Replace, merge, and publication are unavailable. Physical error fixtures and broader metadata coverage remain. |
| AT-28 | Durable events/actionable status | Implemented/Simulated | Not run | Motion, qualification, G0.2, and restore-rehearsal intents, append-only events/artifacts, claims, reservations, phases, cancellation, results, and audit evidence are durable; broader end-to-end restart and delivery-channel evidence is pending. |
| AT-29 | Rust boundary/Linux dependency audit | Implemented (source boundary) | Not run | Rust owns the custom workflow; Debian Bacula 15.0.3 packages are pinned and process-isolated. Final image/SBOM/license audit remains. |
| AT-30 | Safe external-tool execution | Implemented/Simulated | Not run | `mt-st` and Bacula daemon startup use closed argv; `btape` has a fixed transcript and required markers; after its durable destructive boundary G0.2 uses one fixed internal BOT file mark to invalidate the prior btape label, then generated configs and a restricted fixed-input `bconsole` adapter with structured JobId/status correlation. Physical interruption evidence remains, and kernel tape I/O may outlive a userspace deadline. |
| AT-31 | Product-specific visual/workflow review | Implemented | Not run | G0.1, G0.2, Recovery Vault planning, and attended rehearsal display exact blockers, tape/drive/destination boundaries, durable phases, cancellation, evidence, and limitations; product-owner desktop/phone review must still be recorded. |

`Implemented/Simulated` means an implementation exists but its present evidence uses simulated dependencies. A qualified backend may still be absent. `Implemented (source boundary)` is narrower than a completed runtime audit. Neither status may be shortened to Qualified.

## Engineering gates

### G0 — Architecture

Required exit evidence:

- custom Rust boundary and existing Linux dependency inventory;
- selected, pinned engine/format backed by real tape tests;
- actual TrueNAS permissions and snapshot/ZFS streaming path;
- tape/changer proof and stable device reconciliation;
- POSIX and native NFSv4 ACL round trip;
- stable logical source mapping;
- documented restart boundaries;
- reproducible measured workload plan.

Current status: **Blocked by completion/review of the engine matrix and physical laboratory evidence**.

Implemented entry stages:

- **G0.1 Bacula `btape test` harness.** A local operator pass remains a prerequisite artifact for the exact drive/cartridge, not the G0 decision. No reviewed repository qualification report is implied.
- **G0.2 first-recoverable-tape harness.** One immutable source selection is written as a full Bacula file job to one qualified scratch cartridge, reread using Bacula Data verification, restored normally to an isolated destination, and compared using independent manifests. Encryption is explicitly disabled for this lab deviation. No physical G0.2 report has been reviewed in this ledger.
- **G0.2 verified-point rehearsal.** A later attended run can re-open only that exact retained full-file point through its original catalog, read the exact single cartridge into a new isolated destination, and compare against the retained manifest. Container CI repeats the restore after daemon restart with the source unavailable. No physical rehearsal report has been reviewed.

Neither stage qualifies spanning, incrementals, changers, NFSv4 ACLs, native ZFS, encryption, interruption recovery, or clean-catalog reconstruction.

### G1 — Recovery safety

Required exit evidence: file/native/incremental round trips, physical EOM, wrong/unknown media protection, dependency retention, and controlled interruption results.

Current status: **G0.2 path implemented; full G1 evidence not run**.

### G2 — Independent recovery

Required exit evidence: clean replacement installation restores after loss of the test catalog using documented tapes and independently recovered keys.

Current status: **Not run**.

### G3 — Operations

Required exit evidence: rotation, roles, scheduling, notifications, updates, resource limits, recovery exercises, operator documentation, Rust boundary audit, safe tool integration, and product-specific UI review.

Current status: **In implementation; not passed**.

### G4 — Compatibility claims

Required exit evidence: complete LTO-1 through LTO-10 rule coverage plus named physical hardware qualification reports. Every untested combination remains disclosed.

Current status: **Compatibility modeled; no generation physically qualified**.

## Evidence report template

Create one immutable report per execution:

```text
Acceptance ID:
Date and operator:
TapeScale commit/image digest:
TrueNAS/OpenZFS/kernel/driver versions:
Engine/adapter/catalog/format versions:
Drive/changer/HBA/firmware/media identifiers:
Fixture version and checksum:
Preconditions:
Steps:
Expected result:
Observed result:
Artifacts and checksums:
Limitations/deviations:
Reviewer and decision:
```

Store large logs and images in the controlled evidence system; store a checksum-addressed index in the repository. Never commit credentials, recovery keys, unredacted path inventories, or sensitive source data.

## Claim policy

- “Supported” describes implemented product behavior only when its prerequisites are explicit.
- “Compatible” describes the generation rule matrix, not a test result.
- “Qualified” names the exact tested combination.
- “Verified” describes the configured read-back scope for a particular recovery point.
- “Restore tested” describes the exact content, metadata, destination, and date exercised.

Marketing, UI labels, diagnostics, and release notes must use the same distinctions.
