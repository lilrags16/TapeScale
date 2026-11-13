# TrueNAS deployment guide

Supported qualification target: TrueNAS 25.10.8 on x86-64.
Current status: Docker Compose alpha with attended operational full-file backup/write-readback and G0.2 restore-comparison paths; no general TrueNAS or hardware qualification claim has been completed.

TapeScale ships a four-service Compose topology:

- `api`: `tapescale-api`, HTTP port 8080 inside the deployment;
- `scheduler`: `tapescale-scheduler`, durable schedule evaluation and catch-up dispatch without host devices or data mounts;
- `worker`: `tapescale-worker`, privileged orchestration, lab-only rewind/unload, the attended Bacula `btape` harness, fenced operational backup and G0.2 data paths, and attended read-only rehearsals of retained verified points;
- `postgres`: PostgreSQL 16 durable product state.

Production deployment remains blocked until the complete tape-engine matrix passes [ENGINE-EVALUATION.md](ENGINE-EVALUATION.md). A G0.2 pass proves one exact recovery exercise, not general engine, format, or media support.

## Prerequisites

- A named TrueNAS 25.10.8 x86-64 system with exact release and OpenZFS versions recorded.
- Docker Compose/TrueNAS custom-app support appropriate to that release.
- Dedicated persistent datasets for PostgreSQL, TapeScale state, catalog exports, and bounded spool/staging.
- Read-only access to the selected snapshot paths.
- Separately approved writable restore and native staging datasets.
- Locally attached tape/changer devices for later qualification; stable serial/WWN and firmware recorded.
- TLS certificate and hostname for the web endpoint.
- A dedicated TrueNAS API credential with only the permissions justified below.
- A separately controlled administrator credential used only for configuration export with secret seed.

## Dataset layout

Create explicit datasets rather than a broad host mount. A representative layout is:

```text
<apps-pool>/tapescale/postgres
<apps-pool>/tapescale/state
<apps-pool>/tapescale/exports
<apps-pool>/tapescale/spool
<restore-pool>/tapescale-restore
<restore-pool>/tapescale-staging
```

Set quotas/reservations for spool, staging, and restore areas and preserve a configured pool free-space reserve. Source mounts are read-only. In the alpha stack, the API receives read-only views of `/source` and `/restore` plus only `DAC_READ_SEARCH` to validate profile containment and source mount mode through restrictive dataset ACLs; the worker receives read-only `/source` and writable `/restore`. The scheduler receives neither. A production design should move filesystem validation to the fenced worker and narrow the source bind from the alpha stack's broad read-only `/mnt` view to approved snapshot roots.

## Secrets and first run

Provide database credentials, session/secret-encryption material, TrueNAS API credentials, and TLS keys through mounted secret files or the TrueNAS secret facility. Never commit them to `.env`, Compose YAML, shell history, or diagnostic output.

The self-contained `compose.truenas.yaml` is the attended alpha configuration and intentionally embeds its disposable PostgreSQL password. Do not reuse that password or treat this convenience file as the production credential model.

The generic `compose.yaml` has no tape `devices:` mapping and leaves every physical gate disabled. If it is adapted for hardware testing, add an override for the one verified non-rewinding `/dev/nstN` node and keep the container path identical to the host/sysfs name. Do not grant `privileged`, map all `/dev`, or guess a generic SCSI node.

No shared default password is valid. A production first-run bootstrap must create a unique administrator, enroll MFA, issue one-time recovery codes, and invalidate the bootstrap secret. Store recovery codes away from the NAS. The alpha does not yet provide application authentication/MFA; its console must be kept on an isolated trusted network and must not be exposed as a qualified installation.

## TrueNAS API boundary

Control operations use the versioned TrueNAS JSON-RPC API over authenticated secure WebSocket with certificate verification. TapeScale detects and negotiates a supported API version and records the result.

The runtime service account should receive only permissions proven necessary for discovery, snapshot creation/inspection/holds, and the selected native data path. The system-configuration export with “Export Password Secret Seed” requires full administrator access on the baseline release; perform it with a separate, explicitly authorized credential or workflow. Do not silently expand the runtime account.

API availability does not prove that snapshot-file reads, `zfs send`, or `zfs receive` data reach the container. Those paths require separate preflight and qualification.

