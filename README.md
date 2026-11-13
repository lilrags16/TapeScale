# TapeScale

[![CI](https://github.com/lilrags16/TapeScale/actions/workflows/ci.yml/badge.svg)](https://github.com/lilrags16/TapeScale/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

TapeScale is a safety-first LTO backup control plane for TrueNAS. It is designed to manage file and native ZFS protection, media, restores, verification evidence, and confirmed off-site custody without making the application catalog a single point of recovery failure.

> [!IMPORTANT]
> TapeScale is now an **alpha release**, intended for attended evaluation on an isolated network—not production protection. The alpha includes a usable Backup Center for compressed or uncompressed full-file writes across an ordered set of standalone-drive cartridges, plus a narrower single-cartridge G0.2 restore-comparison exercise, through the pinned Bacula adapter. No tape generation, drive, changer, TrueNAS data path, ACL roundtrip, engine, compression mode, spanning behavior, or on-tape format is claimed as generally qualified. A verified result applies only to the exact recorded source snapshot, drive, firmware, cartridge set, and run. See the [qualification ledger](docs/QUALIFICATION.md).

## What is here

- Separate Rust API, durable scheduler, privileged-worker boundary, and independent recovery CLI
- PostgreSQL migrations for durable jobs, recovery dependencies, media, custody, verification, leases, audit events, and notification outbox state
- Explicit LTO-1 through LTO-10 read/write compatibility, including LTO-7 Type M
- Deny-by-default overwrite evaluation and dependency-aware retention
- Truthful job/verification state machines and safe restore preflight
- A constrained external-tool runner with literal arguments, timeouts, cancellation, bounded output, and failure propagation
- Worker-only SAS discovery/status plus durable, audited lab-only rewind and unload using explicitly mapped non-rewinding devices
- An attended, destructive Bacula Community 15.0.3 `btape test` harness with exact drive/cartridge binding, fencing, retained evidence, and a required rewind-to-BOT postcondition
- A guided **Prepare tape** workflow that detects a safe loaded drive, trusts its exact identity when required, registers or selects the cartridge, retains the physical-label assertion and destructive phrase, and opens an idempotent durable `btape` run without sending the operator through separate pages
- An operational Backup Center that guides an operator from one saved existing snapshot through server-calculated media readiness, exact attended drive/cartridge authorization, selectable automatic/GZIP/LZO/no compression, ordered manual volume changes, a dedicated progress view, Bacula write plus physical full readback, and atomically retained recovery points; rolling snapshot selection remains future work
- A contained graphical source browser that shows folders and file metadata beneath the read-only source mount while permitting only a named ZFS snapshot directory to become an operational backup scope
- An attended G0.2 recovery exercise that inventories an immutable mounted snapshot, writes one full file backup through Bacula, performs Bacula Data verification from tape, restores to an isolated destination, and compares independent SHA-256 manifests before creating a verified recovery point
- A Recovery Vault that joins each recovery point to its ordered cartridges, locations, engine record boundaries, dependencies, holds, verification history, originating run, event trail, and content-addressed artifacts, with a graphical browse-only view of the integrity-checked retained file manifest
- Downloadable bounded recovery-evidence reports plus durable, audited restore-readiness plans with explicit media, drive, location, key, destination, capacity, and verification blockers
- An attended verified-point recovery rehearsal that can re-read one retained G0.2 full-file point from its exact cartridge after the original exercise, restore it into a new isolated directory, and independently compare it with the retained source manifest
- Transactional off-site custody operations with named locations, exact cartridge reservations, partial departure/arrival/direct confirmations, append-only corrections, idempotent receipts, and overdue notifications
- A Media Control Center with searchable cartridge inventory, pool assignment, bounded cartridge dossiers with explicit truncation flags, manual/legal/archive holds, audited hold release, and a selected-drive reuse evaluation that fails closed on every uncertain or protected condition
- A deterministic engine simulator that cannot report physical qualification
- A purpose-built responsive storage-operations console
- Hardened Docker Compose packaging with separate API, worker, and PostgreSQL services
- Architecture, security, deployment, engine-gate, recovery, operations, and acceptance documentation

## Run the alpha

Docker Compose is the supported packaging path:

```sh
cp .env.example .env
./scripts/init-secrets.sh
mkdir -p restore
docker compose up --build
```

Open `http://localhost:8080`. A fresh deployment starts without embedded operational fixtures: PostgreSQL and the console are empty until real records are created. Manual cartridge registration is the first durable workflow, and each cartridge remains in PostgreSQL across container restarts.

The generic `compose.yaml` intentionally maps no tape device and keeps all physical-media flags disabled. Enabling those flags outside the TrueNAS alpha file also requires an explicit override that maps the exact non-rewinding `/dev/nstN` node into the worker; never enable a guessed `/dev/sg*` device or add blanket privilege.

For the self-contained TrueNAS alpha deployment, first create the isolated dataset/path `/mnt/tapescale-restores` with a Generic/POSIX-compatible ACL preset and no share, then paste [`compose.truenas.yaml`](compose.truenas.yaml) into **Apps → Discover Apps → ⋮ → Install via YAML** and open `http://TRUENAS-IP:18080`. The alpha file intentionally embeds its disposable database password and uses Docker-managed state volumes, but it does not enable demo fixtures. It exposes host `/mnt` to the API and worker as read-only `/source`, exposes only `/mnt/tapescale-restores` as writable `/restore` to the worker, maps `/dev/nst0`, and enables the attended scratch-media gates. If the host uses `/dev/nst1`, set `TAPESCALE_TAPE_DEVICES` and both sides of the device mapping to `/dev/nst1`; the container node name must match Linux sysfs.

> [!CAUTION]
> `btape test` deliberately overwrites the loaded cartridge. Everything previously on that tape must be treated as permanently lost. The TrueNAS alpha Compose grants `rwm` access to the one named device for this reason. Never select production, retained, foreign, WORM, or irreplaceable media.

To run the first physical engine gate, use **Prepare tape** from **Drives & libraries** or **Tapes & pools**:

1. Load a blank or expendable scratch cartridge and wait for the drive to report ready.
2. Select the current exact drive. The wizard can perform the same explicit stable-identity trust operation when it is safe; it never guesses through a stale, conflicting, or inaccessible device.
3. Register the printed cartridge label and generation inline, or select the matching existing unprotected record. A standalone drive cannot discover that printed label, so physically verify the exact loaded cartridge.
4. Accept both destructive confirmations and type `DESTROY <label> WITH BACULA BTAPE TEST` exactly. The wizard then queues the existing fenced qualification workflow; it does not weaken or bypass any API/worker revalidation.
5. Leave the app and drive running while the durable run is `queued` or `running`. The wizard and Qualification view retain progress and evidence. A pass is narrow G0.1 evidence only; it is not a successful backup, restore, or completed G0 decision.

After the exact drive/cartridge combination passes, open **Backups** for a durable attended full backup:

This path supports one manually started Full file backup of one fixed existing snapshot. Bacula may write it across as many qualified scratch cartridges as the data requires in one standalone drive, pausing for an attended, ordered tape change at each end-of-media boundary, and then requests the same volumes in order for complete physical readback. The backup profile can select automatic, GZIP, LZO, or no Bacula compression. Encryption remains unavailable. TapeScale no longer imposes a total file-count or logical-byte policy ceiling on this operational inventory; storage, filesystem, engine, hardware, and available-media limits still apply. It does not create the snapshot, run incrementals, or prove a restore.

1. Choose **Choose snapshot**, name the backup, and use **Choose folder** to navigate `/source`. Folders and bounded file metadata are visible; select one existing named directory inside `.zfs/snapshot/<snapshot-name>` as the recursive Full-backup scope. Saving it does not create, advance, hold, or write a snapshot. Every later **Start backup** for that saved item uses that same fixed snapshot; choose and save a newer snapshot as a separate item when its contents should be protected.
2. Choose retention, compression, media spanning, and, optionally, a preferred scratch-media pool, then choose **Continue to tape**. **Automatic** resolves to fast GZIP; explicit balanced GZIP, LZO, and no-compression modes remain recorded with the run. Console backups are deliberately manual: TapeScale does not yet create a fresh snapshot for each run or present this fixed source as rolling protection.
3. TapeScale asks the server for the currently eligible drive/tape pairs before it creates a request. The server checks the saved snapshot binding, exact trusted ready drive, cartridge identity and eligibility, pool, generation compatibility, protection/hold/reservation state, uncertain prior writes, active work, and the current passed setup test for that exact pair. If no pair is ready, use **Prepare loaded tape** in the same flow; after a successful setup test, the pending backup flow reopens.
4. Select the cartridge physically loaded in the chosen standalone drive and continue. TapeScale creates one durable waiting request, rechecks readiness, and carries that pair into **Confirm & start**. Accept the combined loaded-tape, destructive-overwrite, and unencrypted-build statement, then type `OVERWRITE <cartridge label> FOR BACKUP <request id>` exactly. The request remains unwritten until this final authorization succeeds.
5. Follow the dedicated backup page while the worker inventories and hashes the source with a bounded-memory streaming manifest, revalidates every safety gate, and writes through the fenced Bacula adapter. If Bacula reaches end of media, TapeScale releases the drive, displays the next ordered volume request, and waits without consuming the job's progress timeout. Load only the displayed eligible scratch cartridge, verify its printed label, and enter the per-volume confirmation. The selected inventory record is rejected before writing if it is protected, unqualified, reserved, or already used by this archive. A standalone SAS drive cannot machine-read the physical cartridge label, so matching that record to the tape actually loaded remains an explicit operator attestation.
6. After the write, supply the same recorded volumes in the displayed order when TapeScale requests them for Bacula Data verification. This is a physical full readback, including decompression, rather than a source reread. The phase view reports durable stages; a percentage appears only when the backend has a real measured value. Request IDs, ordered volume history, raw engine evidence, and complete history remain available under **Technical details**.
7. On success, choose **Browse backed-up files** for a graphical view of the integrity-checked retained manifest, or **Recovery options** to open the retained recovery point. The browser shows recorded names and metadata without loading the tape and cannot change restore scope. **Verified** means the Full write and complete physical readback of every recorded volume succeeded; it does not claim a restore test.

The readiness screen is server-authored guidance, not write authorization. Its no-cache response is marked `safety_authority: false`; the assignment mutation repeats the gates under database locks and the worker repeats them at dispatch. A pair can therefore disappear or be rejected after the preview if the drive, tape, qualification, reservation, source, or active-work state changes.

Cancellation before assignment is immediate. Cancellation after worker claim is a durable request observed at safe boundaries; it may not stop kernel tape I/O instantly. Partial or uncertain writes remain failed/cancelled evidence and never become a recovery point. Previously valid backups are not changed.

After that exact drive/cartridge combination passes, run the first recoverable-tape exercise:

1. Create or select an immutable TrueNAS snapshot below `/mnt`, for example `pool/dataset/.zfs/snapshot/tapescale-g02`, and keep the same qualified scratch cartridge loaded.
2. Open **Recovery exercise**, create a profile with the snapshot path relative to `/mnt` and an isolated restore subpath below `/mnt/tapescale-restores`.
3. Select the trusted drive and registered cartridge. Confirm scratch use, destructive overwrite, and the explicit G0.2 deviation that encryption is disabled.
4. Type `WRITE <profile name> TO <cartridge label> AND RESTORE VERIFY` exactly and start the exercise.
5. Leave the worker, database, drive, and mounts available. The run is not passed until Bacula completes the full write, a Data verification that rereads tape, a normal restore, and TapeScale's independent manifest comparison.
6. Inspect the phase timeline, engine evidence, source/restored manifests, comparison, and final recovery point. Restored files remain isolated below `/mnt/tapescale-restores/<chosen subpath>/<run id>`; do not publish them as a share.

After the exercise passes, open **Restore → Recovery vault**. Select the retained recovery point to inspect the exact tape and record range, full-readback and restore-test evidence, originating job, artifact digests, and the graphical **Browse files** view derived from the retained source manifest. That browser returns names and recorded metadata only; it does not read tape or expose worker storage paths. It is not yet a selective-restore control: the proven rehearsal still restores and compares the complete captured set. **Download evidence** produces a bounded JSON attachment without exposing the worker's artifact paths. **Plan restore** persists a 30-minute readiness preview and reports every missing prerequisite.

For a ready plan backed by that exact G0.2 full-file point, choose **Run attended recovery rehearsal**, select the qualified drive holding the listed cartridge, and confirm the attended media check. If the plan reports unknown custody, first use **Movements** to record a truthful physical-location confirmation; a typed assertion in the restore dialog does not replace custody evidence. TapeScale revalidates the unexpired plan and preview hash, reserves and fences the exact drive/cartridge, starts a read-only Bacula restore of the retained backup JobId, and writes only below `/restore/<planned destination>/<rehearsal id>`. It then inventories the restored tree and compares it with the retained source manifest. The durable run remains visible through queued, restoring, comparing, and terminal phases; cancellation, failure, artifacts, and a passing `restore_test` record are retained. This execution path is intentionally limited to one unencrypted, single-volume, full-file G0.2 point with its original Bacula catalog. It cannot run native, incremental, multi-volume, encrypted, replace/merge, automatic-publication, or clean-catalog restores.

Use **Movements** to establish the cartridge's first confirmed location and manage later on-site/off-site handling. Create named locations, plan an exact cartridge set, then confirm a direct move or the separate departure and arrival steps. A plan or due date never changes physical custody. Partial confirmations keep the task open, each mutation is idempotent and audited, and overdue work remains overdue until every required cartridge arrives or the task is explicitly cancelled.

Use **Tapes & pools** as the media control center. Its inventory can be searched and filtered by pool, location, identity, health, protection, and attention state. Open a cartridge dossier to review identity, content coordinates, retained recovery points, dependency protection, verifications, holds, custody, reservations, and audit history. Pool assignment and manual, legal, or archive holds use the displayed cartridge version so a stale browser cannot overwrite newer state; safe retries return the original idempotent result. Hold release requires a reason and appends evidence rather than deleting history.

The dossier can evaluate reuse against one selected drive. This is a read-only, fail-closed decision: unknown or conflicting identity, catalog disagreement, retention/dependencies, holds, archives, reservations, pending custody, WORM state, health uncertainty, unknown location, or an incompatible drive keeps the cartridge blocked and lists the affected recovery points and corrective actions. TapeScale does **not** expose a general erase, relabel, recycle, reuse execution, or administrative override endpoint in this release. An `allowed` evaluation is information, not permission to destroy media.

The operational Backup Center and G0.2 exercise share the closed physical adapter: the pinned Bacula Director, Storage Daemon, and File Daemon start only on worker loopback under a fenced run. No operator console or arbitrary engine command is exposed. Operational backup inventory uses a bounded-memory external merge and has no TapeScale total-entry or logical-byte ceiling; the retained-manifest browser remains a bounded catalog view. The operational path supports attended standalone-drive spanning and Bacula compression. The separate G0.2 restore-comparison harness remains deliberately limited to one cartridge, 100,000 manifest entries, and 100 GiB logical data. Incrementals, changers, native ZFS streams, encryption/key recovery, and clean-catalog recovery are rejected rather than simulated.

To update the TrueNAS custom app, open the installed `tapescale` app for editing, replace its YAML with the latest raw [`compose.truenas.yaml`](https://raw.githubusercontent.com/lilrags16/TapeScale/main/compose.truenas.yaml), and save. Each alpha deployment file pins its Git build context and OCI revision to a release tag; `pull_policy: build` rebuilds that exact source instead of silently moving with `main`.

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

The current suite exercises SAS VPD identity/status parsing, allowlisted motion commands, guided tape-preparation prerequisites, server-authored backup-readiness results, bounded source and retained-manifest browsing, durable backup/media requests, scheduler occurrence idempotency, drive-operation and restore-rehearsal leases, device replacement, reset-position recovery, uncertain-motion gates, compatibility discontinuities, overwrite blockers, cartridge-version conflicts, pool/hold idempotency, retention graphs, job transitions, restore prerequisites, path/link containment, RBAC, custody confirmation, engine claim boundaries, cancellation, and safe process execution. Container CI performs single-volume, forced two-volume GZIP6, and LZO file-device roundtrips; each includes Data verification, catalog validation, byte comparison, then a second restore after every Bacula daemon restarts with the original source unavailable. File devices validate software behavior, not physical LTO hardware.

## Runtime boundaries

| Component | Responsibility | Privilege boundary |
|---|---|---|
| `tapescale-api` | Web/API, restore planning, operator workflows | Read-only source/restore views, read-only recovery-manifest artifacts, and `DAC_READ_SEARCH` in the alpha stack; no devices, Docker socket, writable host path, or blanket privilege |
| `tapescale-scheduler` | Timezone-aware Saturday full occurrences, missed-occurrence coalescing, custody reconciliation, and durable waiting-for-media requests | No source, restore, or device access |
| `tapescale-worker` | Durable orchestration, SAS discovery, rewind/unload, `btape`, attended operational full backup/write-readback, the fenced G0.2 exercise, and verified-point recovery rehearsals | Exact tape node, read-only source tree, isolated writable restore root, and narrowly required metadata-preservation capabilities in the alpha stack |
| `tapescale-recovery` | Clean-host bundle inspection and recovery workflow | Runs independently of the original database |
| PostgreSQL 16 | Product state, history, leases, evidence, audit/outbox | Private Compose network and persistent volume |
| Engine adapter | Fixed Bacula 15.0.3 configuration, restricted console commands, compression, ordered volume changes, physical write, Data verify, and restore correlation | Attended operational multi-volume Full-backup plus narrower G0.2 single-volume lab scope; the production engine/format gate remains open |

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

SAS discovery, guided scratch-tape preparation, narrowly allowlisted lab rewind/unload, the Bacula 15.0.3 destructive drive test, graphical source/retained-manifest browsing, bounded-memory operational source inventory, durable attended compressed/uncompressed multi-volume Full backups with ordered physical readback, the narrower G0.2 single-volume restore-comparison exercise, Recovery Vault/evidence export, durable restore-readiness planning, attended verified-point recovery rehearsals, confirmed media-custody operations, and read-only fail-closed media reuse decisions are implemented. An operational backup can create a recovery point only for its exact immutable source, complete ordered physical readback, and fenced hardware tuple; it does not claim a restore test. G0.2 adds an isolated restore and manifest comparison, while a later attended rehearsal can independently reread that narrow kind of retained point. Browsing a retained manifest is not selective restore execution. None is a general restore engine or a production-qualification claim. A safe reuse decision does not erase, relabel, recycle, or overwrite a tape. This release does not physically prove TrueNAS NFSv4 ACL fidelity, multi-volume/EOM behavior, compression restore, wrong-media handling, controlled interruption recovery, encryption, native ZFS, or clean-host catalog reconstruction.

## License

TapeScale is licensed under the [Apache License 2.0](LICENSE).

The runtime test image also distributes unmodified Debian Bacula Community packages under AGPLv3 and Bacula's accompanying additional terms/notices. Their installed copyright and license material remains under `/usr/share/doc/bacula-common/`; TapeScale invokes those programs as separate processes and does not relicense them under Apache-2.0. See the [third-party runtime notices](THIRD_PARTY_NOTICES.md) for the exact packaged versions and corresponding source location.
