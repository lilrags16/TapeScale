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
| AT-01 | Clean installation and preflight | Implemented/Simulated | Not run | SAS discovery, identity reconciliation, and gated lab motion exist; named TrueNAS/device proof and remaining dependencies are pending. |
| AT-02 | LTO-1 through LTO-10 compatibility | Simulated | Not run | Rules can be unit-tested; no generation is physically qualified. |
| AT-03 | Manual multi-volume job | Simulated | Not run | Requires physical EOM and wrong continuation media. |
| AT-04 | Automated inventory and movement | Simulated | Not run | Requires a named changer and import/export-slot evidence. |
| AT-05 | Concurrent compatible drives | Implemented/Simulated | Not run | Durable per-drive motion leases and fencing are tested in PostgreSQL; two-drive hardware and engine routing remain unproven. |
| AT-06 | Snapshot protection and cleanup | Simulated | Not run | Requires real TrueNAS snapshots, holds, hooks, and pressure cases. |
| AT-07 | File content and metadata round trip | Not run | Not run | Requires real POSIX and TrueNAS NFSv4 ACL fixtures. |
| AT-08 | Native dataset reconstruction | Not run | Not run | Requires actual `zfs send`/`receive` path and alternate pool. |
| AT-09 | Explicit zvol coverage | Not run | Not run | UI/model treatment alone is insufficient. |
| AT-10 | Exact selected recovery point | Simulated | Not run | Namespace model may be tested; engine and snapshot-path proof remains. |
| AT-11 | Object larger than one cartridge | Simulated | Not run | Requires file and native-stream physical spanning. |
| AT-12 | Cancellation and host/process failure | Implemented/Simulated | Not run | Expired motion leases become blocked and revoke reconciliation without retry; controlled physical interruption and engine evidence remain. |
| AT-13 | Unknown media/device loss/position | Implemented/Simulated | Not run | Missing/path-changed SAS devices, stale identity, and unverifiable motion postconditions are blocked. Kernel-position loss remains a data-I/O blocker; Linux-supported rewind/offline recovery requires an affirmative BOT/no-media postcondition. Physical reset/disconnect evidence remains. |
| AT-14 | Compression modes restore | Not run | Not run | Each supported resolved pipeline requires byte-exact restore evidence. |
| AT-15 | Corrupt/reordered/truncated content | Simulated | Not run | Record/authentication fault injection must be tied to selected engine. |
| AT-16 | Media read-back and restore tests | Not run | Not run | Cached or source rereads do not count. |
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
| AT-27 | Source errors/existing destinations | Simulated | Not run | Requires actual engine/source error and metadata coverage. |
| AT-28 | Durable events/actionable status | Implemented/Simulated | Not run | Drive-motion intents, claims, results, and audit evidence are durable; broader end-to-end restart and delivery-channel evidence is pending. |
| AT-29 | Rust boundary/Linux dependency audit | Implemented (source boundary) | Not run | Rust custom binaries exist; runtime/SBOM/dependency audit remains before release. |
| AT-30 | Safe external-tool execution | Implemented/Simulated | Not run | `mt-st` rewind/unload use a closed argv mapping, bounded output, and a process deadline; a stuck kernel tape ioctl may outlive that deadline, and selected engine/physical interruption evidence remains. |
| AT-31 | Product-specific visual/workflow review | Implemented | Not run | Product-owner desktop/phone review must be recorded. |

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

Current status: **Blocked by engine selection and physical laboratory evidence**.

### G1 — Recovery safety

Required exit evidence: file/native/incremental round trips, physical EOM, wrong/unknown media protection, dependency retention, and controlled interruption results.

Current status: **Not run**.

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
