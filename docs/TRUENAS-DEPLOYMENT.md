# TrueNAS custom-app deployment

TapeScale `0.2.0-alpha.9` is an attended evaluation build for TrueNAS SCALE on x86-64. It is not production-qualified. The repository-root [`compose.truenas.yaml`](../compose.truenas.yaml) is the complete pasteable custom-app definition.

The active data path is TapeScale Native v2 with DAR 2.8.6. Native v1 remains readable; the runtime does not install or start Bacula.

## What the stack runs

- `api`: web/API on container port 8080, published as host port 18080;
- `scheduler`: durable request/schedule evaluation, without data or device mounts;
- `worker`: source inventory, DAR staging/testing, native preparation, tape write/readback, and evidence retention;
- `postgres`: PostgreSQL 16 product state on an internal-only network.

Only the worker receives the tape device. Neither API nor worker receives the Docker socket or a writable host-root mount.

## Before installation

### 1. Create the dedicated datasets

Create:

- `/mnt/tapescale-spool`: large, fast, writable native rolling spool;
- `/mnt/tapescale-restores`: isolated writable restore destination.

Use the Generic/POSIX ACL preset. Do not share either dataset over SMB or NFS. The worker runs as container root with a narrow capability set and must be able to create, write, rename, and `fsync` files in both locations.

The spool does not need to hold the complete archive. It must hold the configured high-water window plus reserve; a dedicated 1 TiB filesystem supports the release 768/256/8 GiB policy when sufficient free space remains. Fast HDD may be usable but NVMe/SSD provides more headroom for concurrent pack production and tape consumption. Do not place the spool beneath the source, restore, or evidence tree.

The Compose bind mounts use `create_host_path: false`; installation fails instead of silently creating ordinary directories when a dataset is absent.

All host-specific values are together at the top of the Compose file. Edit only this block during normal installation:

```yaml
x-tapescale-host-settings:
  source: &tapescale-source-host-path /mnt
  spool: &tapescale-spool-host-path /mnt/tapescale-spool
  restore: &tapescale-restore-host-path /mnt/tapescale-restores
  tape: &tapescale-tape-device-mapping /dev/nst0:/dev/nst0:rwm
```

The service mount lists consume those YAML anchors and retain their required read-only/read-write boundaries. Do not edit the repeated service definitions.

### 2. Identify the tape device

On the TrueNAS host, determine the non-rewinding device exported for the standalone drive, normally `/dev/nst0`. Set the single `tape` value in `x-tapescale-host-settings`, keeping the mapping's host and container paths identical. If the host uses `/dev/nst1`, use `/dev/nst1:/dev/nst1:rwm`. The worker automatically discovers mapped `/dev/nstN` devices inside the container. Never map `/dev/stN`; automatic rewind on close breaks TapeScale's tape-file layout.

### 3. Confirm the source model

The alpha maps host `/mnt` read-only as container `/source` so the graphical browser can select existing named snapshots. A selection must resolve to a directory beneath `.zfs/snapshot/<name>` and the kernel must report its open mount read-only.

This broad read-only bind is an alpha convenience. A qualified deployment should expose only approved snapshot roots.

## Install from YAML

1. Open **Apps → Discover Apps → ⋮ → Install via YAML**.
2. Paste the entire contents of [`compose.truenas.yaml`](../compose.truenas.yaml).
3. Edit only `x-tapescale-host-settings` and check the host port.
4. Install the app.
5. Open `http://TRUENAS-IP:18080` from the isolated management network.

The Compose build context follows GitHub `main`, and `pull_policy: build` makes every explicit install or TrueNAS redeploy resolve and build the current branch head. It does not update a running app in the background. This mutable channel favors rapid alpha testing over reproducible release pinning; retain the resulting image digest and application logs with any qualification evidence.

## Native worker configuration

The stack enables:

```text
TAPESCALE_TAPE_MOTION_ENABLED=true
TAPESCALE_TAPE_PREPARATION_ENABLED=true
TAPESCALE_NATIVE_BACKUP_ENABLED=true
TAPESCALE_DAR_PATH=/usr/local/bin/dar
TAPESCALE_NATIVE_SPOOL_DIR=/var/lib/tapescale/spool
TAPESCALE_NATIVE_SPOOL_RESERVE_BYTES=8589934592
TAPESCALE_NATIVE_PACK_TARGET_BYTES=274877906944
TAPESCALE_NATIVE_SPOOL_HIGH_WATER_BYTES=824633720832
TAPESCALE_NATIVE_SPOOL_LOW_WATER_BYTES=274877906944
TAPESCALE_NATIVE_SLICE_BYTES=68719476736
TAPESCALE_NATIVE_COMPRESSION_THREADS=4
```

The first three flags expose physical operations. Disable motion/preparation before connecting retained or unknown cartridges. New deployments should use these canonical names; `TAPESCALE_LAB_*` names are compatibility aliases only.

The image builds hash-pinned DAR 2.8.6 and libthreadar 1.6.1 from their unmodified upstream archives. Zstandard, LZ4, GZIP, and multithreading are enabled. Source archives and license texts are retained in the image as described by [`THIRD_PARTY_NOTICES.md`](../THIRD_PARTY_NOTICES.md).

