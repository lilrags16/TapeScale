# TapeScale architecture

Status: alpha release architecture; production qualification remains incomplete
Specification baseline: TrueNAS LTO Backup System requirements v1.1, 11 November 2026

TapeScale is a control plane for protecting TrueNAS data on LTO media. It deliberately does not invent a tape-record format. Product policy and operator workflows belong to TapeScale; physical records, volume spanning, tape positioning, and the engine catalog belong to a qualified tape engine.

No general physical tape, changer, ACL, or disaster-recovery qualification is implied by this repository. A passed G0.2 run is evidence only for its exact recorded tuple. See [QUALIFICATION.md](QUALIFICATION.md).

## Runtime boundaries

```mermaid
flowchart LR
    Browser[Browser] -->|HTTPS / JSON| API[Web and API service]
    API --> DB[(PostgreSQL)]
    Scheduler[Scheduler and orchestrator] --> DB
    Worker[Privileged worker] --> DB
    Worker -->|versioned adapter| Engine[Tape engine]
    Worker -->|authenticated WSS JSON-RPC| TrueNAS[TrueNAS API]
    Worker -->|bounded argv execution| ZFS[OpenZFS tools or constrained host helper]
    Worker --> Tape[Tape drives and changers]
    Engine --> EngineDB[(Engine catalog)]
```

The architecture separates three custom Rust runtime responsibilities. The alpha Compose deployment has `api`, `scheduler`, `worker`, and `postgres` services. PostgreSQL is the handoff boundary; no service relies on an in-memory queue for durable work:

- The target web/API service authenticates users, enforces request-level authorization, creates audited intents, and serves the compiled browser application. The alpha has no application sign-in yet. It has no tape devices, writable host path, or Docker socket. The alpha stack gives it read-only source/restore views, a read-only view of retained recovery-manifest artifacts, and only `DAC_READ_SEARCH` for path containment, metadata browsing, and mount-mode preflight; a future qualified design moves filesystem proof to the fenced worker.
- The scheduler evaluates timezone-aware schedules, downtime/catch-up policy, retention graphs, recovery dependencies, resource availability, and movement plans. It creates durable work in PostgreSQL and has no tape devices or source/restore mounts.
- The worker performs privileged orchestration and is the only component allowed to touch approved source mounts, restore destinations, tape/changer devices, ZFS integration, or the engine control interface. Worker authorization is checked again at execution time.
- The recovery CLI is a separate Rust binary intended to inspect recovery bundles, identify media, rebuild/import catalog information through supported interfaces, and initiate recovery without the original TapeScale database.

The current binaries are `tapescale-api` on port 8080, `tapescale-scheduler`, `tapescale-worker`, and `tapescale-recovery`. PostgreSQL 16 is the durable product store and coordination boundary. The attended operational backup path now uses the same pinned physical Bacula adapter as G0.2, while production qualification remains gated. Explicit lab flags enable durable rewind/unload, the Bacula Community 15.0.3 `btape test`, an operational full-file backup that can span an ordered set of attended tapes, a separate one-volume G0.2 restore-comparison exercise, and later attended read-only ordered-volume rehearsals. An operational backup creates a recovery point only after exact ordered physical Data readback and immutable-source recheck; it does not claim `restore_test`. G0.2 creates its recovery point only after physical readback plus an independently compared isolated restore.

Browser JavaScript or TypeScript and build tooling are permitted. Runtime API, scheduling, policy, workers, adapters, helpers, and recovery logic must remain Rust. PostgreSQL, OpenZFS, the selected tape engine, and qualified tape utilities are existing Linux components rather than custom TapeScale backend code.

## Authority and catalog ownership

TapeScale is authoritative for:

- users, roles, sessions, MFA, and reauthentication;
- TrueNAS connections, source selections, and logical source identity;
- policies, schedules, retention, holds, and encryption lineage;
- user-visible jobs, attempts, state transitions, and notifications;
- restore plans, approved destinations, and destructive-action previews;
- locations, planned movements, confirmed custody, and correction history;
- cross-system stable identifiers and reconciliation state.

