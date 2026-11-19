# TapeScale

[![CI](https://github.com/lilrags16/TapeScale/actions/workflows/ci.yml/badge.svg)](https://github.com/lilrags16/TapeScale/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

TapeScale is a safety-first LTO backup control plane for TrueNAS. It manages standalone SAS tape drives, cartridges, bounded rolling file archives, physical verification evidence, recovery metadata, retention, holds, and custody through a graphical console.

> [!IMPORTANT]
> Version `0.2.0-alpha.6` is an attended evaluation build, not production protection. New backups use **TapeScale Native v2**, a bounded rolling-spool format; existing Native v1 captures remain readable. Multi-tape writing, immediate physical readback, and database-independent full-archive capture/restore are implemented. In-console restore and selected-file recovery are not. Do not make this alpha the only copy of any data.

## What changed in 0.2

New backups no longer use Bacula. TapeScale now owns the media format and physical workflow while invoking [DAR](https://dar.sourceforge.io/) as a separate archive-codec executable.

The Native v2 path uses independently finalized packs in a bounded spool window:

1. Inventory and SHA-256 hash the selected immutable snapshot.
2. Deterministically group manifest entries into independently recoverable DAR packs, keeping hard links together.
3. Prefill a validated ready-pack reserve to the high-water mark minus one maximum in-progress pack (or finish producing a smaller archive) before the first tape motion.
4. Stream only complete packs to tape, reclaim each pack after its tape files close durably, and refill from low-water to the resume threshold while parked at a clean filemark.
5. Read the loaded tape's preparation UUID and reject the wrong cartridge.
6. Write a TapeScale volume header, source manifest, typed pack descriptors, isolated catalogs, payload slices, volume index, and trailer.
7. Chain continuation-volume indexes and place a final archive-set seal on the last cartridge.
8. Rewind and immediately read every tape file back, validating its digest and identity bindings.
9. Retain a recovery point only after every volume, pack, chain link, and final seal passes physical readback.

Small files are packaged into large sequential files before they reach tape. A 30 TB archive therefore does not need a 30 TB staging disk: only the configured rolling window is retained, and pack production overlaps tape consumption. If sustained production cannot keep up, TapeScale waits between complete pack tape files instead of starving a partially written tape file.

See [TapeScale Native v2 format](docs/NATIVE_FORMAT.md) for the media contract and current recovery limitations.

## Current functionality

- Rust API, scheduler, privileged worker boundary, PostgreSQL state, and responsive web console
- SAS drive discovery with stable SCSI identity and exact `/dev/nstN` allowlisting
- Guided destructive tape preparation with a machine-readable cartridge UUID at beginning of tape
- Native destructive qualification using deterministic records, two tape files, filemarks, rewind, and full hash readback
- Graphical source browser restricted to named ZFS snapshot directories beneath the read-only source mount
- Bounded-memory source inventory and retained-manifest browsing without an artificial total file-count or logical-byte ceiling
- DAR Full archives with automatic fast Zstandard, fast LZ4, balanced GZIP, or no compression
- Bounded rolling-disk staging with 256 GiB target packs, startup prefill, and functional high/low-water refill hysteresis
- Ordered attended multi-cartridge continuation at complete pack boundaries without whole-archive staging
- Versioned self-identifying headers, typed pack records, chained indexes, final archive-set seal, per-file checksums, and per-pack database records
- Immediate physical full readback of every written native volume
- Live inventory, packing, staged-byte, current-file, tape-write, tape-read, and attended-handoff status
- Media pools, retention, holds, overwrite evaluation, reservations, custody movements, audit history, and evidence artifacts

## Deploy on TrueNAS

Create two datasets before installing the app:

- `/mnt/tapescale-spool`: a fast Generic/POSIX dataset used as the bounded rolling-pack window. The release defaults are 768 GiB high-water, 256 GiB low-water, and 8 GiB reserved free space; the user's 1 TB spool is sufficient for those defaults.
- `/mnt/tapescale-restores`: an isolated Generic/POSIX dataset reserved for future restore execution. Do not share it over SMB or NFS.

The spool does not need to match the source size. It must hold the configured high-water window plus reserve and allow the container worker to create, write, and `fsync` files. With the defaults, tape starts or resumes with at least 384 GiB of validated ready packs unless the producer has already finished a smaller archive. One individual file or hard-link allocation group must still fit both the configured pack maximum and a usable cartridge payload.

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

Automatic compression selects fast Zstandard. Compression occurs while building each disk pack, not while feeding that pack to tape. Packs contain fixed-size DAR slices and are assigned whole to volumes; an archive can span as many eligible prepared cartridges as its plan requires without staging the complete set.

Cancellation before physical writing leaves the selected tape unchanged. Once a media write starts, cancellation is cooperative and cannot stop kernel tape I/O instantaneously. An interrupted or failed write leaves that cartridge uncertain and no recovery point is created from it. Alpha.6 does not resume a partially written member; retry creates and restages a new archive set.

## Understanding progress

The running-job page separates counters that mean different things:

- **Inventory** reports source entries and logical bytes discovered.
- **Packing** reports the active pack, bounded spool occupancy, high/low watermarks, reclaimed bytes, and the source path currently reported by DAR.
- **Tape write** and **Tape read** use physical counters for the exact drive.
- **Volume position** reports the current archive volume and completed native tape files.
- **Waiting for tape** names the required continuation and does not consume the active-transfer timeout.

Source logical bytes, compressed archive bytes, and physical device bytes are deliberately not treated as interchangeable. Sparse files, hard links, metadata, compression, padding, headers, and readback make a single synthetic byte percentage misleading.

## Format transition

`tapescale-native-v2` is the default for new operational backups. It is incompatible with Native v1 at the write-layout level, but `tapescale-recovery` retains explicit v1 and v2 readers. Both native formats are incompatible with prior Bacula-written experiments. During upgrade:

- Saved backup profiles migrate to Native v2.
- Untouched queued Native v1 backup requests migrate to v2. The separate G0.2 restore-comparison exercise remains v1 until that workflow has a v2 executor.
- Running, terminal, or media-touched Native v1 rows keep their original format and remain readable as v1 history.
- Failed or media-touched Bacula attempts remain audit history only and are not presented as recoverable archives.
- Scratch media must pass native preparation before its next write.

Do not run preparation on any older cartridge whose contents must be recovered. The main runtime no longer supplies a Bacula recovery path.

## Alpha limitations

- In-console restore and selected-file extraction are not yet connected to the product worker.
- Clean-host recovery is an attended CLI workflow: capture every ordered tape with `tapescale-recovery capture-native-volume`, then run `restore-native-archive` into an empty isolated destination. It is not yet physically qualified across the supported hardware matrix.
- Immediate readback proves that the exact bytes written to each volume can be read and authenticated; it does not prove that a restored filesystem matches the source.
- Only attended, manually started Full file archives on a standalone drive are operational.
- Tape libraries/changers, automatic snapshot creation, incrementals, encryption/key recovery, and native ZFS send streams are unavailable.
- A single file or hard-link allocation group larger than the configured pack maximum or usable cartridge payload is rejected before tape write; native file-chunk records are not implemented yet.
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

- [TapeScale Native v2 format](docs/NATIVE_FORMAT.md)
- [Architecture and authority boundaries](docs/ARCHITECTURE.md)
- [Tape engine decision record](docs/ENGINE-EVALUATION.md)
- [Security and destructive-action gates](docs/SECURITY.md)
- [Qualification ledger](docs/QUALIFICATION.md)
- [Independent recovery runbook](docs/RECOVERY.md)
- [TrueNAS deployment](docs/TRUENAS-DEPLOYMENT.md)
- [Operator workflows](docs/OPERATIONS.md)
- [Product design system](docs/design-system.md)

The qualification ledger and engine decision record describe the active Native v2 architecture and keep physical evidence distinct from implemented software behavior.

## Contributing and security

Contributions are welcome. Start with [CONTRIBUTING.md](CONTRIBUTING.md) and follow the evidence and safety requirements. Report vulnerabilities through the private process in [SECURITY.md](SECURITY.md), not a public issue.

## License

TapeScale is licensed under the [Apache License 2.0](LICENSE). The runtime invokes unmodified DAR and supporting libraries under their own licenses as separate programs; see [third-party runtime notices](THIRD_PARTY_NOTICES.md).
