# TapeScale

[![CI](https://github.com/lilrags16/TapeScale/actions/workflows/ci.yml/badge.svg)](https://github.com/lilrags16/TapeScale/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

TapeScale is a safety-first LTO backup control plane for TrueNAS. It manages standalone SAS tape drives, cartridges, staged file archives, physical verification evidence, recovery metadata, retention, holds, and custody through a graphical console.

> [!IMPORTANT]
> Version `0.2.0-alpha.4` is an attended evaluation build, not production protection. It uses the incompatible **TapeScale Native v1** format. Native archive creation, multi-tape writing, immediate physical readback, and database-independent full-archive capture/restore are implemented. In-console restore and selected-file recovery are not. Do not make this alpha the only copy of any data.

## What changed in 0.2

New backups no longer use Bacula. TapeScale now owns the media format and physical workflow while invoking [DAR](https://dar.sourceforge.io/) as a separate archive-codec executable.

The native path is intentionally stage-first:

1. Inventory and SHA-256 hash the selected immutable snapshot.
2. Pack files and metadata into large compressed DAR slices on the spool filesystem.
3. Test the completed staged archive before moving tape.
4. Plan whole slices across one or more cartridges.
5. Read the loaded tape's preparation UUID and reject the wrong cartridge.
6. Write a TapeScale volume header, recovery catalog, source manifest, slices, volume index, and trailer.
7. Rewind and immediately read every tape file back, validating its digest and identity bindings.
8. Retain a recovery point only after every planned volume passes physical readback.

This separates slow small-file traversal and compression from tape motion. Once writing begins, the drive reads already completed large files from the spool instead of waiting on ZFS directory walks or a per-file backup engine.

See [TapeScale Native v1 format](docs/NATIVE_FORMAT.md) for the media contract and current recovery limitations.

## Current functionality

- Rust API, scheduler, privileged worker boundary, PostgreSQL state, and responsive web console
- SAS drive discovery with stable SCSI identity and exact `/dev/nstN` allowlisting
- Guided destructive tape preparation with a machine-readable cartridge UUID at beginning of tape
- Native destructive qualification using deterministic records, two tape files, filemarks, rewind, and full hash readback
- Graphical source browser restricted to named ZFS snapshot directories beneath the read-only source mount
- Bounded-memory source inventory and retained-manifest browsing without an artificial total file-count or logical-byte ceiling
- DAR Full archives with automatic fast Zstandard, fast LZ4, balanced GZIP, or no compression
- Full-disk staging and validation before tape motion
- Ordered attended multi-cartridge continuation at complete archive-slice boundaries
- Versioned self-identifying headers, indexes, trailers, per-file checksums, and per-segment database records
- Immediate physical full readback of every written native volume
- Live inventory, packing, staged-byte, current-file, tape-write, tape-read, and attended-handoff status
- Media pools, retention, holds, overwrite evaluation, reservations, custody movements, audit history, and evidence artifacts

## Deploy on TrueNAS

Create two datasets before installing the app:

- `/mnt/tapescale-spool`: a large, fast Generic/POSIX dataset used for complete archive staging. NVMe or SSD is strongly preferred.
- `/mnt/tapescale-restores`: an isolated Generic/POSIX dataset reserved for future restore execution. Do not share it over SMB or NFS.

The spool must be large enough for the complete encoded archive plus the configured free-space reserve. It must allow the container worker to create, write, and `fsync` files. TapeScale checks free space before packing and does not intentionally begin a tape pass with an incomplete archive.

In TrueNAS SCALE:

1. Open **Apps → Discover Apps → ⋮ → Install via YAML**.
2. Paste the complete [`compose.truenas.yaml`](compose.truenas.yaml).
3. Confirm the mapped non-rewinding tape device. The default is `/dev/nst0`; if the host uses another device, update both `TAPESCALE_TAPE_DEVICES` and the device mapping to the same exact path.
4. Install the app and open `http://TRUENAS-IP:18080`.

The alpha Compose file intentionally uses a disposable embedded database password for an isolated test environment. It maps host `/mnt` read-only as `/source`, maps only the spool and restore datasets writable to the worker, exposes one named non-rewinding tape device, and does not mount the Docker socket or host root.

To update the custom app, edit the installed app, replace its YAML with the latest [`compose.truenas.yaml`](https://raw.githubusercontent.com/lilrags16/TapeScale/main/compose.truenas.yaml), and save. Release Compose files pin their Git build context and OCI revision to an immutable tag.

## Prepare a tape

> [!CAUTION]
> **Prepare tape destroys everything on the loaded cartridge.** Use only a blank or expendable scratch tape. Never prepare retained, foreign, WORM, production, or irreplaceable media.

1. Load one blank or expendable tape and wait for the drive to report ready.
2. Open **Prepare tape** from the drive or tape view.
3. Register or select its human-readable inventory label and LTO generation.
4. Review the detected drive and cartridge, then choose **Prepare tape** once.
5. Leave the worker and drive running until qualification and identity readback complete.

Qualification writes deterministic 256 KiB records across two tape files, rewinds, verifies every record and filemark, then writes a `TapeScale Native Preparation` header containing the durable cartridge UUID. Later discovery and destructive boundaries read that UUID from tape; the operator does not need to repeatedly type or attest the loaded identity.

A qualification pass applies only to the exact recorded drive, firmware, media generation, and test. It is not backup or restore proof.

## Run a native backup

The current backup path protects one existing immutable snapshot with a manually started Full archive:

1. In **Backups**, choose a directory inside `.zfs/snapshot/<snapshot-name>` with the graphical browser and save the selection.
2. Choose compression, retention, media policy, and an optional prepared-media pool.
3. Review the server-selected drive and the cartridge identity read directly from tape.
4. Choose the destructive start action. TapeScale repeats eligibility, reservation, lease, source, identity, hold, and overwrite checks under lock and at the physical boundary.
5. Follow the run page through inventory, packing, validation, each tape write, and immediate readback.
6. When requested, unload the completed volume and load the next named prepared cartridge. TapeScale verifies its preparation UUID before accepting or writing it.
7. After every volume passes, browse the retained source manifest and inspect the recorded volume and evidence set.

Automatic compression selects fast Zstandard. Compression occurs while building the disk archive, not while feeding tape. DAR slices are configured independently of cartridge size and are assigned whole to volumes; an archive can span as many eligible prepared cartridges as its plan requires.

Cancellation before physical writing leaves the selected tape unchanged. Once a media write starts, cancellation is cooperative and cannot stop kernel tape I/O instantaneously. An interrupted or failed write leaves that cartridge uncertain and no recovery point is created from it.

## Understanding progress

The running-job page separates counters that mean different things:

- **Inventory** reports source entries and logical bytes discovered.
- **Packing** reports encoded bytes staged on disk and the source path currently reported by DAR.
- **Tape write** and **Tape read** use physical counters for the exact drive.
- **Volume position** reports the current archive volume and completed native tape files.
- **Waiting for tape** names the required continuation and does not consume the active-transfer timeout.

Source logical bytes, compressed archive bytes, and physical device bytes are deliberately not treated as interchangeable. Sparse files, hard links, metadata, compression, padding, headers, and readback make a single synthetic byte percentage misleading.

## Format transition

`tapescale-native-v1` is incompatible with prior Bacula-written experiments. This release assumes there are no valid Bacula recovery tapes to preserve:

- Saved backup profiles migrate to the native engine.
- Untouched queued requests migrate to native.
- Failed or media-touched Bacula attempts remain audit history only and are not presented as recoverable archives.
- Scratch media must pass native preparation before its next write.

Do not run preparation on any older cartridge whose contents must be recovered. The main runtime no longer supplies a Bacula recovery path.

## Alpha limitations

- In-console restore and selected-file extraction are not yet connected to the product worker.
- Clean-host recovery is an attended CLI workflow: capture every ordered tape with `tapescale-recovery capture-native-volume`, then run `restore-native-archive` into an empty isolated destination. It is not yet physically qualified across the supported hardware matrix.
- Immediate readback proves that the exact bytes written to each volume can be read and authenticated; it does not prove that a restored filesystem matches the source.
- Only attended, manually started Full file archives on a standalone drive are operational.
- Tape libraries/changers, automatic snapshot creation, incrementals, encryption/key recovery, and native ZFS send streams are unavailable.
- Real hardware qualification remains required for supported drive generations, compression modes, spanning, wrong-media handling, interruptions, ACLs, xattrs, sparse files, hard links, and unusual filenames.
- The web alpha has no application authentication or TLS termination. Run it only on an isolated trusted network.

## Local development

The generic Compose stack runs without a physical tape mapping and is suitable for UI/API development:

```sh
cp .env.example .env
./scripts/init-secrets.sh
mkdir -p restore
docker compose up --build
```

Open `http://localhost:8080`. A fresh database contains no fake operational records.

Run source validation with:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --release --locked
node --check web/app.js
```

## Runtime boundaries

| Component | Responsibility | Privilege boundary |
|---|---|---|
| `tapescale-api` | Web/API, graphical browsing, media and operator workflows | Read-only source view; no tape device or Docker socket |
| `tapescale-scheduler` | Durable scheduling, reconciliation, and waiting work | No source, restore, or device access |
| `tapescale-worker` | Source inventory, DAR staging/testing, SAS operations, native tape writing/readback | Exact tape node, read-only source, writable dedicated spool and restore roots, narrowly scoped filesystem capabilities |
| `tapescale-recovery` | `capture-native-volume` verifies and stages each tape; `restore-native-archive` validates the complete set, extracts with pinned DAR, and compares the restored tree with the authenticated manifest | Runs independently of PostgreSQL; requires an attended clean host, every ordered cartridge, an empty destination, and physical qualification |
| PostgreSQL 16 | Product state, leases, identities, evidence, audit and outbox | Private Compose network and persistent volume |
| DAR 2.8.6 | Separately executed file-archive codec | Receives closed literal arguments and operates inside the worker's source/spool boundary; it does not own cartridge identity or tape motion |

## Documentation

- [TapeScale Native v1 format](docs/NATIVE_FORMAT.md)
- [Architecture and authority boundaries](docs/ARCHITECTURE.md)
- [Tape engine decision record](docs/ENGINE-EVALUATION.md)
- [Security and destructive-action gates](docs/SECURITY.md)
- [Qualification ledger](docs/QUALIFICATION.md)
- [Independent recovery runbook](docs/RECOVERY.md)
- [TrueNAS deployment](docs/TRUENAS-DEPLOYMENT.md)
- [Operator workflows](docs/OPERATIONS.md)
- [Product design system](docs/design-system.md)

The qualification ledger and engine decision record describe the active Native v1 architecture and keep physical evidence distinct from implemented software behavior.

## Contributing and security

Contributions are welcome. Start with [CONTRIBUTING.md](CONTRIBUTING.md) and follow the evidence and safety requirements. Report vulnerabilities through the private process in [SECURITY.md](SECURITY.md), not a public issue.

## License

TapeScale is licensed under the [Apache License 2.0](LICENSE). The runtime invokes unmodified DAR and supporting libraries under their own licenses as separate programs; see [third-party runtime notices](THIRD_PARTY_NOTICES.md).
