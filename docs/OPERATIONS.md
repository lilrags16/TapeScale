# Operator guide

This guide describes the intended TapeScale operating model. The current release is an engineering preview with read-only SAS discovery and a simulator-only engine adapter. It must not be represented as protecting real tape until G0/G1 qualification is complete.

Procedures below are the operator acceptance target. The preview can exercise console, policy, scheduler, custody, restore-planning, and failure states with simulated data, but it cannot perform or verify a physical backup, restore, movement, encryption recovery, or catalog-loss recovery. Controls that lack a qualified backend are demonstrations, not operational authorization.

## Daily overview

Start on the operational overview and resolve items in this order:

1. unsafe or uncertain device/media state;
2. failed or interrupted restores and writes;
3. waiting-for-media requests and blocked verification;
4. overdue custody movements and off-site coverage gaps;
5. expired backup freshness, key recovery, or retained-media audit targets;
6. forecast media, spool, staging, snapshot, and destination capacity shortages.

The latest successful backup, latest verified recovery point, and latest complete verified off-site source-data time are different values. Do not substitute one for another.

## Configure a protection policy

Choose explicit source datasets, descendant selection, exclusions, and representation:

- File: selected files/tree and metadata with exact namespace tracking.
- Native ZFS: dataset/zvol full and incremental streams with properties and dependencies.
- Both: two independent representations and their additional time/media demand.

Review the stable logical root, consistency boundary, application hooks, compression/encryption pipeline, verification requirement, schedule timezone, retention, media pool, archive setting, rotation plan, and predicted tape/staging demand.

Defaults are weekly full Saturday 02:00, daily incrementals Sunday–Friday 02:00, at least 28 daily recovery points, full read-back before ordinary export, encryption enabled, and weekly media grouping. Bases and intermediate increments remain beyond 28 days when retained dependents require them.

Scope, exclusion, format, or encryption-lineage changes may require a new full. The preview must show that result before saving.

## Run and monitor a backup

Each run begins from immutable selected snapshots and records exact coverage. A run can be queued, preparing, waiting for media, writing, written, verifying, verified, blocked/interrupted, failed, or canceled.

- `waiting for media` retains the required snapshot/holds and identifies compatible tape IDs or operator action.
- `written` is not export-ready when full read-back is required.
- `verified` records the exact read-back scope; it is not a restore test.
- Unexpected unreadable paths, enumeration failure, missing keys, unsupported mandatory metadata, producer failure, or integrity error prevents a clean result.

Never satisfy a media request with a tape chosen only by matching its paper label. Confirm the application identity and wait for TapeScale to reconcile the on-tape identity.

## Manual tape handling

Before handling media, verify the intended drive under **Drives & libraries**. A newly discovered drive is untrusted. Compare its physical model, serial/WWN, firmware, and non-rewinding device path, then select **Trust identity**. TapeScale revokes that reconciliation when the path, identity, firmware, or access state changes. This setup action authorizes observation only—not tape motion or data writes.

For a standalone drive:

1. Read the requested tape IDs, generation/format, pool, and reason.
2. Inspect the physical label and load one candidate.
3. Allow TapeScale/engine to read identity before any write.
4. If the identity is wrong or unknown, unload it unchanged and keep the request open.
5. Confirm removal only after the drive reports unloaded.

For a library, use the inventory and slot view. Changer motion, drive ownership, and import/export operations remain serialized. An export-slot move is not off-site delivery.

After a device reset, disconnect, manual removal, or unexpected slot change, stop and reconcile. Never continue an active write from an assumed position.

## Verification

Verification types are distinct:

- Full read-back: reads recorded data from actual tapes through decoding and validates identities, order, lengths, authentication, decompression, and expected file/native bytes.
- Sample verification: reads only the recorded sample and is labeled accordingly.
- Restore exercise: restores declared content and metadata to an isolated destination.
- Drive read-after-write: device protection only, not application recovery evidence.

Interrupted verification cannot pass. New data defaults to full read-back before ordinary off-site eligibility. Schedule monthly representative file restores, quarterly native-chain exercises, quarterly clean-install catalog-loss exercises, and full qualifying retained-media read-back at least every 12 months, subject to policy and coverage checks.

## Plan and execute a restore

Use the restore planner before loading media. It must show:

- selected recovery point and scope;
- all required tapes, order, and confirmed locations;
- full/incremental dependencies and compatible drives;
- key and original-dataset-key requirements;
- verification/restore-test evidence;
- estimated read and staging work;
- destination mapping, ACL/identity limitations, and conflicts.

Default to a new directory or dataset. For an existing directory, skip conflicts by default; rename or replace must be explicit. Native receives go beneath an isolated approved dataset, with mountpoints rewritten and recovered shares disabled.

