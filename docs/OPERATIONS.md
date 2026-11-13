# Operator guide

This guide describes the intended TapeScale operating model. The current release is an engineering preview with SAS discovery, guided tape initialization, bounded graphical source/retained-manifest browsing, lab-only rewind/unload, a destructive Bacula `btape test` harness, an attended operational full-backup path, and a G0.2 Bacula restore-comparison exercise. It must not be represented as generally protecting production tape until G0/G1 qualification is complete.

Procedures below distinguish the implemented preview from the broader operator acceptance target. Backup Center can queue manual or weekly Saturday 02:00 full-file requests, wait durably for an operator to assign one exact qualified drive and scratch cartridge, write through Bacula, and physically read the entire backup before retaining a recovery point. Tape selection and destructive authorization remain attended. The G0.2 path additionally restores one capture and compares independent manifests. General-purpose restore execution, encryption recovery, catalog-loss recovery, spanning, incrementals, native ZFS, and production authorization remain incomplete or unqualified.

## Daily overview

Start on the operational overview and resolve items in this order:

1. unsafe or uncertain device/media state;
2. failed or interrupted restores and writes;
3. waiting-for-media requests and blocked verification;
4. overdue custody movements and off-site coverage gaps;
5. expired backup freshness, key recovery, or retained-media audit targets;
6. forecast media, spool, staging, snapshot, and destination capacity shortages.

The latest successful backup, latest verified recovery point, and latest complete verified off-site source-data time are different values. Do not substitute one for another.

## Configure a backup plan

In **Backup Center**, create an explicit file-mode plan for a relative path beneath the configured read-only `/source` mount. Use **Browse** to inspect one bounded directory level at a time. It displays folders plus recorded file size/mtime, never file contents; symlinks and special objects are non-navigable, and only a non-root directory can become the recursive backup scope. Prefer an immutable `.zfs/snapshot/<name>` directory. Manual relative-path entry remains available.

The browser is selection assistance, not safety authority. Plan submission independently resolves and contains the path, rejects traversal and symlink escape, verifies that the container mount is read-only, and records the operator's separate immutable-snapshot attestation. That attestation does not create or hold a TrueNAS snapshot; prepare and retain the snapshot outside TapeScale before each run.

The executable preview contract is deliberately fixed:

- representation: file;
- level: full;
- volumes: exactly one;
- encryption: disabled;
- verification: full physical readback;
- retention: 1 to 36,500 days, default 28;
- schedule: manual only, or Saturday at 02:00 in the selected IANA timezone;
- media: optional preferred scratch pool, with exact attended assignment for every request.

Native ZFS, dual representation, incrementals, alternate cron expressions, multi-volume spanning, and encryption fail validation. The weekly scheduler persists an idempotent occurrence and coalesces missed weeks to the latest due full; it does not choose hardware, load media, or grant overwrite authority.

## Run and monitor a backup

Choosing **Run now** or reaching a due weekly occurrence creates a durable job and backup request in `waiting_for_media`. No source read or tape write occurs until an operator assigns exact media. A run then progresses through queued/preparing, source inventory, writing, written, verifying, and verified, or to blocked/interrupted/failed/cancelled with retained evidence.

- `waiting for media` survives restart and requires operator action; TapeScale does not create a TrueNAS hold, so the operator must keep the attested snapshot immutable and available.
- `written` is not export-ready when full read-back is required.
- `verified` records the exact full-media read-back, manifest, catalog binding, volume record, and recovery point; it is not a restore test.
- Unexpected unreadable paths, enumeration failure, missing keys, unsupported mandatory metadata, producer failure, or integrity error prevents a clean result.

Never satisfy a request from a paper label alone. The selected inventory record must be unprotected, non-WORM, healthy, compatible with the exact reconciled drive, in the plan's pool when one is configured, and free of holds, content, conflicting identity, active reservation, or pending unsafe custody. The exact typed phrase binds the request ID and cartridge label before assignment.

## Manual tape handling

Before handling media, verify the intended drive under **Drives & libraries**. A newly discovered drive is untrusted. Compare its physical model, serial/WWN, firmware, and non-rewinding device path, then select **Trust identity**. TapeScale revokes that reconciliation when the path, identity, firmware, or access state changes. This setup action binds the observed identity; it does not by itself authorize motion or data writes.

### Lab-only rewind and unload