## Host-helper boundary

If native streaming cannot be safely provided inside the worker container, install a constrained Rust host helper. It may expose only reviewed operations with validated dataset/snapshot/destination arguments. Document its binary digest, service definition, allowed operations, authentication, upgrade procedure, and capabilities.

The helper must not provide arbitrary command execution or a shell. Production deployment must not depend on undocumented host edits.

## Device mapping

Map exact stable tape and changer device nodes required by the qualified utilities. Do not map all of `/dev` and do not use blanket privileged mode. Record:

- OS device path and stable by-id path;
- drive/changer model, serial/WWN, firmware, and generation;
- HBA, driver, and kernel versions;
- read/write capability profile;
- observed changer slots, drives, and import/export elements.

After a reboot or path change, TapeScale reconciles stable identity and blocks automatic selection if it cannot prove the same device.

The current standalone-drive milestone performs discovery, `mt status`, durable rewind/unload, and—behind a separate destructive lab flag—Bacula's `btape test`. On the TrueNAS host, identify the non-rewinding node before deployment:

```sh
ls -l /dev/nst* /dev/tape/by-id/*
```

The alpha Compose maps host `/dev/nst0` to worker `/dev/nst0` with `rwm` device permission and enables motion, G0.1 qualification, and G0.2 file-roundtrip flags. Write permission is used only inside the typed-confirmation scratch workflows. If the host node is `/dev/nst1`, set `TAPESCALE_TAPE_DEVICES` and both the source and target of the `devices` mapping to `/dev/nst1`. All three paths must match because TapeScale rejects symlinks and verifies the device node's major:minor number against `/sys/class/scsi_tape/nst1/dev` before status probes and again immediately before command dispatch. Never guess or map an unknown `/dev/sg*`, which may be a disk. After redeployment, open **Drives & libraries**, inspect the reported vendor/model/serial or WWN, firmware, path, and status, then use **Trust identity** only when they match the intended physical drive. A missing device, path change, firmware change, identity conflict, inaccessible node, immediate-mode setting, outstanding I/O, or uncertain operation result revokes reconciliation.

Alpha `0.1.0-alpha.2` provides the guided operational snapshot-to-tape path with bounded-memory source inventory, selectable automatic/GZIP/LZO/no Bacula compression, and attended standalone-drive volume spanning. A saved backup still points to one exact, already-existing named ZFS snapshot; TapeScale does not create, advance, hold, or select a fresh snapshot for later runs. The first selected pair is carried into the existing final loaded-tape and destructive-overwrite authorization. If Bacula reaches end of media, the same job releases the drive and creates a durable request for the next ordered scratch volume. Each continuation requires an independently eligible, qualified cartridge and an order-specific confirmation before the worker can clear, label, and mount it. After writing, the operator reloads the recorded volumes in order for complete physical Data verification. The operational path has no TapeScale total file-count or logical-byte policy limit, but available storage, engine, hardware, and media capacity still bound a real run. Encryption and incrementals remain unavailable, and readback alone is not a restore test. Recovery Vault can now start a separate attended, read-only Restore run for a compatible single-volume Full point; multi-volume and path-selective restore remain unavailable.

The readiness result is server-authored, returned with `Cache-Control: no-store`, and explicitly reports `safety_authority: false` and `revalidated_on_assignment: true`. It is a current preview, not authorization to mutate media or a guarantee that assignment will succeed. The assignment transaction repeats all gates under locks, and the worker repeats the physical and identity gates before dispatch, so a state change can invalidate a previously displayed candidate.

Test image `0.1.0-test.18` simplifies backup-plan creation to name, snapshot folder, retention, and optional pool. It removes redundant source/limitations attestations; both picker and API require `.zfs/snapshot/<name>`. The API verifies canonical containment, symlink safety, and exact kernel read-only mount flags when saving and again before a new manual request, while the worker repeats the source gate before tape I/O. Lost-response retries return their retained request without depending on the snapshot still being mounted. The console no longer presents the fixed-source weekly scheduler as rolling protection; automated snapshot creation/selection remains future work.

