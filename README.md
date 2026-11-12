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

Open `http://localhost:8080`. The default configuration is explicitly marked simulation mode but still persists platform state in PostgreSQL.

For a self-contained TrueNAS test deployment, paste [`compose.truenas.yaml`](compose.truenas.yaml) into **Apps → Discover Apps → ⋮ → Install via YAML** and open `http://TRUENAS-IP:18080`. The test file intentionally embeds its database password and uses Docker-managed volumes.

To run only the console from a Rust checkout:

```sh
TAPESCALE_DEMO_MODE=true cargo run --bin tapescale-api
```

Then open `http://localhost:8080`.

## Validate the source

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo build --release --locked
```

The current suite exercises compatibility discontinuities, overwrite blockers, retention graphs, incremental-base selection, job transitions, restore prerequisites, path/link containment, RBAC, custody confirmation, engine claim boundaries, and safe process execution.

## Runtime boundaries

| Component | Responsibility | Privilege boundary |
|---|---|---|
| `tapescale-api` | Web/API, restore planning, operator workflows | No devices, Docker socket, host root, or blanket privilege |
| `tapescale-scheduler` | Timezone-aware occurrences, catch-up/coalescing, durable queue policy | No source, restore, or device access |
| `tapescale-worker` | Durable orchestration and engine/device adapter boundary | Devices and approved paths only in a later qualified hardware override |
| `tapescale-recovery` | Clean-host bundle inspection and recovery workflow | Runs independently of the original database |
| PostgreSQL 16 | Product state, history, leases, evidence, audit/outbox | Private Compose network and persistent volume |
| Engine adapter | Physical records, spanning, positioning, read-back | Simulator only until G0 selects and qualifies an engine/format |

The web and worker containers drop all Linux capabilities in the preview. A production hardware override must map only named tape/changer devices, read-only source snapshots, and approved restore destinations; it must not grant the Docker socket, host root, or blanket privileged mode.

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

The next release gate requires a designated TrueNAS 25.10.8 qualification host, real drives/changer and scratch media, a pinned engine candidate, reproducible fixtures, and retained test evidence. Simulated acceptance coverage is useful engineering evidence, but it is never labeled as a physical pass.

## License

TapeScale is licensed under the [Apache License 2.0](LICENSE).