The selected engine is authoritative for:

- its physical backup records and on-tape format;
- record checksums, volume spanning, spooling, and proven restart boundaries;
- tape position and end-of-media handling;
- its catalog records exposed through supported interfaces.

TapeScale never edits undocumented engine tables. Stable identifiers join records across catalogs. If physical observation, TapeScale state, and engine state disagree, the media is blocked and a reconciliation workflow is created.

## Domain model

The durable model is grouped by responsibility:

- Identity: users, roles, sessions, MFA factors, recovery codes, and reauthentication challenges.
- Sources: TrueNAS systems, datasets and zvols, source selections, exclusions, snapshots, snapshot holds, consistency hooks, and configuration exports.
- Policy: protection policies, schedules, retention rules, verification rules, archives, key lineages, and approved restore destinations.
- Orchestration: job definitions, runs, attempts, transitions, leases, resource requests, media requests, and cancellation intents.
- Recovery: recovery points, backup records, dependency edges, volume records, manifests, file metadata and digests, native streams, verification runs, restore plans, and restore results.
- Hardware: drives, changers, slots, stable device identities, capability observations, reconciliation state, and durable non-data motion requests.
- Media: cartridges, formats, pools, reservations, locations, custody events, holds, and reuse evaluations.
- Operations: rotation plans, movement tasks, confirmations, notifications, audit events, catalog exports, recovery bundles, and component-version evidence.

All operational records use stable identifiers and record actor, time, correlation ID, and structured evidence. Corrections append evidence; they do not erase prior custody or audit history.

## Job lifecycle

The externally visible states are intentionally truthful:

```text
requested -> waiting-for-media -> queued -> preparing -> writing -> written -> verifying -> verified
                               \-> blocked/interrupted
                               \-> failed/canceled
```

- `written` means every required record and finalization operation succeeded. It does not mean verified.
- `verified` means the recorded verification scope completed through the recovery decoding path from the actual media. It does not mean a restore drill passed.
- A failed or incomplete run cannot become an incremental base.
- A reset, disconnect, or uncertain tape position blocks continuation pending reconciliation.
- Cancellation preserves prior valid recovery points and cleans up only TapeScale-owned snapshots and holds when dependency rules permit.

The scheduler persists due occurrences and dispatchable work in PostgreSQL; the worker claims durable execution and device leases there. A drive has one controller at a time; changer actions are serialized. Lease expiry after a crash is not permission to resume writing: the new worker first reconciles physical position and engine state.

Lab drive motion follows the same separation: the API records an exact stable-identity-bound `rewind` or `unload` intent with a designated-scratch-media acknowledgement, and only the singleton worker can claim the drive lease and invoke an allowlisted `mt-st` argument vector. The worker rechecks current identity, path, firmware, access, kernel immediate-mode/outstanding-I/O state, loaded media, and TapeScale's prior-motion certainty before execution and records bounded output plus before/after evidence. Linux treats rewind/offline as recovery operations when the kernel lost its internal tape position; success still requires an affirmative BOT or no-media postcondition. No arbitrary command string crosses the API boundary. Other direct motion, explicit filemark, seek, spacing, retension, label, format, erase, or general data commands remain unavailable. Linux may nevertheless finalize another controller's prior write with a filemark before rewind/offline, which is why this unqualified path is scratch-only.

The G0.1 qualification path is a separate destructive intent. The operator selects a registered unprotected non-WORM cartridge, types its exact label into a closed confirmation phrase, and acknowledges both scratch designation and complete overwrite. PostgreSQL reserves that cartridge and fences the exact reconciled drive. The singleton worker re-probes the bound path and identity, writes a generated per-run Bacula Storage Daemon configuration, and starts only `/usr/sbin/btape -c <config> /dev/nstN` with the fixed `test\n` standard-input transcript. It does not start `bacula-sd` or accept a user-supplied command. On `btape` success the worker performs the separately allowlisted rewind and accepts the run only after the same identity is observed ready at BOT. Failure, timeout, cancellation, lease loss, or an unverifiable postcondition leaves the run failed/interrupted and revokes drive reconciliation. Events and bounded tool evidence are append-only. Even a pass leaves G0 open and creates no recovery point, verified backup, engine selection, or on-tape-format claim.