Test image `0.1.0-test.17` adds the graphical source and retained-manifest browsers plus the guided tape-preparation workflow. Source browsing is a bounded metadata-only view beneath `/source`; directories are selectable but files, links, special objects, and the root are not. Recovery browsing derives from the integrity-checked retained source manifest and remains browse-only—the current rehearsal restores the complete job. The workflow now labeled **Prepare tape** sequences current-drive selection, explicit stable-identity trust when needed, inline registration or exact existing cartridge selection, the physical loaded-label assertion, and the unchanged destructive `btape` phrase. These UI improvements do not bypass API or worker gates and create no broader qualification claim.

Test image `0.1.0-test.16` adds the operational Backup Center to the proven physical path. It persists full-file plans, manual and Saturday 02:00 schedule occurrences, waiting-for-media requests, exact attended drive/cartridge assignment, cancellation, phase history, source manifests, Bacula correlation, physical full readback, volume records, and recovery points. The scheduler never chooses or overwrites media: each request remains waiting until an operator supplies the exact destructive phrase. A verified operational backup has `write_complete` and `full_readback` evidence but no restore-test claim.

The image retains the `test.15` attended, source-independent restore rehearsal. A ready Recovery Vault plan can reserve the exact original qualified drive and cartridge, replay the retained Bacula backup JobId into a fresh isolated directory, compare an independently generated restored manifest, and append a new restore-test verification to the existing recovery point. Every phase, cancellation request, artifact identity, comparison, lease, and terminal result is durable. It also retains the Media Control Center from `test.14`: searchable cartridge dossiers join identity, pool, confirmed location, reservations, physical content coordinates, retained and dependent recovery points, verification, holds, custody, and audit history.

The selected-drive reuse evaluation is read-only and fail-closed. It lists affected recovery points and blocks unknown/conflicting identity, catalog disagreement, retention/dependencies, holds, archives, reservations, custody work, WORM media, health/location uncertainty, incompatible actual format, or an unsuitable drive. This image exposes no general erase, relabel, recycle, overwrite-execution, or administrator-override endpoint. An `allowed` result is not authorization to destroy a cartridge and dispatches no tape command.

This image retains the searchable Recovery Vault, bounded evidence downloads, durable restore-readiness plans, and complete manual media-custody operations. Named on-site, off-site, transit, and device locations can be created in the console. Exact tape sets can be reserved, partially confirmed through departure and arrival or confirmed as a direct physical move, cancelled, and audited; the scheduler durably flags overdue work. The overview surfaces real movement, job, and media actions. Restore plans join the verified point to its dependency chain, ordered tape coordinates, current custody, exact compatible qualification, key declarations, destination constraints, and capacity blockers.

For a blocker-free plan backed by one compatible verified operational-backup or passed G0.2 full-file point, Recovery Vault exposes a separate attended Restore run. The operator selects the exact original qualified drive, confirms the listed tape is loaded and the destination is isolated, and types the phrase bound to the plan. PostgreSQL then reserves and fences the exact drive/cartridge and retains queue, phase, cancellation, artifact, comparison, result, audit, and verification evidence. The worker uses the original Bacula backup JobId/catalog only for a read into `/restore/<plan path>/<rehearsal id>` and compares it with the authenticated retained source manifest. No Restore-run endpoint can label, erase, clear BOT, back up, replace/merge a destination, or publish recovered data. Native, incremental, encrypted, multi-volume, other-format, path-selective, and lost-catalog plans remain planning-only.

This image retains the `test.12` exact per-job Bacula catalog proof and strengthened file-backed CI roundtrip: after the first backup, Data verify, restore, and catalog proof, CI stops every Bacula daemon, makes the source tree unavailable, starts a fresh runtime, restores the same retained JobId into a second destination, and byte-compares that result. Only the Full backup must carry the exercise pool, while successful Data-verify and Restore rows legitimately have no `PoolId`. The proof verifies the pinned `LTO` media type and matches the complete Bacula restore relationship without partial JobId matches. The File device removes the hardware dependency but does not count as physical-media qualification. The image also retains the package-exact schema/bootstrap gates, pre-write schema fingerprint/privilege gate, exact bounded SQL stderr, and scratch-media preparation. Its Git context and OCI revision are pinned to `v0.1.0-alpha.2`. The API container root filesystem remains read-only and receives only `DAC_READ_SEARCH`, read-only `/source` and `/restore` views, and a read-only recovery-artifact volume for integrity-checked manifest browsing. The worker remains UID/GID 0 and receives only `CHOWN`, `DAC_OVERRIDE`, `DAC_READ_SEARCH`, `FOWNER`, and `FSETID`; these are needed to read protected snapshot content and restore recorded ownership/mode bits. It is not privileged. Bacula daemons bind only to worker loopback, configuration and PID files use bounded tmpfs mounts, daemon logs are continuously drained to bounded evidence files, Bacula working/bootstrap state uses `worker_bacula_runtime`, and PostgreSQL retains product state plus the engine catalog. The worker still has no Docker socket or writable host-root mount.