File restore validation compares SHA-256 content and promised metadata. Native validation covers the full send/receive and declared dataset state. Per-file claims for native recovery require an independently recorded file scan. A zvol is a block volume.

Inspect the result before publishing or cutting over. Replacement of live data is an administrator destructive workflow.

## Off-site rotation

Movement tasks describe tape IDs, reason, source, destination, readiness, due time, dependencies, and assigned operator. A weekly group becomes export-ready only after finalized writes, committed recovery metadata, required verification, independent key-package recovery readiness, and absence of active writes.

Use the packing checklist, then record physical departure and arrival or a single confirmed move according to site policy. Scheduled dates, notifications, ejection, and export-slot placement do not change custody.

Partial delivery remains incomplete. Minimum coverage is evaluated using every tape and dependency of a complete verified recovery point at the required location. Routine recall waits for verified replacement coverage. Emergency recall requires an explicit recorded exception showing the temporary gap.

Corrections append custody history. Do not edit a prior location into a value that was never confirmed.

## Retention, holds, and reuse

View retention on recovery points and dependency chains, not only tape dates. A cartridge is protected if any contained recovery point, base, increment, manifest, explicit hold, archive, incomplete recoverable record, export reservation, or active job needs it.

Archives are self-contained full captures, retained indefinitely, with two independently verified copies by default and at least one off site. Archives do not delete source data and use separate media by default.

Automatic reuse requires an expired dependency graph, no holds, correct physical and catalog identity, compatible actual format, healthy reconciled state, and policy authority. Exceptional destruction requires administrator reauthentication and an affected-recovery-point preview. Returning a tape from off site does not make it reusable.

## Scheduling behavior

The standalone `tapescale-scheduler` evaluates these rules and persists occurrences/work in PostgreSQL. It has no tape devices, source paths, restore destinations, or engine control. The worker claims eligible work and performs privileged orchestration.

- Overlapping runs of the same job queue by default with at most one catch-up run.
- After downtime, TapeScale records missed occurrences and captures the current source once; it does not fabricate historical snapshots.
- A due/missed full outranks coalesced incrementals. A failed full does not become a base.
- Requested restores outrank queued backups at safe boundaries but never interrupt an active write solely for priority.
- Timezone schedules run a repeated local occurrence once and move nonexistent local time to the next valid instant.
- Manual media waits persist and remind after 24 hours; they do not release required state or select alternate media destructively.

## Alerts and corrective action

### Unknown or foreign tape

Leave it protected. Inspect read-only identity and catalog evidence. Importing a known foreign format, labeling, erasing, and reuse are separate administrator decisions.

### Waiting for media

Read the requested compatibility, tape IDs, location, and dependency reason. Locate and confirm eligible media; never substitute retained/archive media because it is physically nearby.

### Source read or metadata failure

Fix access/source health and start a recorded retry from an immutable valid snapshot where possible. Do not mark an incomplete manifest successful.

### Verification failure

Quarantine the affected media for investigation, map impacted recovery points, preserve valid copies, and create replacement work from a valid source or independent copy. Checksums detect damage; they do not repair missing bytes.

### Device loss or uncertain position

Stop the controller, preserve logs, check cabling/driver/firmware, then perform inventory and identity reconciliation. Do not manually advance the engine catalog.

### Full spool, staging, or destination

Preserve the free-space reserve. Let backpressure reach a documented safe boundary or fail explicitly. Never delete the only retry state or disable verification to finish within a window.

### Missing key

Identify the required lineage and recover it through the independent key process. Do not expose or export ciphertext as a successful restore.

## Catalog and configuration protection

Create consistent PostgreSQL/configuration exports at least daily and after protected-job finalization, key changes, and relevant configuration changes. Keep the recovery bundle and key package separately. Test the clean-host path in [RECOVERY.md](RECOVERY.md) quarterly.

## Upgrades

Drain new work and stop active operations only at safe engine boundaries. Take and verify pre-migration catalog/configuration backups and a recovery bundle. Record versions, apply migrations, reconcile all hardware/media state, and run preflight plus a historical-format restore fixture before resuming.

## Diagnostic exports

Diagnostic bundles include component versions, sanitized health, correlation IDs, state transitions, and configured evidence. Review the redaction preview. Never include credentials, cookies, recovery codes, usable encryption keys, authorization headers, or an unrestricted sensitive path inventory.

## Escalation rule

When the UI and physical reality disagree, preserve the media and recoverable state, stop destructive work, and escalate with the correlation ID and evidence. Direct engine-console changes can invalidate reconciliation and are not a supported shortcut.