The graphical tape-initialization wizard is an orchestration surface over those existing boundaries, not a new privileged primitive. It can select one current safe discovered drive, invoke the same exact-identity reconciliation, create or reuse an unprotected inventory record, collect the physical loaded-label assertion, and finally submit the unchanged destructive qualification contract. The qualification request includes a random durable idempotency key whose server-side fingerprint binds the drive, stable identity, cartridge, label, acknowledgements, and pinned engine/test semantics. Concurrent or later matching replays return the original run; changed-payload key reuse conflicts. Each operation remains independently durable and revalidated; the UI cannot infer standalone-drive media identity, waive the typed phrase, dispatch `btape` directly, or turn a pass into backup authority beyond the recorded exact tuple.

The operational backup boundary adds durable plans and requests ahead of the same fenced data executor. A plan binds a server-contained named `.zfs/snapshot/<name>` relative path under `/source`, verifies the exact open directory is on a kernel-reported read-only mount, and records optional scratch pool, retention, compression mode, and whether media spanning is permitted. Supported policy modes resolve to automatic LZO, explicit balanced GZIP, LZO, or no Bacula compression. The console creates manual plans and rechecks the exact source before queueing. The API-level Saturday 02:00 scheduler remains implemented for fixed-source evaluation, but it is not presented as rolling protection because it does not create or select a newer snapshot. Starts first commit a `job_run`, `backup_request`, correlation identity, and append-only events in `waiting_for_media`. No scheduler path carries drive, cartridge, or overwrite authority. An operator must select one exact trusted drive and eligible scratch cartridge and type `OVERWRITE <label> FOR BACKUP <request UUID>`. Assignment atomically revalidates compatibility, current qualification, identity, content/holds/WORM/custody/reservation state, optional pool membership, and drive readiness before binding the original job identity to a physical run.

Source browsing is a metadata-only GET surface rooted beneath the configured `/source`. Requests accept normalized relative paths, hold the root and selected directory as file descriptors, resolve beneath them without following symlinks, and enumerate/stat entries relative to the held directory so a host-side rename cannot redirect the listing. They return one bounded directory level in deterministic folders-first order and expose relative names/type/size/mtime only. Oversized directories return a bounded prefix with `truncated=true`; that presentation bound does not constrain plan selection or operational inventory. The browser may synthesize the contained `.zfs` directory when OpenZFS hides it from ordinary enumeration. The operational-plan picker enables only directories within a named `.zfs/snapshot/<name>` scope; authoritative canonical containment and read-only checks still occur when the plan is submitted.

The worker inventories the immutable source through a fixed-memory external sort: bounded entry runs spill to private files, a merge produces the exact content-addressed manifest, and disk-backed hard-link grouping preserves the manifest contract without holding the source tree in memory. The operational path therefore has no TapeScale total-entry or logical-byte policy ceiling, while filesystem, artifact storage, engine, device, no-progress, and available-media constraints remain. Before committing the destructive boundary, the worker verifies the isolated Bacula spool is a real writable directory and measures its current filesystem capacity. Bacula automatically enables attribute spooling with data spooling and always places those files in the Storage Daemon working directory, so TapeScale places that working directory on the same measured filesystem. It reserves a conservative allowance derived from manifest size and entry count plus the configured free-space reserve before selecting the run-scoped data-buffer maximum. The generated Backup Job sets `Spool Data = yes`; the Storage Device applies equal total/per-job data-spool ceilings. Each job receives a private run-scoped directory beneath the measured spool root. Bacula despools and continues when the data buffer reaches its ceiling, so that ceiling bounds temporary data buffering rather than archive size.

