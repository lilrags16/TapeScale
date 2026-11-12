# Contributing to TapeScale

Thank you for helping build TapeScale. The project welcomes focused bug fixes, tests, documentation, design work, and carefully scoped features.

TapeScale controls workflows that can overwrite media and determine whether backups are recoverable. Contributions must preserve the distinction between modeled capability, simulated evidence, and physical qualification.

Participation in this project is governed by the [Code of Conduct](CODE_OF_CONDUCT.md).

## Before opening a change

- Search existing issues and pull requests for related work.
- Open an issue before starting a broad architectural change.
- Never include real credentials, customer names, private dataset paths, tape contents, recovery keys, or unsanitized diagnostics.
- Do not describe simulator output as physical qualification or a successful production backup.
- Keep changes compatible with the authority boundaries in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and the security contract in [docs/SECURITY.md](docs/SECURITY.md).

## Development setup

The Rust toolchain is pinned in `rust-toolchain.toml`. Docker Compose is used for the complete local stack.

```sh
cp .env.example .env
./scripts/init-secrets.sh
docker compose up --build
```

For source-only development:

```sh
cargo build --locked
TAPESCALE_DEMO_MODE=true cargo run --bin tapescale-api
```

## Required checks

Run these before submitting a pull request:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets --locked
cargo build --release --bins --locked
docker build --tag tapescale:contributor .
```

Add or update tests for behavior changes. Update operational and qualification documentation when a change affects safety gates, persisted state, recovery, media handling, privileges, or deployment.

## Database changes

Committed migrations are append-only. Do not edit a migration that may have run on another system. Add a new numbered migration and document its forward and recovery behavior.

## Pull requests

Keep pull requests small enough to review. Explain:

- the problem and intended outcome;
- safety, privilege, persistence, and recovery effects;
- tests and evidence produced;
- any remaining gaps or unqualified behavior.

Pull requests must not weaken deny-by-default overwrite rules, path containment, least-privilege boundaries, audit history, or truthful qualification labels without an explicit design decision and corresponding tests.

By contributing, you agree that your contribution is licensed under the Apache License 2.0.