## Mount and privilege boundaries

| Service | Source | Spool | Restore | Recovery evidence | Tape device |
|---|---|---|---|---|---|
| API | `/mnt` → `/source` read-only | none | read-only | read-only | none |
| Scheduler | none | none | none | none | none |
| Worker | `/mnt` → `/source` read-only | read/write | read/write | read/write | exact `/dev/nstN:rwm` |

The API runs with only `DAC_READ_SEARCH` so it can inspect restrictive read-only snapshot paths. The worker receives `CHOWN`, `DAC_OVERRIDE`, `DAC_READ_SEARCH`, `FOWNER`, and `FSETID` for source metadata access and recovery fidelity. All containers drop every other capability, use `no-new-privileges`, and have a read-only root filesystem plus bounded tmpfs.

PostgreSQL is not published outside the private `control` network. Its named volume and the worker recovery volume are durable across container replacement. The spool is a host dataset, not a hidden Docker volume.

## First startup checks

After deployment, confirm:

```sh
docker compose ps
docker compose logs api worker scheduler postgres
```

The UI should report:

- PostgreSQL ready and migrations through the current version;
- TapeScale Native v2 / DAR 2.8.6 as the active engine/codec;
- native backup, tape preparation, and manual motion enabled;
- no fake jobs, tapes, recovery points, or test fixtures on a fresh database;
- the exact mapped drive after discovery.

The web preflight separates database readiness, native engine availability, device access, discovered stable identity, and an exact prepared drive/cartridge pair. A discovered drive is not automatically trusted. Reconcile it, then use **Prepare tape** only with blank or expendable scratch media.

## Prepare and back up

Preparation is destructive. It tests two native tape files and writes the inventory cartridge UUID at BOT. A pass applies only to the exact drive, firmware, media, and test evidence.

For backup:

1. browse to an immutable snapshot path;
2. save a profile with automatic Zstd, Zstd, LZ4, GZIP, or no compression;
3. create a request and review server-calculated media readiness;
4. start the destructive action;
5. watch the first DAR pack become ready, then concurrent rolling-spool production and tape writing;
6. supply requested prepared continuation cartridges;
7. wait for immediate full readback of every volume.

A recovery point is created only after every planned native volume passes and the source remains unchanged. This is write/readback evidence, not an in-console restore test.

## Live telemetry

The worker reads physical byte counters from `/sys/class/scsi_tape/stN/stats` for the exact mapped drive when the kernel exposes them. The UI reports source inventory, disk-stage bytes, DAR current file, physical tape write, and physical tape read as separate facts. Missing, first, or stale samples remain unavailable; they are not displayed as zero throughput.

Packing delays indicate source traversal, CPU/compression, or spool I/O. Tape writing can begin after the first validated pack while later packs are produced concurrently. If the ready window drains, TapeScale pauses only at a completed tape-file boundary; repeated draining means sustained pack production is below tape consumption and the source, compression, pack size, and spool throughput need investigation.

## Offline recovery access

The release image includes `tapescale-recovery`. A clean-host operator can capture native volumes and restore a complete DAR archive without PostgreSQL. The worker container itself should not be repurposed as an interactive recovery shell during an active deployment; use a separate isolated host/container with the exact device and dedicated capture/destination mounts.

See [RECOVERY.md](RECOVERY.md) for commands and evidence requirements. In-console native restore remains pending.

## Update the TrueNAS app

1. Wait for all runs and physical tape I/O to finish.
2. Confirm no drive or cartridge has an uncertain write state.
3. Back up PostgreSQL and the recovery evidence volume.
4. Redeploy the installed custom app to build the newest `main` commit.
5. If `compose.truenas.yaml` itself changed, replace the installed YAML with the newest file and reapply only the `x-tapescale-host-settings` values.
6. Retain the resulting image digest and inspect migrations/logs.
7. Re-run preflight and a disposable scratch-media exercise before trusting the new build.

Never point an older application image at a database that has already migrated and call that rollback. Restore a consistent database backup with the matching release instead.

An interrupted partial Native v2 member is not resumed in place by alpha.9. TapeScale quarantines touched media, and retry creates and restages a new archive set. Do not delete a retained failed spool until its diagnostic evidence has been reviewed.

## Security warning

The alpha Compose deliberately embeds a disposable PostgreSQL password and the web application has no authentication or TLS. Anyone who reaches port 18080 can inspect filenames and attempt physical workflows. Use only an isolated attended network, firewall the port, and never load production or unknown tapes.

A production design must add TLS ingress, accounts/RBAC/MFA, external secrets, narrower source mounts, qualified hardware mappings, off-host database/evidence backups, monitoring, and the complete acceptance evidence in [QUALIFICATION.md](QUALIFICATION.md).

## Removal

Before removing the app, stop all jobs at safe boundaries and unload media. Preserve PostgreSQL/evidence volumes and any needed capture data before deleting application storage. Removing containers does not erase tapes; deleting database/evidence volumes can destroy the only convenient map from human labels and custody history to self-identifying archive sets.
