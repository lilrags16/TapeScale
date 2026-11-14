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

This is the alpha qualification ledger. Automated test references should be added as the corresponding code lands; physical columns remain Not run until laboratory execution.

| ID | Acceptance case | Software status | Physical/operational evidence | Release note |
|---|---|---|---|---|
| AT-01 | Clean installation and preflight | Implemented/Simulated | Not run | SAS discovery, identity reconciliation, gated motion, and a pinned G0.1 `btape` harness exist; named TrueNAS execution and remaining dependencies are pending. |
| AT-02 | LTO-1 through LTO-10 compatibility | Simulated | Not run | Rules can be unit-tested; no generation is physically qualified. |
| AT-03 | Manual multi-volume job | Implemented/Simulated | Not run | Operational backups retain ordered per-volume requests and authorizations, resume one Bacula JobId after attended media changes, and verify the exact catalog volume set; physical EOM and wrong-continuation-media evidence remains required. |
| AT-04 | Automated inventory and movement | Simulated | Not run | Requires a named changer and import/export-slot evidence. |
| AT-05 | Concurrent compatible drives | Implemented/Simulated | Not run | Durable per-drive motion/qualification leases, media reservations, and fencing are tested in PostgreSQL; two-drive hardware and engine routing remain unproven. |
| AT-06 | Snapshot protection and cleanup | Simulated | Not run | Requires real TrueNAS snapshots, holds, hooks, and pressure cases. |
| AT-07 | File content and metadata round trip | Implemented (G0.2 subset) | Not run | Operational full backup retains the source manifest and physical readback; G0.2 additionally compares paths, SHA-256 content, and declared manifest metadata after a Bacula restore. Representative POSIX and TrueNAS NFSv4 ACL fixtures remain required. |
| AT-08 | Native dataset reconstruction | Not run | Not run | Requires actual `zfs send`/`receive` path and alternate pool. |
| AT-09 | Explicit zvol coverage | Not run | Not run | UI/model treatment alone is insufficient. |
| AT-10 | Exact selected recovery point | Implemented/Simulated | Not run | G0.2 binds a normalized read-only snapshot selection, Bacula JobId, cartridge, and restore comparison; broader namespace/snapshot automation remains. |
| AT-11 | Object larger than one cartridge | Implemented/Simulated (file path) | Not run | The operational Bacula file path permits an object/job to cross ordered volumes without a TapeScale logical-size ceiling; forced-capacity and physical EOM restore evidence remains required, and native streaming is absent. |
| AT-12 | Cancellation and host/process failure | Implemented/Simulated | Not run | Motion, destructive-test, operational backup, G0.2, and restore-rehearsal work is durable and fenced. Waiting backup requests cancel immediately; claimed backup/rehearsal runs have observed safe-boundary Bacula cancellation paths. Controlled power/process interruption on physical media remains. |
| AT-13 | Unknown media/device loss/position | Implemented/Simulated | Not run | Missing/path-changed SAS devices, stale identity, protected/foreign/WORM inventory, and unverifiable postconditions are blocked. Restore rehearsals re-probe the exact identity and rely on Bacula volume-header/catalog matching without exposing a tape-write primitive. Physical wrong-media/reset/disconnect evidence remains. |
| AT-14 | Compression modes restore | Implemented/Simulated (write/readback) | Not run | Operational policy and the pinned adapter support automatic LZO, explicit balanced GZIP, LZO, and none with physical Data readback; the required byte-exact restore matrix and physical evidence have not run. |
| AT-15 | Corrupt/reordered/truncated content | Simulated | Not run | Record/authentication fault injection must be tied to selected engine. |
| AT-16 | Media read-back and restore tests | Implemented (ordered-volume online subset); restarted source-independent file-backed restore runs in CI | Not run | CI proves label, Full backup, `Level=Data`, initial restore, daemon shutdown/restart, the original source made unavailable, a second restore, byte comparison, and both catalog relationships without hardware. The attended physical rehearsal path now retains exact ordered handoffs and all-volume proof, but named SAS/LTO execution evidence is still required. |
| AT-17 | Loss of original host and catalog | Not run | Not run | Requires clean replacement installation and independently recovered keys. |
| AT-18 | Independent keys and rotation | Simulated | Not run | Real encrypted historical backups and portable-key recovery remain. |
| AT-19 | Dependencies/shared tapes/archives | Implemented/Simulated | Not run | Dependency and overwrite-gate tests are control-plane evidence. |
| AT-20 | Overlap/downtime/local time | Implemented (weekly-full subset)/Simulated | Not run | Saturday 02:00 full occurrences are timezone-aware, idempotent, and coalesce missed weeks into the latest due waiting request. Overlap policy, other schedules, and named-host restart/DST evidence remain. |
| AT-21 | Confirmed and missed exchanges | Implemented/Simulated | Not run | Custody workflow does not prove physical movement. |
| AT-22 | Minimum recovery coverage | Simulated | Not run | Emergency recall and replacement-delivery cases remain operational exercises. |
| AT-23 | Roles and safe destinations | Implemented/Simulated | Not run | Add API authorization and adversarial filesystem evidence. |
| AT-24 | Container replacement/migration | Not run | Not run | Requires previous-version database and active-job drain exercise. |
| AT-25 | Backpressure/exhausted capacity | Implemented/Simulated | Not run | Backup preflight proves the Storage Daemon working directory writable, reserves a conservative manifest-derived attribute allowance plus configurable free space, and, for disk-staged delivery, bounds Bacula data spooling without limiting archive size. Direct delivery bypasses the payload spool but still needs sustained source/compression throughput. Attribute spooling has no Bacula hard ceiling, so real metadata-heavy spool, destination, snapshot, direct-stream starvation, and staging pressure evidence remains. |
| AT-26 | Representative workloads | Not run | Not run | Targets must be measured, not invented. |
| AT-27 | Source errors/existing destinations | Implemented/Simulated | Not run | G0.2 rejects writable/changed sources; the later rehearsal does not need the source and only creates a new server-generated isolated destination. Replace, merge, and publication are unavailable. Physical error fixtures and broader metadata coverage remain. |
| AT-28 | Durable events/actionable status | Implemented/Simulated | Not run | Motion, qualification, operational backup plans/requests, G0.2, and restore-rehearsal intents, append-only events/artifacts, claims, reservations, phases, cancellation, results, and audit evidence are durable; broader end-to-end restart and delivery-channel evidence is pending. |
| AT-29 | Rust boundary/Linux dependency audit | Implemented (source boundary) | Not run | Rust owns the custom workflow; Debian Bacula 15.0.3 packages are pinned and process-isolated. Final image/SBOM/license audit remains. |
| AT-30 | Safe external-tool execution | Implemented/Simulated | Not run | `mt-st` and Bacula daemon startup use closed argv; `btape` has a fixed transcript and required markers; after its durable destructive boundary G0.2 uses one fixed internal BOT file mark to invalidate the prior btape label, then generated configs and a restricted fixed-input `bconsole` adapter with structured JobId/status correlation. Physical interruption evidence remains, and kernel tape I/O may outlive a userspace deadline. |
| AT-31 | Product-specific visual/workflow review | Implemented | Not run | Backup Center source browsing, guided tape preparation, G0.1/G0.2, Recovery Vault complete/selected planning, immutable scope display, and attended rehearsal display exact blockers, tape/drive/destination boundaries, durable phases, cancellation, evidence, and limitations; product-owner desktop/phone review must still be recorded. Browsing alone does not prove a selected restore. |

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
- **Guided tape preparation and browsing.** The wizard sequences the same identity reconciliation, media registration, and G0.1 confirmation gates; it creates no new physical claim. Source browsing is advisory metadata selection, while Recovery Vault browsing projects a retained manifest and performs no tape read. A selected restore exists only after the server authenticates and normalizes the requested scope and, for an exactly one-volume point, a later attended run reads and compares that scope. Multi-volume selected planning remains blocked from execution.
- **Operational attended full-backup slice.** A durable manual or weekly request waits for exact operator media assignment, then writes one compressed or uncompressed full-file Bacula job to an ordered set of manually supplied qualified scratch cartridges and requires ordered physical Data readback before atomically retaining a recovery point. Inventory uses a bounded-memory external merge and has no TapeScale total file-count or logical-byte ceiling. It is unencrypted and operator-dependent for immutable snapshot creation/retention and continuation media; verification is not a restore test. No physical multi-volume or compression report has been reviewed.
- **G0.2 first-recoverable-tape harness.** One immutable source selection is written as a full Bacula file job to one qualified scratch cartridge, reread using Bacula Data verification, restored normally to an isolated destination, and compared using independent manifests. Encryption is explicitly disabled for this lab deviation. No physical G0.2 report has been reviewed in this ledger.
- **Verified-point rehearsal.** A later attended run can re-open a compatible operational Full point through its original catalog, reserve its complete cartridge set, read each exact volume through a durable ordered handoff into a new isolated destination, and compare the complete point against the retained manifest. For an exactly one-volume point, it can instead resolve a server-normalized projection through JobId-pinned catalog identities and compare only that selected scope; selected evidence cannot satisfy the complete-point gate. Legacy G0.2 points remain one cartridge. Container CI repeats a restore after daemon restart with the source unavailable and exercises Bacula BVFS extraction in spanning mode as an engine-mechanism test only. It does not prove or enable TapeScale multi-volume selected execution. No physical single-volume, multi-volume, or selected-path rehearsal report has been reviewed.

Neither stage qualifies spanning, compression, incrementals, changers, NFSv4 ACLs, native ZFS, encryption, interruption recovery, or clean-catalog reconstruction. “Implemented” for spanning/compression means the control and adapter paths exist; AT-03, AT-11, and AT-14 remain physically **Not run**.

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