Bacula data spooling is segmented fill-and-drain behavior, not a producer/consumer ring. Source traversal and compression fill a segment; Bacula then stops producing into that segment while the Storage Daemon drains it to tape, and production resumes afterward. The segment can protect tape motion during each drain when spool reads and the tape path are fast enough, but it cannot make a producer whose long-term rate is below the drive's minimum streaming rate sustain that drive. Increasing the ceiling reduces the number of transitions and lengthens each continuous drain, but also lengthens the initial fill and cannot solve a throughput deficit unless the complete archive is staged. The Device also sets a 32 GiB maximum logical tape-file size instead of Bacula's one-gigabyte default, reducing EOF/filemark stops while retaining periodic positioning records; this is not an archive or volume-size limit.

While a job is active, TapeScale combines exact JobId-scoped Storage Daemon counters and state with aggregate regular-file occupancy of the private spool directory and exact-drive Linux SCSI tape counters. It records starting, buffering, streaming, despooling, reading, waiting, idle, and stalled activity without exposing spool paths or filenames. Heartbeat receipt is not progress: the transfer watchdog resets only on measured engine, spool, or tape movement and cancels after the configured bounded stall interval. The worker then prepares and labels the exact first volume and executes one Bacula Full job. Only Bacula's `m` status (new volume required), not `M` (mount the current volume), may create the next destructive request. The worker releases storage, durably requests the next order, and pauses its no-progress watchdog until the operator authorizes an independently eligible cartridge. A continuation tape is cleared and labeled only after its order-specific destructive boundary. A standalone SAS drive does not report the physical cartridge's inventory label, so the operator must attest that the selected label is loaded; TapeScale revalidates the drive and inventory gates but does not claim machine-proven wrong-media prevention. Bacula retains physical record and EOM authority across the same JobId.

After the write, a correlated `Level=Data` verification requests the recorded volumes in Bacula catalog order and rereads them through the selected decompression path. Readback media requests are non-destructive and never clear or relabel tape. TapeScale then rescans the source to detect mutation. Atomic success requires the streaming source manifest, exact ordered catalog volume set, finalized engine write, complete physical readback of every volume, and unchanged input; it creates the recovery point, ordered volume records, `write_complete`, and `full_readback` verification records together. No restore or manifest comparison occurs on this path, so it cannot emit a restore-test claim. Waiting cancellation is immediate; claimed cancellation is durable and cooperative at safe worker/Bacula boundaries. Failed or uncertain writes never create a recovery point and can quarantine every touched cartridge.

The G0.2 data path is another explicit, destructive, laboratory-only intent. A profile binds a normalized relative selection beneath the read-only `/source` mount to an isolated relative destination beneath `/restore`. Queueing requires the exact passed G0.1 drive/cartridge combination, all destructive/scratch acknowledgements, an explicit encryption-disabled deviation, and `WRITE <profile name> TO <cartridge label> AND RESTORE VERIFY`. PostgreSQL atomically binds the source snapshot, drive identity, cartridge, pinned engine/adapter/format versions, reservations, and fenced leases. Catalog proof uses Bacula's persisted one-byte job codes, including `V/A` for a `Level=Data` verify (`D` is the unrelated differential-backup level).

The worker independently inventories and hashes the source, persists append-only evidence, and commits a durable media-write boundary. Qualification, an interrupted prior attempt, or another scratch workflow can leave records or a valid label at BOT, so the worker uses one fixed internal BOT file-mark command to invalidate prior scratch contents and rewinds again; that destructive primitive is unavailable to the public motion API. It renders fixed Bacula 15.0.3 resources and starts process-isolated Director, Storage Daemon, and File Daemon instances under the singleton controller. A restricted fixed-input `bconsole` adapter labels only the selected scratch volume, submits one full file job, correlates the exact engine JobId, requires strict terminal success, submits `Level=Data` verification against that job, then performs a normal Bacula restore into the server-generated isolated run directory. TapeScale inventories the restored tree independently and compares paths, SHA-256 content, and declared metadata coverage. Only an exact match plus valid engine/tape evidence is atomically promoted to a verified recovery point and volume record. Warnings, verify differences, missing evidence, source changes, lease loss, cancellation, or cleanup uncertainty cannot pass.

