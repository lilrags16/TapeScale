# TapeScale

[![CI](https://github.com/lilrags16/TapeScale/actions/workflows/ci.yml/badge.svg)](https://github.com/lilrags16/TapeScale/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

TapeScale is a safety-first LTO backup control plane for TrueNAS. It is designed to manage file and native ZFS protection, media, restores, verification evidence, and confirmed off-site custody without making the application catalog a single point of recovery failure.

> [!IMPORTANT]
> This repository is a **G0 engineering preview**, not a production release. It now includes an attended G0.2 single-cartridge Bacula file roundtrip, but no tape generation, drive, changer, TrueNAS data path, ACL roundtrip, engine, or on-tape format is claimed as generally qualified. A pass applies only to the exact recorded source snapshot, drive, firmware, cartridge, and run. See the [qualification ledger](docs/QUALIFICATION.md).

## What is here

- Separate Rust API, durable scheduler, privileged-worker boundary, and independent recovery CLI
- PostgreSQL migrations for durable jobs, recovery dependencies, media, custody, verification, leases, audit events, and notification outbox state
- Explicit LTO-1 through LTO-10 read/write compatibility, including LTO-7 Type M
- Deny-by-default overwrite evaluation and dependency-aware retention
- Truthful job/verification state machines and safe restore preflight
- A constrained external-tool runner with literal arguments, timeouts, cancellation, bounded output, and failure propagation
- Worker-only SAS discovery/status plus durable, audited lab-only rewind and unload using explicitly mapped non-rewinding devices
- An attended, destructive Bacula Community 15.0.3 `btape test` harness with exact drive/cartridge binding, fencing, retained evidence, and a required rewind-to-BOT postcondition
- An attended G0.2 recovery exercise that inventories an immutable mounted snapshot, writes one full file backup through Bacula, performs Bacula Data verification from tape, restores to an isolated destination, and compares independent SHA-256 manifests before creating a verified recovery point
- A Recovery Vault that joins each recovery point to its ordered cartridges, locations, engine record boundaries, dependencies, holds, verification history, originating run, event trail, and content-addressed artifacts
- Downloadable bounded recovery-evidence reports plus durable, audited restore-readiness plans with explicit media, drive, location, key, destination, capacity, and verification blockers
- Transactional off-site custody operations with named locations, exact cartridge reservations, partial departure/arrival/direct confirmations, append-only corrections, idempotent receipts, and overdue notifications
- A deterministic engine simulator that cannot report physical qualification
- A purpose-built responsive storage-operations console
- Hardened Docker Compose packaging with separate API, worker, and PostgreSQL services
- Architecture, security, deployment, engine-gate, recovery, operations, and acceptance documentation

## Run the engineering preview

Docker Compose is the supported packaging path:

```sh
cp .env.example .env
./scripts/init-secrets.sh
mkdir -p restore
docker compose up --build
```

Open `http://localhost:8080`. A fresh deployment starts without embedded operational fixtures: PostgreSQL and the console are empty until real records are created. Manual cartridge registration is the first durable workflow, and each cartridge remains in PostgreSQL across container restarts.

The generic `compose.yaml` intentionally maps no tape device and keeps all physical lab flags disabled. Enabling those flags outside the TrueNAS test file also requires an explicit override that maps the exact non-rewinding `/dev/nstN` node into the worker; never enable a guessed `/dev/sg*` device or add blanket privilege.

For a self-contained TrueNAS test deployment, first create the isolated dataset/path `/mnt/tapescale-restores` with a Generic/POSIX-compatible ACL preset and no share, then paste [`compose.truenas.yaml`](compose.truenas.yaml) into **Apps → Discover Apps → ⋮ → Install via YAML** and open `http://TRUENAS-IP:18080`. The test file intentionally embeds its disposable database password and uses Docker-managed state volumes, but it does not enable demo fixtures. It exposes host `/mnt` to the API and worker as read-only `/source`, exposes only `/mnt/tapescale-restores` as writable `/restore` to the worker, maps `/dev/nst0`, and enables the attended scratch-media gates. If the host uses `/dev/nst1`, set `TAPESCALE_TAPE_DEVICES` and both sides of the device mapping to `/dev/nst1`; the container node name must match Linux sysfs.

> [!CAUTION]
> `btape test` deliberately overwrites the loaded cartridge. Everything previously on that tape must be treated as permanently lost. The TrueNAS test Compose grants `rwm` access to the one named device for this reason. Never select production, retained, foreign, WORM, or irreplaceable media.

To run the first physical engine gate:

1. Load a blank or expendable scratch cartridge and wait for the drive to report ready.
2. Register that physical cartridge under **Tapes & pools** using a unique internal label.
3. Under **Drives & libraries**, trust the exact observed SAS identity if needed and use **Rewind** to establish BOT.
4. Choose **Run btape test**, select the registered cartridge that is physically loaded, accept both destructive confirmations, and type the displayed phrase exactly.
5. Leave the app and drive running while the durable run is `queued` or `running`. TapeScale captures the bounded Bacula output and requires a final rewind with an observed ready-at-BOT postcondition.
6. Review the result under **System & qualification → Qualification**. A pass is G0.1 drive/media evidence only; it is not a successful backup, restore, or completed G0 decision.

After that exact drive/cartridge combination passes, run the first recoverable-tape exercise:

1. Create or select an immutable TrueNAS snapshot below `/mnt`, for example `pool/dataset/.zfs/snapshot/tapescale-g02`, and keep the same qualified scratch cartridge loaded.
2. Open **Recovery lab**, create a profile with the snapshot path relative to `/mnt` and an isolated restore subpath below `/mnt/tapescale-restores`.
3. Select the trusted drive and registered cartridge. Confirm scratch use, destructive overwrite, and the explicit G0.2 deviation that encryption is disabled.
4. Type `WRITE <profile name> TO <cartridge label> AND RESTORE VERIFY` exactly and start the exercise.
5. Leave the worker, database, drive, and mounts available. The run is not passed until Bacula completes the full write, a Data verification that rereads tape, a normal restore, and TapeScale's independent manifest comparison.
6. Inspect the phase timeline, engine evidence, source/restored manifests, comparison, and final recovery point. Restored files remain isolated below `/mnt/tapescale-restores/<chosen subpath>/<run id>`; do not publish them as a share.

After the exercise passes, open **Restore → Recovery vault**. Select the retained recovery point to inspect the exact tape and record range, full-readback and restore-test evidence, originating job, and artifact digests. **Download evidence** produces a bounded JSON attachment without exposing the worker's artifact paths. **Plan restore** persists a 30-minute readiness preview and reports every missing prerequisite. It does not yet dispatch a second tape read or write a restore destination; the result says this explicitly.

Use **Movements** to establish the cartridge's first confirmed location and manage later on-site/off-site handling. Create named locations, plan an exact cartridge set, then confirm a direct move or the separate departure and arrival steps. A plan or due date never changes physical custody. Partial confirmations keep the task open, each mutation is idempotent and audited, and overdue work remains overdue until every required cartridge arrives or the task is explicitly cancelled.

The closed G0.2 path starts the pinned Bacula Director, Storage Daemon, and File Daemon only on worker loopback under the singleton worker's fenced run. It does not expose an operator console or arbitrary engine commands. The current laboratory fixture limit is 100,000 manifest entries and 100 GiB logical data; larger datasets are rejected before tape writing. It is limited to one full file backup on one standalone drive and one cartridge. Spanning, incrementals, changers, native ZFS streams, encryption/key recovery, and clean-catalog recovery remain blocked.

To update the TrueNAS custom app, open the installed `tapescale` app for editing, replace its YAML with the latest raw [`compose.truenas.yaml`](https://raw.githubusercontent.com/lilrags16/TapeScale/main/compose.truenas.yaml), and save. Each physical-test file pins its Git build context and OCI revision to a release tag; `pull_policy: build` rebuilds that exact source instead of silently moving with `main`.

To run an empty, read-only console from a Rust checkout without PostgreSQL:

```sh
TAPESCALE_DEMO_MODE=true cargo run --bin tapescale-api
```

Then open `http://localhost:8080`. Use the Compose stack when you want durable writes such as cartridge registration.

## Validate the source

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo build --release --locked
```

The current suite exercises SAS VPD identity/status parsing, allowlisted motion commands, durable drive-operation leases and audit state, device replacement, reset-position recovery, uncertain-motion gates, compatibility discontinuities, overwrite blockers, retention graphs, incremental-base selection, job transitions, restore prerequisites, path/link containment, RBAC, custody confirmation, engine claim boundaries, and safe process execution.

## Runtime boundaries

| Component | Responsibility | Privilege boundary |
|---|---|---|
| `tapescale-api` | Web/API, restore planning, operator workflows | Read-only source/restore views and `DAC_READ_SEARCH` in the test stack; no devices, Docker socket, writable host path, or blanket privilege |
| `tapescale-scheduler` | Timezone-aware occurrences, catch-up/coalescing, durable queue policy | No source, restore, or device access |
| `tapescale-worker` | Durable orchestration, SAS discovery, rewind/unload, `btape`, and the fenced G0.2 Bacula write/readback/restore exercise | Exact tape node, read-only source tree, isolated writable restore root, and narrowly required metadata-preservation capabilities in the test stack |
| `tapescale-recovery` | Clean-host bundle inspection and recovery workflow | Runs independently of the original database |
| PostgreSQL 16 | Product state, history, leases, evidence, audit/outbox | Private Compose network and persistent volume |
| Engine adapter | Fixed Bacula 15.0.3 configuration, restricted console commands, physical write, Data verify, and restore correlation | G0.2 single-volume lab scope only; the general production engine/format gate remains open |

The scheduler drops all Linux capabilities. The TrueNAS test API runs as container root with only `DAC_READ_SEARCH` so profile preflight can traverse restrictive dataset ACLs through read-only mounts. The worker runs as container root with only `CHOWN`, `DAC_OVERRIDE`, `DAC_READ_SEARCH`, `FOWNER`, and `FSETID`, which Bacula needs to read protected source metadata and restore recorded ownership/mode bits. Neither service is privileged; only the worker has the exact `/dev/nst0:rwm` mapping, `/mnt` remains read-only, and neither has a host-root or Docker-socket mount. A qualified deployment should move filesystem proof into the worker, narrow the source bind to selected snapshots, and use the host tape group where possible.

## Recovery CLI

```sh
cargo run --bin tapescale-recovery -- runbook
cargo run --bin tapescale-recovery -- qualification
cargo run --bin tapescale-recovery -- inspect-bundle ./recovery-bundle/manifest.json
```

Bundle inspection validates format, size, contained artifact paths, SHA-256 declarations, and required software metadata without requiring the original TapeScale database. Physical media import remains blocked until the engine and format complete G0.

## Documentation

- [Architecture and authority boundaries](docs/ARCHITECTURE.md)
- [Security model and destructive-action gates](docs/SECURITY.md)
- [AT-01 through AT-31 qualification ledger](docs/QUALIFICATION.md)
- [Tape-engine evaluation gate](docs/ENGINE-EVALUATION.md)
- [Independent recovery runbook](docs/RECOVERY.md)
- [TrueNAS deployment and privilege mapping](docs/TRUENAS-DEPLOYMENT.md)
- [Operator workflows](docs/OPERATIONS.md)
- [Product design system](docs/design-system.md)

## Contributing and security

Contributions are welcome. Start with [CONTRIBUTING.md](CONTRIBUTING.md), which explains the safety and evidence standards expected for changes, and follow the [Code of Conduct](CODE_OF_CONDUCT.md).

Please do not report vulnerabilities in a public issue. Follow the private process in [SECURITY.md](SECURITY.md). The product threat model and production security contract remain documented separately in [docs/SECURITY.md](docs/SECURITY.md).

## Project status

SAS discovery, narrowly allowlisted lab rewind/unload, the Bacula 15.0.3 destructive drive test, the G0.2 single-volume full-file roundtrip, Recovery Vault/evidence export, durable restore-readiness planning, and confirmed media-custody operations are implemented. A G0.2 pass can create a verified recovery point only for its exact immutable source, physical tape readback, isolated restore, and manifest comparison. A readiness plan is not reusable restore execution. This does not promote the product to production or prove TrueNAS NFSv4 ACL fidelity, multi-volume/EOM behavior, wrong-media handling, interruption recovery, encryption, native ZFS, clean-host catalog reconstruction, or a second independent tape restore after restart.

## License

TapeScale is licensed under the [Apache License 2.0](LICENSE).

The runtime test image also distributes unmodified Debian Bacula Community packages under AGPLv3 and Bacula's accompanying additional terms/notices. Their installed copyright and license material remains under `/usr/share/doc/bacula-common/`; TapeScale invokes those programs as separate processes and does not relicense them under Apache-2.0. See the [third-party runtime notices](THIRD_PARTY_NOTICES.md) for the exact packaged versions and corresponding source location.