Linux may finalize a prior write with a filemark before rewind/offline even though TapeScale issues no data-write command. The API therefore requires an explicit designated-scratch-media acknowledgement. Do not connect retained, production, unknown, or foreign media to this unqualified workflow.

The alpha API does not yet implement accounts or authentication. Anyone who can reach port 18080 can request allowlisted motion or attempt to queue destructive qualification, backup, or G0.2 work after supplying its visible confirmations. Use only an isolated, attended network and disable all three physical-media flags before connecting non-scratch media.

The motion gate exposes rewind and rewind-and-unload through fixed `mt-st` argument vectors. The qualification gate exposes one fixed Bacula invocation with the literal `test\n` input transcript; it does not expose `fill`, label, erase, a console, or arbitrary arguments. Before queueing, TapeScale requires an exact typed cartridge label, both destructive acknowledgements, an unprotected non-WORM inventory record, and a trusted ready drive. The worker holds one fenced drive lease, rechecks physical identity immediately before dispatch, and serializes discovery/motion with `btape`. A successful test is followed by a required rewind and ready-at-BOT observation. A process deadline cannot force tape I/O stuck in the kernel to finish; timeout, cancellation, lease loss, or an uncertain postcondition revokes reconciliation. A passing run is narrow G0.1 evidence and does not satisfy the complete engine/format gate.

## Backup, G0.2 source, and restore mounts

Before installing `compose.truenas.yaml`, create `/mnt/tapescale-restores` as a dedicated empty alpha restore dataset/path using a Generic/POSIX-compatible ACL preset. Do not share it over SMB or NFS. The Compose bind uses `create_host_path: false`, so installation fails instead of silently creating a plain directory when this dataset is missing. The pasteable alpha file binds:

- host `/mnt` to container `/source` as read-only in API and worker;
- host `/mnt/tapescale-restores` to `/restore` as read-only in API and read/write in worker;
- Docker volume `worker_recovery` as read/write in the worker and read-only in the API so the Recovery Vault can validate and project retained manifests without exposing storage paths;
- the exact non-rewinding tape device only to the worker;
- Docker-managed evidence, Bacula working, and PostgreSQL volumes.

The broad `/mnt` source bind is an alpha convenience, not the production layout. In **Backups**, use the graphical browser to navigate one directory level at a time; it shows bounded folder/file metadata and can surface the contained `.zfs` namespace even when OpenZFS omits it from ordinary directory enumeration. Files, symlinks, special objects, and `/source` itself cannot become a backup root. In **Backups** or **Recovery exercise**, select only an immutable snapshot path such as `pool/dataset/.zfs/snapshot/tapescale-full`. TapeScale rejects traversal, resolves the selection beneath `/source`, and asks the kernel for the exact open directory's read-only mount flags when the selection is saved and again before a manual request is queued. A saved backup remains bound to that exact already-existing snapshot. The alpha does not create, hold, retain, advance, or replace the snapshot; protecting a newer point in time requires selecting and saving that named snapshot separately.

The Recovery Vault's file browser reads only a retained source-manifest artifact from the API's read-only recovery volume. It returns bounded recorded names/type/size/mtime, not file contents or internal artifact paths, and it performs no tape I/O. The alpha remains unauthenticated, so both filename-browsing APIs increase information visible to anyone who can reach port 18080; keep it on the isolated attended network. Retained-manifest browsing does not enable selective restore—the current rehearsal restores the complete job.