The attended restore-rehearsal path is a separate read-only intent derived from an unexpired, blocker-free restore plan. It accepts a verified operational `file/full` origin with one contiguous finalized volume set, or a passed single-volume G0.2 origin, with no dependencies or key lineage, the pinned adapter/format, an intact source-manifest artifact, and one selected current exact G0.1-qualified drive plus qualified/readable cartridge set. Queueing binds the plan and preview hash, original Bacula backup JobId/pool, retained manifest identity, immutable complete-point or selected-path scope, destination, drive identity, versions, reservation, and correlation ID. A complete-point run atomically reserves every ordered cartridge and snapshots each inventory label, on-tape identity, Bacula volume label/MediaId, record coordinates, and encoded size into an authoritative restore ledger; legacy scalar columns remain first-volume compatibility fields. Selected-path execution requires exactly one retained volume. Multi-volume selected plans are blocked because the alpha has no authenticated path-to-volume map or ledger state that can distinguish an untouched cartridge from one physically read.

The singleton worker claims and fences the drive plus volume 1, re-probes the exact SAS identity, validates the backup Job against the complete catalog volume order, and recreates the pinned runtime for the original pool. A pre-progress Bacula mount wait can remount the current tape once. Only an exact structured Storage-Daemon proof that the correlated restore JobId is waiting for the next recorded volume lets the worker durably complete and release the current volume lease, retain that exact next request, and move the run to `waiting_for_media` without consuming a no-progress watchdog. The API accepts only `LOAD <label> FOR RESTORE <run UUID> VOLUME <number>` for that exact request, original drive, and reserved cartridge. The worker then obtains a new media fence, freshly re-probes the drive, rewinds, and mounts the recorded volume. A loaded request without matching in-memory worker state fails closed rather than guessing across restart. The Bacula path can issue only the fixed normal restore, mount/release, status, and cancel commands; after any possible tape read, cleanup also uses the allowlisted rewind and requires ready-at-BOT. It has no rehearsal path to label, erase, invalidate BOT, back up, verify-write, or otherwise mutate tape.

Every rehearsal destination is server generated beneath the planned new relative directory and rehearsal UUID. The worker verifies the retained manifest's path, length, artifact SHA-256, format, counts, coverage, and entry digest, then recomputes the immutable restore scope. A complete-point run restores the entire original job. A single-volume selected-path run resolves paths with parameterized catalog queries pinned to the original JobId and sends only server-generated BVFS table names plus numeric FileIndex/PathId identities through the restricted console adapter; raw source-selection paths never enter `bconsole`. The worker validates the new restore JobId's catalog relationship, inventories the output independently, and compares either the complete manifest or the selected projection while allowing only required ancestor-directory scaffolding. Drive/current-media leases, ordered-volume state, handoff requests, heartbeats, phase events, engine correlation, cancellation intent, immutable artifacts, result, and cleanup evidence are durable. Atomic success additionally requires every authorized ledger row completed in order, no open handoff, and exact ordered-volume evidence in the engine/report artifacts. It appends `restore_test` for a complete point or `selective_restore_test` for a one-volume selection; selective evidence cannot satisfy the complete-point gate and neither kind creates or upgrades a recovery point. Container CI exercises the catalog and BVFS mechanisms with an LTO-typed File device, including a spanning engine fixture. That deterministic engine test does not enable multi-volume selected product execution and does not replace physical SAS/LTO qualification.

Recovery-point file browsing uses the same integrity-checked retained source-manifest artifact as the rehearsal preflight. The API has only a read-only recovery-artifact mount and returns a bounded one-level projection of manifest entries without exposing storage references, host paths, or file content. Browsing performs no tape read. For a compatible file point, the client can request up to 256 paths; the API validates every path against the authenticated manifest, recursively expands directories, removes redundant descendants, closes hard-link groups, and persists a deterministic normalized scope and selection hash. The worker reloads the manifest and requires the same resolution before dispatch, so client state never becomes physical-read authority.