The disposable test Compose enables `TAPESCALE_LAB_TAPE_MOTION_ENABLED=true`. With a current reconciled drive and a reported loaded medium, **Rewind** and **Unload** create durable worker requests. Confirm that the loaded cartridge is designated scratch media, acknowledge the prior-write warning, then wait for the request to leave `queued`/`running` and verify the resulting drive status. Unload uses the device's rewind-and-offline operation; it puts the device offline and may physically eject the cartridge.

Linux may write a filemark before rewind/offline when the previous tape operation was a write. TapeScale exposes no data-write or explicit filemark command here, but it cannot promise unchanged media after another controller wrote to the drive. Never use retained, production, unknown, or foreign media with this unqualified lab control.

TapeScale leases one drive controller, rechecks its exact stable identity and safety state, executes only the fixed allowlisted command, and appends audit evidence. A missing/stale drive, identity change, conflict, TapeScale-uncertain prior motion result, absent medium, or existing active request blocks execution. The [Linux SCSI tape driver explicitly permits rewind/offline as recovery operations](https://github.com/torvalds/linux/blob/master/drivers/scsi/st.c#L3771-L3791) when its internal tape position was lost after reset; TapeScale still requires a ready scratch cartridge and verifies BOT after rewind or no-media after unload. Do not run another tape controller against the same drive during this lab workflow.

### Guided tape initialization and destructive Bacula qualification

The disposable TrueNAS Compose also enables `TAPESCALE_LAB_TAPE_QUALIFICATION_ENABLED=true` and packages Debian's pinned Bacula Community 15.0.3 tools. This is an attended G0.1 harness, not a backup job.

Choose **Initialize tape** from **Drives & libraries** or **Tapes & pools**. The wizard composes the existing durable operations instead of inventing a weaker shortcut:

1. Physically load a blank or expendable cartridge and wait for the drive to report ready. When exactly one current safe drive exists it can be preselected; otherwise select the exact drive explicitly.
2. Review its stable identity, non-rewinding path, serial/WWN, and firmware. If it is not yet reconciled, explicitly allow the wizard to perform the same **Trust identity** API check. Stale, inaccessible, conflicting, uncertain-position, immediate-mode, or busy drives remain blocked.
3. Register the cartridge's printed label and generation inline, or select its existing inventory record. Barcode is optional and WORM is never offered as an initialization candidate. WORM media, cartridges with content/holds, foreign or mismatched identities, and reserved media are ineligible.
4. Physically confirm that the displayed label is the cartridge loaded in that standalone drive. The drive cannot discover the printed inventory label, so the operator assertion remains mandatory.
5. Accept both destructive acknowledgements and type `DESTROY <label> WITH BACULA BTAPE TEST` exactly. No tape command is issued before this final step.
6. Keep the host, container, drive, and cartridge available while the durable run is queued/running. The wizard can be closed; progress and evidence remain under **System & qualification → Qualification**. A pass requires Bacula exit success plus TapeScale's cleanup rewind and affirmative same-drive ready-at-BOT observation.

Registration, drive reconciliation, and qualification remain separate audited operations. The browser retains a random request key before qualification submission, and PostgreSQL atomically binds that key to the exact semantic request. Replaying the same key returns the original run—even after it becomes terminal—while reuse with a different drive or cartridge is rejected. A response-loss retry therefore reloads the exact existing media record and durable qualification instead of dispatching another destructive test. Failure never advances automatically into a backup; inspect the retained result and reconcile uncertain hardware before deliberately starting a new attempt.

The API reserves the selected cartridge and creates append-only events before the worker can claim the run. The worker rechecks path, stable identity, model, serial/WWN, firmware, device access, kernel immediate mode, outstanding I/O, loaded/ready state, reservation, and the two confirmations. It invokes only `btape -c <per-run-config> /dev/nstN` with a fixed `test\n` input transcript. A timeout, cancellation, worker loss, lease expiry, Bacula failure, identity change, or missing postcondition records a failed/interrupted result and revokes drive reconciliation. The captured output is deliberately bounded and reports truncation.

`btape` overwrites media. A successful result demonstrates basic Bacula record/filemark/position/append/re-read behavior for the recorded tuple; it does not prove file backup, full readback, restore, metadata fidelity, spanning, encryption, catalog recovery, or G0 completion. The harness does not expose Bacula `fill`, label, erase, a console, or arbitrary command execution.

### Operational attended full backup

Use **Backup Center** only after the exact loaded drive/cartridge combination has a passed G0.1 `btape` result. Every assignment is destructive to the selected scratch cartridge.

1. Create a backup plan for an existing immutable snapshot below the read-only source mount. Choose retention, an optional scratch pool, and either manual operation or the supported Saturday 02:00 schedule.
2. Choose **Run now** or allow the scheduler to create the occurrence. Confirm the request is **Waiting for media**; this is durable demand, not evidence that the source was captured.
3. Physically load one eligible scratch cartridge into the exact trusted and qualified drive. Check its printed identity against the inventory record and ensure no other controller uses the drive.
4. Choose **Assign media**. Type the cartridge label, confirm that it is physically loaded, authorize complete scratch overwrite, accept the unencrypted test-gate deviation, and type `OVERWRITE <cartridge label> FOR BACKUP <request UUID>` exactly.
5. Monitor the durable phases and append-only events. The worker re-probes identity and eligibility, reserves and fences the drive/cartridge, inventories and hashes the source, commits the destructive boundary, prepares BOT, labels the assigned volume, submits one Bacula Full job, and correlates its catalog result.
6. Keep all resources available while Bacula performs `Level=Data` verification. This rereads the physical tape rather than the mounted source. TapeScale rescans the source afterward and refuses success if the supposedly immutable input changed.
7. Treat the run as protected only after **Verified** and inspect the retained source manifest, Bacula JobId, volume, full-readback verification, and recovery-point UUID. Run an attended Recovery Vault rehearsal later for independent restore evidence.

A waiting request can be cancelled immediately with a retained reason. Once assigned, cancellation is cooperative: the worker observes the durable request at safe scan/engine boundaries, asks Bacula to cancel when applicable, performs required cleanup, and records the honest terminal result. TapeScale cannot guarantee immediate interruption of a command blocked in kernel tape I/O. A cancelled, failed, interrupted, source-changed, or cleanup-uncertain run creates no recovery point and may leave the cartridge quarantined or write-unverified.

The operational path does not perform the immediate isolated restore and manifest comparison used by G0.2. Its two separate verification records mean only that the write completed and the full physical readback completed. Do not describe that result as a restore test.

### First recoverable tape (G0.2)

Use **Recovery lab** only after the exact loaded drive/cartridge combination has a passed G0.1 `btape` run. This attended exercise deliberately overwrites that cartridge again.

1. Create an immutable TrueNAS snapshot below the read-only host source mount. In the test Compose, enter its path relative to `/mnt`, such as `pool/dataset/.zfs/snapshot/tapescale-g02`.
2. Ensure `/mnt/tapescale-restores` is a dedicated isolated dataset with a Generic/POSIX-compatible ACL preset and no shares. An SMB/NFSv4 `aclmode=restricted` destination can reject required metadata restoration. Choose a relative restore subpath that is not used by another run.
3. Create a recovery profile. TapeScale resolves the selected source beneath `/source`, confirms its container mount is read-only, and records an operator assertion that it is an immutable snapshot.
4. Select the same trusted drive and registered scratch cartridge. Confirm scratch designation, destructive overwrite, and the G0.2 encryption-disabled deviation.
5. Type `WRITE <profile name> TO <cartridge label> AND RESTORE VERIFY` exactly. Keep the host, database, mounts, worker, drive, and tape available until the run reaches a terminal state.
6. Follow the phase timeline: queued, preparing, writing, written, tape verification, restore, manifest comparison, and terminal result. Do not infer progress from drive motion alone.
7. Review the retained source manifest, Bacula job evidence, restored manifest, comparison, and evidence report. A recovery point is created only after strict Bacula job success and an exact independent comparison.

Qualification, an interrupted earlier attempt, or another scratch workflow can leave records or a valid label at BOT. G0.2 therefore cannot assume that the Director's normal `label` command is starting from blank media; Bacula correctly refuses to overwrite a valid existing label. After the durable write boundary and all three destructive confirmations are committed, the worker rewinds the exact fenced scratch drive, issues one fixed internal `mt-st -f /dev/nstN weof 1` command to invalidate prior BOT contents, records that action, and rewinds again before starting Bacula. This is not exposed as a general motion control. Failure or interruption quarantines the cartridge as write-unverified and prevents a recovery point.

The source manifest records per-file SHA-256 and the explicitly covered namespace/metadata fields. This G0.2 harness accepts at most 100,000 manifest entries and 100 GiB logical data so the in-memory comparison remains bounded; larger fixtures are blocked before a tape write. Bacula Data verification rereads the physical volume; it is not satisfied by rereading the mounted source. The subsequent normal Bacula restore is written beneath `/restore/<profile restore subpath>/<run id>` and remains isolated for inspection. TapeScale then scans that restored tree independently. Any missing/extra path, content or covered-metadata difference, warning/error terminal state, lease loss, identity change, source mutability, or uncertain cleanup prevents a pass.

This pass is deliberately narrow. It establishes recoverability only for the exact recorded run and does not qualify NFSv4 ACLs, encryption, incrementals, multiple volumes, EOM/wrong-media behavior, changers, native ZFS streams, interruption recovery, or clean-host catalog reconstruction. Do not move the cartridge off site or reuse it under a general policy on the strength of this exercise.

The rewind/unload motion gate does not expose load, seek, spacing, retension, an explicit filemark, read, write, label, format, or erase command. A successful rewind/unload does not identify the cartridge or prove restoration. The separate `btape` harness performs only its fixed destructive test and creates no recovery point; only the fenced operational-backup and G0.2 workflows may invoke the fixed Bacula data path described above.

For a standalone drive:

1. Read the requested tape IDs, generation/format, pool, and reason.
2. Inspect the physical label and load one candidate.
3. Allow TapeScale/engine to read identity before any write.
4. If the identity is wrong or unknown, unload it unchanged and keep the request open.
5. Confirm removal only after the drive reports unloaded.

For a library, use the inventory and slot view. Changer motion, drive ownership, and import/export operations remain serialized. An export-slot move is not off-site delivery.

After a device reset, disconnect, manual removal, or unexpected slot change, never continue an active write from an assumed position. Rewind/unload may recover an endpoint only in this scratch-media lab path; all future data I/O must still reconcile the engine position and media state first.

## Verification

Verification types are distinct:

- Full read-back: reads recorded data from actual tapes through decoding and validates identities, order, lengths, authentication, decompression, and expected file/native bytes.
- Sample verification: reads only the recorded sample and is labeled accordingly.
- Restore exercise: restores declared content and metadata to an isolated destination.
- Drive read-after-write: device protection only, not application recovery evidence.

Interrupted verification cannot pass. New data defaults to full read-back before ordinary off-site eligibility. Schedule monthly representative file restores, quarterly native-chain exercises, quarterly clean-install catalog-loss exercises, and full qualifying retained-media read-back at least every 12 months, subject to policy and coverage checks.

## Plan and execute a restore

Open **Restore → Recovery vault**, search by source, tape, representation, status, or date, and select a point. The dossier joins the durable recovery contract to ordered volume records, confirmed locations, physical readback, isolated restore-test evidence, origin events, retained artifact digests, and later rehearsal runs. Evidence downloads are bounded and omit internal artifact storage paths.

For file-mode points with an integrity-checked source manifest, choose **Browse files** to navigate the recorded directory tree. The API reads the retained manifest through a read-only artifact mount and returns only bounded names and recorded type/size/mtime metadata—never file contents or the worker artifact path. This is catalog inspection, not tape I/O. The current attended rehearsal still restores and compares the complete retained job; selecting one displayed file or directory for execution is not implemented.

Select **Plan restore**, choose a contained relative destination and conflict policy, and declare the destination/key/capacity facts that are actually known. TapeScale persists the preview for 30 minutes and reports required tapes, dependency order, exact compatible qualified drives, locations, keys, warnings, and blockers. An unverified point, unknown tape location, missing exact hardware qualification, unsafe destination, insufficient declared capacity, or unavailable key prevents a ready result. Planning alone never moves media, invokes Bacula, or creates a destination.

A ready plan exposes **Run attended recovery rehearsal** only when it resolves to a passed G0.2 `file/full` point with exactly one finalized unencrypted volume, no recovery dependencies or key lineage, the pinned Bacula 15.0.3 adapter/format, a retained source manifest, an intact original backup JobId/catalog, and a current exact qualified drive/cartridge tuple. Every broader plan remains planning-only.

Before queueing the rehearsal, physically retrieve the displayed tape, load it in the selected exact drive, confirm its printed/internal identity against the plan, and keep the worker and database available. Queueing submits the plan UUID, unchanged preview hash, selected drive UUID, and attended confirmation. The worker rechecks every durable prerequisite, fences the drive and cartridge, re-probes the SAS identity, and asks Bacula to read only the retained backup JobId. It never labels, clears, erases, backs up, or otherwise writes the cartridge.

The destination is always a newly created `/restore/<planned relative path>/<rehearsal UUID>` directory. Replace, rename, merge, live cutover, and automatic share publication are unavailable. TapeScale verifies the retained source-manifest artifact before dispatch, performs the normal physical-media restore, scans the recovered tree independently, and requires an exact comparison of every promised path, SHA-256 content value, and covered metadata field. A pass appends another `restore_test` verification record; it does not create a recovery point or expand hardware qualification.

Queued and running rehearsals are durable. The Recovery Vault reports exact media/drive/destination, phase events, engine correlation, comparison, result, cancellation, and bounded evidence. A running cancellation asks Bacula to cancel and waits for a terminal observation. Lease loss, worker restart during physical work, timeout, identity change, catalog disagreement, wrong/missing media, manifest mismatch, or uncertain cleanup cannot pass and may require drive reconciliation.

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

The current manual-custody workflow supports named physical locations and exact take-off-site, return, swap, and recall tasks. A newly registered cartridge with no location can be established at its first endpoint only through a direct physical confirmation. A two-step move requires a confirmed source plus an explicit in-transit location.

Each task reserves its exact cartridge set. Operators may confirm a subset, so a multi-tape delivery remains incomplete until every tape reaches the destination. Departure, arrival, direct confirmation, cancellation, and custody correction use optimistic versions and idempotency keys; retrying the same request cannot silently complete it twice. Cartridge location and reservation updates, append-only custody history, audit events, and in-app outbox records commit together. Missed work is durably marked overdue and remains overdue through partial progress.

Movement tasks describe tape IDs, reason, source, destination, readiness, due time, dependencies, and assigned operator. A weekly group becomes export-ready only after finalized writes, committed recovery metadata, required verification, independent key-package recovery readiness, and absence of active writes.

Use the packing checklist, then record physical departure and arrival or a single confirmed move according to site policy. Scheduled dates, notifications, ejection, and export-slot placement do not change custody.

Partial delivery remains incomplete. Minimum coverage is evaluated using every tape and dependency of a complete verified recovery point at the required location. Routine recall waits for verified replacement coverage. Emergency recall requires an explicit recorded exception showing the temporary gap.

Corrections append custody history. Do not edit a prior location into a value that was never confirmed.

## Retention, holds, and reuse

Open **Tapes & pools** to work from the durable cartridge inventory rather than a handwritten label. Search and filter inventory by pool, confirmed location, identity, health, protection, and attention state. The cartridge dossier joins the registered identity to on-tape identity, pool, location, reservation, volume coordinates, recovery points, dependencies, expiry, verification, holds, custody, and audit history. It does not disclose worker artifact paths.

View retention on recovery points and dependency chains, not only tape dates. A cartridge is protected if any contained recovery point, base, increment, manifest, explicit hold, archive, incomplete recoverable record, export reservation, pending custody task, or active job needs it. A cartridge may appear in more than one recovery dependency chain; every affected point must be cleared before reuse can be considered.

Archives are self-contained full captures, retained indefinitely, with two independently verified copies by default and at least one off site. Archives do not delete source data and use separate media by default.

Create routine, archive, scratch, or cleaning pools deliberately. Pool assignment is an audited cartridge mutation. The browser submits the dossier's current `state_version`; if the cartridge changes first, refresh and review the new facts instead of forcing the assignment. Repeating the same idempotent request returns its original receipt. Never move protected archive content into a reusable pool or treat a pool name as proof that physical media is blank.

Manual, legal, and archive holds can be added from the dossier. Releasing one of these operator-created holds requires a reason and appends actor, correlation, and release evidence; it does not delete the hold record. Dependency, active-job, and unknown-identity protection is derived from system state and is not released through this control. A stale cartridge version blocks either mutation.

Select a currently observed drive to evaluate reuse. The evaluation is read-only and denies on uncertainty. It reports identity/catalog reconciliation, retained and dependent recovery points, holds and archives, reservations and custody, WORM state, health and location certainty, actual format/generation compatibility, and affected recovery points. Correct the listed state and evaluate again; do not substitute a different drive merely to hide a protection blocker.

Returning a tape from off site does not make it reusable. This release has no general erase, relabel, recycle, overwrite-execution, or policy-override control. Even when the read-only evaluation says `allowed`, an operator has not authorized destruction and TapeScale dispatches no tape command. The production target's exceptional-destruction workflow still requires administrator reauthentication and an unchanged affected-object preview, but that workflow is not implemented in this preview.

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