Operational backups are manually started, full-file, unencrypted, and complete-readback verified. A profile can request automatic fast GZIP, balanced GZIP, LZO, or no compression. With spanning enabled, one Bacula job may continue across an ordered set of manually supplied cartridges in the same standalone drive; TapeScale retains each authorization, engine label, and catalog segment. It still does not create incrementals. The console does not yet automate rolling snapshot creation or selection, and the API-level fixed-source Saturday scheduler remains an engineering mechanism rather than a rolling-backup claim.

When the operator chooses **Start backup**, the console first requests the server's current readiness preview for the saved profile. Only server-returned candidate pairs are shown: the saved snapshot binding, trusted ready drive identity/path, accessible device, cartridge eligibility and identity, required pool, generation compatibility, protection and WORM state, holds, reservations, unverified-write state, exact current setup-test result, and active work are considered. If the pair needs its destructive setup test, **Prepare loaded tape** opens that workflow directly and the backup resumes only after the exact pair passes. The preview is returned without cache and is deliberately non-authoritative for mutation (`safety_authority: false`): final assignment repeats all gates under locks, and the worker repeats them before tape I/O.

After the operator selects the physically loaded scratch tape, TapeScale creates one durable waiting request and rechecks the same pair before final authorization. The operator accepts one combined statement covering physical label verification, destructive overwrite, and the absence of encryption, then types `OVERWRITE <label> FOR BACKUP <request UUID>`. For a spanning write, each next tape has a distinct durable request and `OVERWRITE <label> FOR BACKUP <request UUID> VOLUME <number>` confirmation. For full readback, the requests are read-only and use `LOAD <label> FOR BACKUP <request UUID> READBACK VOLUME <number>`; TapeScale never clears or relabels media during that phase. The dedicated backup page retains the stage, next action, ordered tape history, measured progress when the engine supplies it, outcome, and cancellation controls. A successful run retains a recovery point only after the Bacula Full job and complete physical Data verification of every cataloged volume succeed and the source remains unchanged. **Browse backed-up files** projects the integrity-checked retained manifest without tape I/O, and **Recovery options** opens the recovery point. Neither complete readback nor manifest browsing performs or proves a restore test.

Create a restore profile subpath below `/mnt/tapescale-restores`. Each run receives a server-generated run directory created with descriptor-relative, no-symlink-following operations. The worker must never restore into `/source`, an existing live dataset, or an operator-supplied absolute path. A G0.2 run is destructive to tape and explicitly disables encryption as a recorded lab deviation. It supports exactly one full file capture, one standalone drive, one cartridge, at most 100,000 manifest entries, and at most 100 GiB logical data. A passed `btape` result for the exact drive/cartridge is a prerequisite.

## Compose deployment

From the repository root, review the rendered configuration before starting:

```bash
docker compose config
docker compose build --pull
docker compose up -d postgres
docker compose up -d api scheduler worker
docker compose ps
```

Use pinned release image digests for a qualified installation. Local builds are development artifacts unless their source commit, toolchain, lockfile, SBOM, and image digest are retained.

Expose port 8080 only through the configured TLS ingress/reverse proxy. Keep PostgreSQL on the private Compose network. Neither API nor worker needs the Docker socket.

## Preflight

Before enabling a policy, the web preflight must report each item independently:

- PostgreSQL schema and persistent-volume health;
- TapeScale API/scheduler/worker version agreement;
- TrueNAS API version, authentication, and certificate verification;
- selected dataset identity and snapshot visibility;
- source ACL/read access and read-only mount status;
- native `zfs send`/receive or helper execution path;
- configuration-export freshness;
- drive/changer access and stable identity;
- selected engine/tool versions and license inventory;
- key availability without revealing key material;
- spool, staging, restore capacity, quotas, and free-space reserve;
- compatible writable media and overwrite-gate result.