These paths are not the general engine contract: they intentionally exclude encryption, incrementals, changers, native ZFS, schedules other than the fixed weekly full, catalog-loss reconstruction, destination merge/replace, automatic publication, and broad metadata qualification. The operational path implements manual standalone-drive spanning and Bacula compression. Executable complete-point rehearsals replay the recovery point's complete exact volume set, selected-path rehearsals require exactly one retained volume, and the immediate G0.2 exercise remains single-volume. Spanning, compression, attended multi-volume complete restore, and single-volume selected extraction are implemented but not physically qualified by repository tests; multi-volume selected execution is blocked. Bacula remains authoritative for its physical job/volume/catalog records; TapeScale retains the cross-catalog binding, ordered authorizations, manifests, immutable selection, comparison, and operator evidence.

## Backup representations

File mode captures a defined immutable snapshot tree, exact selection rules, namespace state, metadata, and SHA-256 content digests while reading the selected snapshot. Exact incrementals must account for additions, changes, moves, deletions, and metadata-only changes.

Native mode stores versioned full/incremental `zfs send` streams plus dataset, snapshot, property, feature, key, and dependency records. Zvols are block volumes and are not advertised as browsable file trees. Interrupted native streams restart from the beginning unless a narrower engine recovery unit is demonstrated and recorded.

Both mode creates two explicit representations. It is not an implicit claim that a file index makes a native stream seekable by filename.

## Logical source identity

Each run records immutable snapshot name, GUID or stable identifier, capture time, selected descendants, and resolved exclusions. Changing physical snapshot mount paths map to a stable logical root used for catalog identity and restore browsing. Separate pools are described as separate crash-consistency boundaries unless a validated integration establishes something stronger.

TapeScale releases only its own holds, and only after every backup, retry, verification, and dependency that requires the snapshot has ended. User-created snapshots are never removed as job cleanup.

## Engine adapter contract

The adapter is versioned separately from the product API and exposes structured operations for:

- dependency and version health;
- device discovery, stable identity, inventory, and capabilities;
- media identity, reservation, label, import, and reconciliation;
- file/native write planning and execution;
- operation polling, safe cancellation, and durable recovery units;
- read-back verification, restore planning, and restore execution;
- catalog export, import, and reconstruction.

Arguments are passed directly to processes without a shell. Input is validated, output is bounded, timeouts and cancellation are explicit, and both producer and consumer exit status are required for success. The adapter must never infer structured state from decorative console output when a supported structured interface exists.

The development simulator remains available for broader policy and failure testing. It is not tape-engine qualification and must be visibly distinguished from the fenced Bacula adapter used by the operational full-backup and G0.2 paths in system status and diagnostic exports.

## Retention and overwrite gate

Retention is evaluated over the recovery-point dependency graph. A base, intermediate increment, manifest, tape, or key remains protected until its final retained dependent expires. Entire cartridges remain protected while any contained record is retained, held, archived indefinitely, incomplete-but-recoverable, or reserved.

An overwrite decision is a conjunction, never a best effort:

1. Physical media identity is known and matches the requested cartridge.
2. Engine and TapeScale catalogs agree, or reconciliation has explicitly resolved the difference.
3. No retained recovery point, dependency, hold, archive, export, active job, or pending custody task protects the tape.
4. The actual format/generation is writable by the selected drive.
5. Health and position are known.
6. Policy permits reuse.
7. The actor has authority; exceptional destruction also has current reauthentication and an unchanged affected-object preview.

Failure or uncertainty at any step denies the write.

The alpha implements the decision side of this boundary as a Media Control Center. Inventory summaries and cartridge dossiers are derived from the durable catalog, recovery graph, volume records, verification, custody, holds, reservations, and drive observations. Pool assignment and operator-created hold mutations require an expected cartridge `state_version`; a database trigger advances that version for every cartridge update, including worker and custody updates outside the media service. Idempotency receipts, cartridge state, append-only audit, and notification outbox records commit transactionally.

