# TrueNAS deployment guide

Supported qualification target: TrueNAS 25.10.8 on x86-64.
Current status: Docker Compose engineering preview; no TrueNAS or hardware qualification has been completed.

TapeScale ships a four-service Compose topology:

- `api`: `tapescale-api`, HTTP port 8080 inside the deployment;
- `scheduler`: `tapescale-scheduler`, durable schedule evaluation and catch-up dispatch without host devices or data mounts;
- `worker`: `tapescale-worker`, privileged orchestration and the simulator engine adapter;
- `postgres`: PostgreSQL 16 durable product state.

Production deployment remains blocked until a tape engine passes [ENGINE-EVALUATION.md](ENGINE-EVALUATION.md). Do not map production media to the simulator and infer that a write is protected.

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

Set quotas/reservations for spool, staging, and restore areas and preserve a configured pool free-space reserve. Source mounts are read-only. The API service receives none of these source/restore mounts; only the worker receives the exact paths it needs.

## Secrets and first run

Provide database credentials, session/secret-encryption material, TrueNAS API credentials, and TLS keys through mounted secret files or the TrueNAS secret facility. Never commit them to `.env`, Compose YAML, shell history, or diagnostic output.

The self-contained `compose.truenas.yaml` is an explicitly disposable test configuration and intentionally embeds its PostgreSQL password. Do not reuse that password or treat this convenience file as the production credential model.

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

The fresh preview console starts without operational fixtures and reports only records stored in PostgreSQL. The engine adapter remains a simulator and cannot make device, ZFS, ACL, engine, or media checks pass.

## TrueNAS custom-app configuration

For the current fixture-free test deployment, copy the complete repository-root `compose.truenas.yaml` into **Apps > Discover Apps > Install via YAML**. It builds public `main`, exposes the console on host port 18080, keeps the intentionally inline test database password, and stores state in Docker-managed volumes. The initial UI is empty; manually registered cartridges are the first real durable records.

To update this Git-context build, edit the installed custom app, replace its YAML with the latest raw `compose.truenas.yaml`, and save. Because the file uses `pull_policy: build`, the redeploy fetches and rebuilds the current `main` source. Verify the new commit and container logs before relying on any newly implemented workflow.

For a future qualified deployment, translate the same Compose boundaries into explicit TrueNAS datasets and production credential facilities:

- four services: API, scheduler, worker, PostgreSQL 16;
- private application network and TLS ingress to API port 8080;
- persistent host-path datasets for the explicit state paths;
- no devices or source/restore mounts on API or scheduler;
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

The Compose topology, PostgreSQL 16 persistence, health surface, static console, simulator, and service separation are preview implementation. TLS ingress, production identity/MFA, TrueNAS WebSocket integration, ZFS/helper data paths, hardware device mappings, selected tape engine, backup/restore data flow, key packages, and physical preflight evidence are target requirements and remain incomplete or unqualified. Do not turn a target checklist item green solely because its configuration field exists.
