# Security model

TapeScale `0.2.0-alpha.11` is an attended, non-production release. The alpha web application has no accounts, authorization, or TLS termination. Anyone who can reach its HTTP port can inspect filenames and attempt to initiate the exposed workflows. Deploy it only on an isolated trusted network behind host-level access controls, and never load retained or unknown media while destructive features are enabled.

## Authority boundaries

The deployment separates four services:

- the API serves the UI, performs bounded read-only browsing, and persists intent;
- the scheduler creates durable work but has no source, restore, or tape access;
- the worker alone receives the exact tape device, read-only source mount, and writable spool/restore roots;
- PostgreSQL is reachable only on the private application network.

Neither the API nor scheduler receives `/dev/nstN`. No service receives the Docker socket or a writable host-root mount. The TrueNAS alpha worker runs as container root with a narrow capability set because it must traverse protected source metadata and preserve filesystem ownership; it is not privileged.

## Destructive tape operations

Public tape motion is a closed set: rewind and rewind-and-unload. Preparation is a separate destructive workflow. The API does not expose seek, spacing, arbitrary filemarks, erase, format, raw read/write, shell, or free-form `mt-st` arguments.

Preparation requires:

- `TAPESCALE_TAPE_PREPARATION_ENABLED=true`;
- an exact reconciled allowlisted `/dev/nstN` drive;
- a ready loaded medium;
- a registered, unprotected, non-WORM scratch cartridge;
- no conflicting hold, retained content, reservation, identity mismatch, or active operation;
- one server-bound destructive action;
- fenced drive and media leases revalidated by the worker.

The worker dispatches fixed `mt-st` argument vectors and native record I/O only to the configured non-rewinding device. It writes deterministic qualification data, rereads and hashes it, then writes and rereads a preparation header containing the cartridge UUID. A process timeout cannot force a kernel-blocked tape operation to return; timeout, cancellation, lease loss, identity change, or an uncertain postcondition revokes trust and blocks later destructive work.

Linux may commit a pending filemark when a tape descriptor closes or before motion. Rewind and unload therefore remain physical operations and should be used only with media whose current state is understood.

## Native backup safety gates

New backups use TapeScale Native v2; Native v1 remains read-compatible and Bacula is not installed in the runtime. Before tape motion, the worker:

1. reopens and verifies the selected source is a contained named snapshot on a read-only mount;
2. inventories and hashes the source with bounded-memory external sorting;
3. creates a private run-scoped bounded rolling spool on the dedicated filesystem;
4. completes, tests, hashes, and durably records each independent DAR pack before that pack is consumed;
5. verifies pack and cartridge allocation boundaries against conservative native capacity;
6. rechecks leases, drive identity, cartridge eligibility, and the preparation UUID read from tape.

The permanent media-write boundary is committed before overwriting the preparation record. After a complete pack's tape files close durably, its disk files may be reclaimed so the spool remains bounded; that does not verify the pack or create a recovery point. The containing volume must still be rewound and read back completely. Any failure or ambiguity quarantines touched media and prevents creation of a recovery point.

An operator preview is advisory. Assignment and worker execution repeat eligibility checks under database locks and fencing tokens. Printed labels, UI selection, and typed text never substitute for the native UUID read from tape.

## DAR process boundary

DAR 2.8.6 and libthreadar are distributed under their own licenses as separate executables. TapeScale does not link their code into its Rust binaries.

The worker invokes an absolute configured executable with a closed argument list and no shell. Source and stage paths are server-resolved absolute paths. Configuration files are disabled with `-N`, interactive prompts are disabled with `-Q`, compression and thread counts come from enumerated/bounded values, and stdout/stderr evidence is bounded. A run-scoped directory accepts only expected regular files and rejects symlinks, special objects, unexpected names, missing slices, non-contiguous ordinals, and overflowed byte totals.

DAR input is still the contents and metadata of the selected filesystem tree. Unusual paths and metadata require physical qualification, but filenames are never interpolated into a shell command.

## On-tape authenticity and integrity

Native metadata records use a versioned binary envelope with record kind, payload length, and SHA-256. Headers, pack descriptors, volume indexes, trailers, and the final archive-set seal bind cartridge, archive, archive-volume, pack, and ordinal identities. Every DAR catalog/slice has an expected commitment. The final seal commits to the complete ordered pack and volume roots.

SHA-256 detects accidental corruption and inconsistent media; it is not a signature against a malicious party who can rewrite both data and metadata. The current unencrypted alpha has no key-backed media authentication or confidentiality. Treat physical custody and network isolation as part of the security boundary.

## Source and manifest browsing

The source browser is read-only, one directory level at a time, bounded, non-cacheable, descriptor-relative, and does not follow symlinks. Special objects are visible but not navigable or selectable. It never makes `/source` itself a backup root.

The retained browser verifies the manifest artifact before projecting recorded names, types, sizes, and timestamps. It does not read source or tape content and does not return internal storage paths. Because the alpha is unauthenticated, both browsers disclose filenames to any network client that can reach the API.

## Spool and evidence isolation

The native spool must be a real directory isolated from source, restore, and evidence trees. It should be a dedicated dataset that is writable only by the worker. The API must never mount it. Run directories are created without following symlinks; unexpected non-regular files cause failure.

Recovery evidence is writable by the worker and read-only to the API. Public evidence responses are bounded and omit internal artifact paths. PostgreSQL append-only events retain intent, physical boundaries, volume identities, and results.

## Offline recovery utility

`tapescale-recovery` runs without the original database. Its native commands:

- accept only exact `/dev/nstN` device paths;
- use `mt-st` only for variable-block mode and rewind with fixed arguments;
- stream tape files into a real capture directory;
- validate all volume and slice bindings before committing a receipt;
- refuse duplicate/inconsistent archive members;
- require a complete contiguous volume/slice set before extraction;
- require an existing empty real destination;
- test the reconstructed DAR archive before restore;
- compare the restored tree with the authenticated source manifest.

Captured archive directories contain plaintext source data and metadata. Protect them as strongly as the original source, restrict permissions, and remove them through an approved lifecycle after recovery evidence is retained.

## Database and secrets

The pasteable TrueNAS alpha intentionally contains a disposable inline PostgreSQL password because it targets an isolated test environment. It is not a production credential design. A qualified deployment must use secret files or an external secret facility, TLS for the web endpoint, authentication, RBAC, audited reauthentication for destructive actions, and independently stored recovery material.

Do not log keys, passwords, usable recovery secrets, or raw configuration exports. Encryption and key recovery are not implemented by the native alpha.

## Denial of service and resource limits

The API bounds request bodies, pages, evidence values, manifest browsing, and concurrency. The worker bounds command output, metadata records, thread counts, slice sizes, timeouts, retained JSON, and rolling-spool watermarks. Source inventory has no artificial total entry or logical-byte ceiling, so real storage, inode, CPU, spool, and time exhaustion remain possible. Use a dedicated dataset, quotas, container memory/CPU/PID limits, and network isolation.

## Known gaps

- no application authentication, authorization, MFA, or native TLS;
- no encrypted archive or authenticated metadata signature;
- no production-qualified drive/media/metadata matrix;
- no changer support or unattended media selection;
- in-console native restore is pending;
- no automatic secure deletion of captured recovery plaintext;
- no mid-volume writer restart/resume; an interrupted partial member is quarantined and a retry creates a new archive set;
- legacy Bacula source modules remain for historical audit compatibility, but the 0.2 runtime neither packages nor uses Bacula for native backup or recovery.

Report vulnerabilities through the private process described in the repository `SECURITY.md`, not a public issue.