The fresh alpha console starts without operational fixtures and reports only records stored in PostgreSQL. A correctly mapped SAS drive can make the device-access and stable-identity checks pass after explicit reconciliation. **Prepare tape** can sequence that same trust action, inline registration or exact-record selection, the physical loaded-label assertion, and destructive `btape` confirmation in one wizard; the API and worker still revalidate every boundary independently. A completed `btape` run is displayed separately and never promotes the drive or product to generally qualified. An operational backup can create one verified recovery point after its potentially multi-volume Full write, ordered physical Data verification, immutable-source recheck, and atomic evidence commit; it does not claim a restore test. A G0.2 run adds a normal isolated restore and independent manifest match. A later attended Restore run can append an independent restore-test result to a compatible single-volume operational or G0.2 point. None of these results generally qualifies native ZFS, NFSv4 ACL, compression, spanning, encryption, changers, clean-catalog recovery, or the engine/format.

## TrueNAS custom-app configuration

For the fixture-free alpha deployment, first create `/mnt/tapescale-restores`, then copy the complete repository-root `compose.truenas.yaml` into **Apps > Discover Apps > Install via YAML**. It builds the public `v0.1.0-alpha.2` tag, exposes the console on host port 18080, keeps the intentionally inline disposable database password, stores state in Docker-managed volumes, enables all three attended scratch-media gates, mounts the source/restore roots plus read-only recovery-manifest view described above, and maps `/dev/nst0:rwm` only to the worker. The initial operational UI is empty; discovered drives, guided tape preparation, motion history, registered cartridges, pools, holds, qualification runs, saved snapshot backups and requests, recovery exercises, locations, and movements become the first real durable records.

To update this Git-context build, edit the installed custom app, replace its YAML with the latest raw `compose.truenas.yaml`, and save. Because the file uses `pull_policy: build`, the redeploy fetches and rebuilds the release tag named in that file. Retain the resulting image digest with physical-test evidence and verify the revision label plus container logs before relying on a newly implemented workflow.

For a future qualified deployment, translate the same Compose boundaries into explicit TrueNAS datasets and production credential facilities:

- four services: API, scheduler, worker, PostgreSQL 16;
- private application network and TLS ingress to API port 8080;
- persistent host-path datasets for the explicit state paths;
- no devices on API or scheduler; if API-side path validation remains enabled, API receives read-only source/restore views only;
- read-only selected source snapshot mounts to worker only;
- read/write approved restore/staging mounts to worker only;
- explicit qualified device mappings to worker only;
- secret-file mounts with restrictive permissions;
- health checks and CPU/memory/process limits;
- restart policy that does not imply safe write resumption.

Capture screenshots or exported YAML, with secrets redacted, as AT-01 evidence. Any custom-app field or behavior must be revalidated against the pinned TrueNAS release.

## Upgrade and rollback

1. Prevent new schedules and drain operations at documented safe boundaries.
2. Confirm no drive has an uncertain or active write state.
3. Create and verify PostgreSQL, configuration, and recovery-bundle backups.
4. Record current image, schema, engine, adapter, and format versions.
5. Pull/build the pinned release and run versioned migrations.
6. Start services, run preflight, and reconcile devices, media, and jobs.
7. Restore a supported historical fixture before resuming production.

Restoring the old image against a migrated database is not rollback. Follow the release's database recovery plan.

## Removal

Stopping or removing containers must not delete persistent datasets, recovery bundles, engine catalog, or key packages. Before permanent removal, export and verify the catalog/configuration bundle and document custody of all retained tapes. Destruction of persistent state is a separate explicitly authorized operation.

## Alpha implementation boundary

The Compose topology, PostgreSQL 16 persistence, health surface, static console, simulator, service separation, standalone SAS discovery, guided tape preparation, server-authored readiness preview, dedicated backup progress, bounded source/manifest browsing, bounded-memory operational inventory, guarded rewind/unload, G0.1 `btape`, attended compressed/uncompressed multi-volume operational full-file backup/write-readback, the narrower single-volume G0.2 restore-comparison roundtrip, and narrow verified-point rehearsal are alpha implementation. TLS ingress, production identity/MFA, TrueNAS WebSocket snapshot creation/holds, path-selective restore execution, native ZFS/helper data paths, changer control, incrementals, encryption/key packages, general restore execution, clean-catalog recovery, NFSv4 ACL fidelity, and broad physical qualification—including physical compression and end-of-media cases—remain incomplete or unqualified. Do not turn a target checklist item green solely because browsing, discovery, motion, `btape`, one readiness result, or one verified backup succeeds.
