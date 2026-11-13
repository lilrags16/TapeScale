# TrueNAS deployment guide

Supported qualification target: TrueNAS 25.10.8 on x86-64.
Current status: Docker Compose engineering preview with an attended G0.2 file-roundtrip path; no general TrueNAS or hardware qualification claim has been completed.

TapeScale ships a four-service Compose topology:

- `api`: `tapescale-api`, HTTP port 8080 inside the deployment;
- `scheduler`: `tapescale-scheduler`, durable schedule evaluation and catch-up dispatch without host devices or data mounts;
- `worker`: `tapescale-worker`, privileged orchestration, lab-only rewind/unload, the attended Bacula `btape` harness, the fenced Bacula G0.2 write/readback path, and attended read-only rehearsals of its retained verified points;
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

Set quotas/reservations for spool, staging, and restore areas and preserve a configured pool free-space reserve. Source mounts are read-only. In the G0.2 test stack, the API receives read-only views of `/source` and `/restore` plus only `DAC_READ_SEARCH` to validate profile containment and source mount mode through restrictive dataset ACLs; the worker receives read-only `/source` and writable `/restore`. The scheduler receives neither. A production design should move filesystem validation to the fenced worker and narrow the source bind from the test stack's broad read-only `/mnt` view to approved snapshot roots.

## Secrets and first run

Provide database credentials, session/secret-encryption material, TrueNAS API credentials, and TLS keys through mounted secret files or the TrueNAS secret facility. Never commit them to `.env`, Compose YAML, shell history, or diagnostic output.

The self-contained `compose.truenas.yaml` is an explicitly disposable test configuration and intentionally embeds its PostgreSQL password. Do not reuse that password or treat this convenience file as the production credential model.

The generic `compose.yaml` has no tape `devices:` mapping and leaves every physical gate disabled. If it is adapted for hardware testing, add an override for the one verified non-rewinding `/dev/nstN` node and keep the container path identical to the host/sysfs name. Do not grant `privileged`, map all `/dev`, or guess a generic SCSI node.

No shared default password is valid. A production first-run bootstrap must create a unique administrator, enroll MFA, issue one-time recovery codes, and invalidate the bootstrap secret. Store recovery codes away from the NAS. The preview does not yet provide a production authentication/MFA bootstrap; demo access must not be exposed as a qualified installation.

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

The test Compose maps host `/dev/nst0` to worker `/dev/nst0` with `rwm` device permission and enables motion, G0.1 qualification, and G0.2 file-roundtrip flags. Write permission is used only inside the typed-confirmation scratch workflows. If the host node is `/dev/nst1`, set `TAPESCALE_TAPE_DEVICES` and both the source and target of the `devices` mapping to `/dev/nst1`. All three paths must match because TapeScale rejects symlinks and verifies the device node's major:minor number against `/sys/class/scsi_tape/nst1/dev` before status probes and again immediately before command dispatch. Never guess or map an unknown `/dev/sg*`, which may be a disk. After redeployment, open **Drives & libraries**, inspect the reported vendor/model/serial or WWN, firmware, path, and status, then use **Trust identity** only when they match the intended physical drive. A missing device, path change, firmware change, identity conflict, inaccessible node, immediate-mode setting, outstanding I/O, or uncertain operation result revokes reconciliation.

Test image `0.1.0-test.15` adds an attended, source-independent restore rehearsal to the proven G0.2 tape path. A ready Recovery Vault plan can reserve the exact original qualified drive and cartridge, replay the retained Bacula backup JobId into a fresh isolated directory, compare an independently generated restored manifest, and append a new restore-test verification to the existing recovery point. Every phase, cancellation request, artifact identity, comparison, lease, and terminal result is durable. The image also retains the Media Control Center from `test.14`: searchable cartridge dossiers join identity, pool, confirmed location, reservations, physical content coordinates, retained and dependent recovery points, verification, holds, custody, and audit history.

The selected-drive reuse evaluation is read-only and fail-closed. It lists affected recovery points and blocks unknown/conflicting identity, catalog disagreement, retention/dependencies, holds, archives, reservations, custody work, WORM media, health/location uncertainty, incompatible actual format, or an unsuitable drive. This image exposes no general erase, relabel, recycle, overwrite-execution, or administrator-override endpoint. An `allowed` result is not authorization to destroy a cartridge and dispatches no tape command.

This image retains the searchable Recovery Vault, bounded evidence downloads, durable restore-readiness plans, and complete manual media-custody operations. Named on-site, off-site, transit, and device locations can be created in the console. Exact tape sets can be reserved, partially confirmed through departure and arrival or confirmed as a direct physical move, cancelled, and audited; the scheduler durably flags overdue work. The overview surfaces real movement, job, and media actions. Restore plans join the verified point to its dependency chain, ordered tape coordinates, current custody, exact compatible qualification, key declarations, destination constraints, and capacity blockers.

For a blocker-free plan backed by one passed G0.2 full-file point, the Recovery Vault now exposes a separate attended rehearsal. The operator selects the exact original qualified drive, confirms the listed tape is loaded and the destination is isolated, and types the phrase bound to the plan. PostgreSQL then reserves and fences the exact drive/cartridge and retains queue, phase, cancellation, artifact, comparison, result, audit, and verification evidence. The worker uses the original Bacula backup JobId/catalog only for a read into `/restore/<plan path>/<rehearsal id>` and compares it with the retained source manifest. No rehearsal endpoint can label, erase, clear BOT, back up, replace/merge a destination, or publish recovered data. Native, incremental, encrypted, multi-volume, other-format, and lost-catalog plans remain planning-only.

