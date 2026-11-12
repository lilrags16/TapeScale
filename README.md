# TapeScale

[![CI](https://github.com/lilrags16/TapeScale/actions/workflows/ci.yml/badge.svg)](https://github.com/lilrags16/TapeScale/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

TapeScale is a safety-first LTO backup control plane for TrueNAS. It is designed to manage file and native ZFS protection, media, restores, verification evidence, and confirmed off-site custody without making the application catalog a single point of recovery failure.

> [!IMPORTANT]
> This repository is a **G0-ready engineering preview**, not a production release. The control plane, safety rules, simulator, API, UI, schema, and recovery tooling are implemented and tested. No tape generation, drive, changer, TrueNAS data path, ACL round trip, engine, or on-tape format is claimed as physically qualified yet. See the [qualification ledger](docs/QUALIFICATION.md).

## What is here

- Separate Rust API, durable scheduler, privileged-worker boundary, and independent recovery CLI
- PostgreSQL migrations for durable jobs, recovery dependencies, media, custody, verification, leases, audit events, and notification outbox state
- Explicit LTO-1 through LTO-10 read/write compatibility, including LTO-7 Type M
- Deny-by-default overwrite evaluation and dependency-aware retention
- Truthful job/verification state machines and safe restore preflight
- A constrained external-tool runner with literal arguments, timeouts, cancellation, bounded output, and failure propagation
- Worker-only SAS discovery/status plus durable, audited lab-only rewind and unload using explicitly mapped non-rewinding devices
- An attended, destructive Bacula Community 15.0.3 `btape test` harness with exact drive/cartridge binding, fencing, retained evidence, and a required rewind-to-BOT postcondition
- A deterministic engine simulator that cannot report physical qualification
- A purpose-built responsive storage-operations console
- Hardened Docker Compose packaging with separate API, worker, and PostgreSQL services
- Architecture, security, deployment, engine-gate, recovery, operations, and acceptance documentation

## Run the engineering preview

Docker Compose is the supported packaging path:

```sh
cp .env.example .env
./scripts/init-secrets.sh
docker compose up --build
```

Open `http://localhost:8080`. A fresh deployment starts without embedded operational fixtures: PostgreSQL and the console are empty until real records are created. Manual cartridge registration is the first durable workflow, and each cartridge remains in PostgreSQL across container restarts.

For a self-contained TrueNAS test deployment, paste [`compose.truenas.yaml`](compose.truenas.yaml) into **Apps → Discover Apps → ⋮ → Install via YAML** and open `http://TRUENAS-IP:18080`. The test file intentionally embeds its disposable database password and uses Docker-managed volumes, but it does not enable demo fixtures. It maps only `/dev/nst0` to the worker and explicitly enables the scratch-media lab gates for status, rewind, unload, and the destructive Bacula `btape test`. If the host uses `/dev/nst1`, set `TAPESCALE_TAPE_DEVICES` and both sides of the device mapping to `/dev/nst1`; the container node name must match Linux sysfs.

> [!CAUTION]
> `btape test` deliberately overwrites the loaded cartridge. Everything previously on that tape must be treated as permanently lost. The TrueNAS test Compose grants `rwm` access to the one named device for this reason. Never select production, retained, foreign, WORM, or irreplaceable media.

To run the first physical engine gate:

1. Load a blank or expendable scratch cartridge and wait for the drive to report ready.
2. Register that physical cartridge under **Tapes & pools** using a unique internal label.
3. Under **Drives & libraries**, trust the exact observed SAS identity if needed and use **Rewind** to establish BOT.
4. Choose **Run btape test**, select the registered cartridge that is physically loaded, accept both destructive confirmations, and type the displayed phrase exactly.
5. Leave the app and drive running while the durable run is `queued` or `running`. TapeScale captures the bounded Bacula output and requires a final rewind with an observed ready-at-BOT postcondition.
6. Review the result under **System & qualification → Qualification**. A pass is G0.1 drive/media evidence only; it is not a successful backup, restore, or completed G0 decision.

The harness does not expose arbitrary commands, label, erase, `fill`, or general backup/restore operations. Bacula's Storage Daemon package is pinned in the image to supply `btape`, but no Storage Daemon is started and no production on-tape format is selected.

To rebuild a TrueNAS custom app from the latest public `main`, open the installed `tapescale` app for editing, replace its YAML with the latest raw [`compose.truenas.yaml`](https://raw.githubusercontent.com/lilrags16/TapeScale/main/compose.truenas.yaml), and save. Its `pull_policy: build` and Git build context cause Compose to fetch and rebuild the current source rather than reuse an older application image.

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
| `tapescale-api` | Web/API, restore planning, operator workflows | No devices, Docker socket, host root, or blanket privilege |
| `tapescale-scheduler` | Timezone-aware occurrences, catch-up/coalescing, durable queue policy | No source, restore, or device access |
| `tapescale-worker` | Durable orchestration, SAS discovery, lab-only rewind/unload, and attended `btape test` execution | Only explicitly mapped non-rewinding tape nodes; destructive data I/O exists solely inside the separately gated, typed-confirmation qualification run |
| `tapescale-recovery` | Clean-host bundle inspection and recovery workflow | Runs independently of the original database |
| PostgreSQL 16 | Product state, history, leases, evidence, audit/outbox | Private Compose network and persistent volume |
| Engine adapter | Physical records, spanning, positioning, read-back | Simulator only until G0 selects and qualifies an engine/format |

The web and worker containers drop all Linux capabilities in the preview. The TrueNAS test worker runs as container root so the exact `/dev/nst0:rwm` mapping can be inspected, moved, and used by the separately gated destructive `btape` harness; it is not privileged and has no broad `/dev`, host-root, or Docker-socket mount. A qualified deployment should instead use the host tape group where possible and must map only named tape/changer devices, read-only source snapshots, and approved restore destinations.

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

SAS discovery, narrowly allowlisted lab rewind/unload, and the first Bacula 15.0.3 destructive drive test are implemented. A passing `btape test` is retained as evidence for the exact drive, firmware, device path, and operator-asserted scratch cartridge, but it does not qualify a backup representation or promote the drive/product to `physically_qualified`. The remaining gate requires real file backup/readback/restore, TrueNAS metadata fidelity, multi-volume/EOM and wrong-media behavior, failure recovery, encryption, and clean-host catalog reconstruction.

## License

TapeScale is licensed under the [Apache License 2.0](LICENSE).

The runtime test image also distributes unmodified Debian Bacula Community packages under AGPLv3 and Bacula's accompanying additional terms/notices. Their installed copyright and license material remains under `/usr/share/doc/bacula-common/`; TapeScale invokes those programs as separate processes and does not relicense them under Apache-2.0. See the [third-party runtime notices](THIRD_PARTY_NOTICES.md) for the exact packaged versions and corresponding source location.