Reuse evaluation accepts an explicit selected drive and returns the conjunction above, including actionable blockers and affected recovery points. It is deliberately read-only. The alpha provides no general erase, relabel, recycle, reuse execution, or policy-override primitive, so an allowed evaluation cannot itself alter media or dispatch engine work.

## API shape

The JSON API is versioned below `/api/v1`. Principal groups are authentication, setup/preflight, TrueNAS and sources, policies and schedules, jobs and runs, devices and media, recovery points and verification, restore planning/execution, locations and rotations, keys and recovery bundles, notifications, audit, and diagnostics. Bacula's restricted console is generated and reachable only on worker loopback. Bacula requires broad Job/Client visibility ACLs for structured Storage-Daemon running-status records, but its command ACL, backup/restore client ACLs, storage, pool, fileset, directory, and destination ACLs remain scoped; the generated configuration defines only TapeScale's fixed jobs and client. Storage allows one data job plus one read-only status connection while the physical Device remains limited to one job.

The production contract makes destructive operations three-step workflows: generate a preview, reauthenticate, then execute using a short-lived token bound to the preview hash and exact object versions. The explicitly unauthenticated G0.1, operational-backup, and G0.2 paths are temporary, isolated-lab exceptions with separate scratch-media acknowledgements and exact typed phrases; their constraints are documented in [Security](SECURITY.md). They must not be exposed as production workflows.

## Deployment rules

- Pin release images and the Rust toolchain; commit the lockfile.
- Persist PostgreSQL, engine catalog/configuration, TapeScale state, manifests, recovery exports, and required spool/staging data outside containers.
- Keep database and engine control ports on private networks.
- Mount sources read-only and restore targets writable only into the worker. If the API performs path preflight or browsing, give it read-only views only. Manifest browsing may add the exact recovery-artifact volume to the API read-only; do not expose worker spool or Bacula runtime state.
- Map only named tape/changer devices. Never mount host root or the Docker socket.
- Use mounted secret files or the platform secret facility; do not place long-lived credentials in checked-in environment files.
- Expose dependency-specific readiness for database, engine, TrueNAS API/certificate, snapshot access, native-ZFS path, devices, keys, scratch capacity, and compatible media.

See [TRUENAS-DEPLOYMENT.md](TRUENAS-DEPLOYMENT.md) for the deployment contract.

## Version and compatibility policy

Every release records the TapeScale version, schema version, engine and adapter version, on-tape format, manifest format, recovery-bundle version, Rust toolchain, TrueNAS/OpenZFS version, host drivers, and device firmware used for qualification. Format changes require regression restoration from supported earlier releases or a maintained compatible reader.

Generation compatibility rules are data, separate from physical qualification evidence. Detection and successful simulation never become a hardware qualification result.

## Alpha implementation versus target contract

Normative language in this document describes the release contract, not a claim that every path is complete. The alpha implements Rust API/scheduler/worker/recovery process boundaries, PostgreSQL schema and coordination primitives, SAS identity/status discovery, guided tape preparation, bounded source and retained-manifest browsing, bounded-memory operational inventory, bounded Bacula data spooling, lab-only rewind/unload, the fenced Bacula `btape` harness, durable attended operational full backups with selectable Bacula compression, manual multi-volume continuation, weekly request scheduling, ordered physical readback, the G0.2 single-volume file write/readback/restore path, durable attended ordered-volume complete-point rehearsals, and single-volume selected-path rehearsals of compatible operational/G0.2 points, plus simulator-backed broader workflows, LTO capability rules, dependency-aware retention, the read-only media reuse gate, versioned pool/hold operations, core state machines, safe process execution, and the guided browser workflow. Production authentication/MFA, TrueNAS snapshot automation and native streaming, destructive reuse execution, a generally qualified engine/format, incrementals, encryption/key lifecycle, changer control, notifications, general restore execution, multi-volume selected execution, clean-host media reconstruction, and physical qualification of compression/EOM/multi-volume/selective restore behavior remain incomplete, blocked, or unqualified. The acceptance ledger is authoritative for evidence status.