This image retains the `test.12` exact per-job Bacula catalog proof and strengthens the file-backed CI roundtrip: after the first backup, Data verify, restore, and catalog proof, CI stops every Bacula daemon, makes the source tree unavailable, starts a fresh runtime, restores the same retained JobId into a second destination, and byte-compares that result. Only the Full backup must carry the exercise pool, while successful Data-verify and Restore rows legitimately have no `PoolId`. The proof verifies the pinned `LTO` media type and matches the complete Bacula restore relationship without partial JobId matches. The File device removes the hardware dependency but does not count as physical-media qualification. The image also retains the package-exact schema/bootstrap gates, pre-write schema fingerprint/privilege gate, exact bounded SQL stderr, and scratch-media preparation. Its Git context and OCI revision are pinned to `v0.1.0-test.15`. The API container root filesystem remains read-only and receives only `DAC_READ_SEARCH`. The worker remains UID/GID 0 and receives only `CHOWN`, `DAC_OVERRIDE`, `DAC_READ_SEARCH`, `FOWNER`, and `FSETID`; these are needed to read protected snapshot content and restore recorded ownership/mode bits. It is not privileged. Bacula daemons bind only to worker loopback, configuration and PID files use bounded tmpfs mounts, daemon logs are continuously drained to bounded evidence files, Bacula working/bootstrap state uses `worker_bacula_runtime`, and PostgreSQL retains product state plus the engine catalog. The worker still has no Docker socket or writable host-root mount.

Linux may finalize a prior write with a filemark before rewind/offline even though TapeScale issues no data-write command. The API therefore requires an explicit designated-scratch-media acknowledgement. Do not connect retained, production, unknown, or foreign media to this unqualified workflow.

The preview API does not yet implement accounts or authentication. Anyone who can reach test port 18080 can request allowlisted motion or attempt to queue either destructive workflow after supplying its visible confirmations. Use only an isolated, attended lab network and disable all three lab flags before connecting non-scratch media.

The motion gate exposes rewind and rewind-and-unload through fixed `mt-st` argument vectors. The qualification gate exposes one fixed Bacula invocation with the literal `test\n` input transcript; it does not expose `fill`, label, erase, a console, or arbitrary arguments. Before queueing, TapeScale requires an exact typed cartridge label, both destructive acknowledgements, an unprotected non-WORM inventory record, and a trusted ready drive. The worker holds one fenced drive lease, rechecks physical identity immediately before dispatch, and serializes discovery/motion with `btape`. A successful test is followed by a required rewind and ready-at-BOT observation. A process deadline cannot force tape I/O stuck in the kernel to finish; timeout, cancellation, lease loss, or an uncertain postcondition revokes reconciliation. A passing run is narrow G0.1 evidence and does not satisfy the complete engine/format gate.

## G0.2 source and restore mounts

Before installing `compose.truenas.yaml`, create `/mnt/tapescale-restores` as a dedicated empty test dataset/path using a Generic/POSIX-compatible ACL preset. Do not share it over SMB or NFS. The Compose bind uses `create_host_path: false`, so installation fails instead of silently creating a plain directory when this dataset is missing. The pasteable test file binds:

- host `/mnt` to container `/source` as read-only in API and worker;
- host `/mnt/tapescale-restores` to `/restore` as read-only in API and read/write in worker;
- the exact non-rewinding tape device only to the worker;
- Docker-managed evidence, Bacula working, and PostgreSQL volumes.

The broad `/mnt` source bind is a test convenience, not the production layout. In **Recovery lab**, select only an immutable snapshot path such as `pool/dataset/.zfs/snapshot/tapescale-g02`. TapeScale rejects traversal, resolves the selection beneath `/source`, and verifies that the most-specific container mount is read-only. The operator's immutable-snapshot acknowledgement is additional evidence; it is not a substitute for that mount check.

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

The fresh preview console starts without operational fixtures and reports only records stored in PostgreSQL. A correctly mapped SAS drive can make the device-access and stable-identity checks pass after explicit reconciliation. A completed `btape` run is displayed separately and never promotes the drive or product to generally qualified. A G0.2 run can create one verified recovery point only after a Bacula full backup, physical Data verification, normal isolated restore, and independent manifest match. A later attended rehearsal can append another restore-test result to that exact point but cannot make native ZFS, NFSv4 ACL, spanning, encryption, changer, clean-catalog, or full engine checks pass.

## TrueNAS custom-app configuration

For the current fixture-free test deployment, first create `/mnt/tapescale-restores`, then copy the complete repository-root `compose.truenas.yaml` into **Apps > Discover Apps > Install via YAML**. It builds the public `v0.1.0-test.15` tag, exposes the console on host port 18080, keeps the intentionally inline test database password, stores state in Docker-managed volumes, enables all three attended scratch-media lab gates, mounts the source/restore roots described above, and maps `/dev/nst0:rwm` only to the worker. The initial operational UI is empty; discovered drives, motion history, manually registered cartridges, pools, holds, qualification runs, profiles, recovery exercises, locations, and movements become the first real durable records.

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

## Preview implementation boundary

The Compose topology, PostgreSQL 16 persistence, health surface, static console, simulator, service separation, standalone SAS discovery, lab-only rewind/unload, G0.1 `btape`, the attended G0.2 full-file roundtrip, and its narrow verified-point rehearsal are preview implementation. TLS ingress, production identity/MFA, TrueNAS WebSocket snapshot control, native ZFS/helper data paths, changer control, spanning, incrementals, encryption/key packages, general restore execution, clean-catalog recovery, NFSv4 ACL fidelity, and broad physical qualification remain incomplete or unqualified. Do not turn a target checklist item green solely because discovery, motion, `btape`, or one recovery exercise succeeds.
