# TapeScale Native operations

This guide covers TapeScale `0.2.0-alpha.9`, an attended alpha for one standalone SAS tape drive. New backups use **TapeScale Native v2** with DAR 2.8.6 as a separate archive codec. Native v1 media remains readable. Bacula is not part of the runtime.

Do not make this alpha the only copy of data. Physical qualification is pending.

## Backup flow

1. Select an existing named, read-only ZFS snapshot with the graphical browser.
2. TapeScale inventories and hashes the immutable source and groups hard links.
3. The worker creates and tests independently finalized DAR packs in a bounded spool.
4. Before the first tape motion, TapeScale fills the validated ready queue to high-water minus one maximum in-progress pack, or reaches producer completion for an archive smaller than that threshold.
5. TapeScale writes complete packs sequentially while the producer continues filling the ready window.
6. Once a pack's complete tape files close durably, its disk space may be reclaimed. It remains unverified until the containing volume is rewound and read back completely.
7. At or below low-water, the drive remains parked after a clean tape filemark until the ready queue reaches the resume threshold or the producer finishes.
8. At a volume boundary, TapeScale verifies and unloads the member, then requests the next prepared cartridge.
9. A recovery point is retained only after every volume passes and the final archive-set seal and source stability checks succeed.

Multi-tape archives do not require whole-archive staging. One file, or one hard-link allocation group, must still fit both the configured pack maximum and one cartridge's usable payload. TapeScale fails closed rather than splitting that group.

## Rolling-spool controls

The TrueNAS Compose supplies:

| Environment variable | Release default |
|---|---:|
| `TAPESCALE_NATIVE_PACK_TARGET_BYTES` | `274877906944` (256 GiB) |
| `TAPESCALE_NATIVE_SPOOL_HIGH_WATER_BYTES` | `824633720832` (768 GiB) |
| `TAPESCALE_NATIVE_SPOOL_LOW_WATER_BYTES` | `274877906944` (256 GiB) |
| `TAPESCALE_NATIVE_SPOOL_RESERVE_BYTES` | `8589934592` (8 GiB) |
| `TAPESCALE_NATIVE_SLICE_BYTES` | `68719476736` (64 GiB) |

Use a dedicated, fast, writable POSIX-compatible spool outside the source, restore, and evidence trees. A 1 TiB spool supports the defaults, allowing for filesystem overhead and other use. The high-water value must fit alongside the reserve and leave a ready-pack resume threshold above low-water after reserving one maximum in-progress pack. With the defaults that threshold is 384 GiB. Smaller media generations may impose a lower effective pack maximum.

Only packs whose complete tape files closed durably are eligible for disk reclamation. Reclamation is not verification; no recovery point exists until the whole volume and final archive-set seal pass physical readback. A write failure never silently promotes the last diagnostic state.

## Prepare and identify tape

Preparation is destructive. Use only blank or expendable media. The single **Prepare tape** action writes and reads a deterministic qualification pattern, then writes a durable TapeScale cartridge UUID. At each later destructive boundary the worker reads that UUID from tape and matches it to the reservation; the operator does not repeatedly attest a printed label.

Preparation proves only that exact drive/media pass. It is not general hardware, spanning, restore, or filesystem-fidelity qualification.

## Live job page

The live page reports separate, truthful measurements:

- source inventory entries and logical bytes;
- current source path and active pack;
- ready/spooled/reclaimed bytes and high/low-water thresholds;
- physical tape write and readback rates;
- current volume, pack, safe refill state, and requested continuation tape.

Logical source bytes, compressed pack bytes, and physical device bytes are different bases. A stale or unavailable counter is shown as unavailable, not as an invented zero. Operator waits are distinct from transfer stalls.

## Multi-tape handoff

When the job requests another cartridge, unload the verified member and load the requested prepared tape. TapeScale reads its on-tape UUID before overwrite. A wrong, unreadable, protected, WORM, held, reserved, or already-used cartridge is rejected. Waiting for attended media is durable and does not consume the active-transfer timeout.

## Manage job history

A terminal backup attempt exposes **Delete job** on its detail page. One confirmation removes it from Backup Center, overview counts, and job-action feeds. Deletion is deliberately non-destructive to retained data: tape and volume records, recovery points, manifests, artifacts, transitions, cartridge protection, and audit events remain intact.

Active jobs cannot be deleted. Cancel the job first and wait for the worker to record a terminal state. Failed or interrupted media can still require reconciliation after its job is removed; cartridge safety state remains visible independently in **Tapes**.

## Recovery and upgrade

The web console can browse retained manifests and evidence but does not yet execute a restore. On a clean host, run `tapescale-recovery capture-native-volume` once per member into one capture directory, then `restore-native-archive` into an existing empty destination. The tool auto-detects Native v1 and v2; v2 recovery requires every ordered pack/volume and the final archive-set seal. See [RECOVERY.md](RECOVERY.md).

Upgrading does not rewrite retained media. Untouched queued v1 operational backups upgrade to v2. The separate G0.2 restore-comparison exercise remains v1; running, terminal, or media-touched v1 work also keeps its format and remains readable through the compatibility path.

## Failure and alpha limits

- Before the media boundary, failure leaves tape unchanged.
- After writing starts, a failed or interrupted member is uncertain until reconciled or verified.
- Startup prefill occurs before tape motion; later low-water refills occur only at clean filemarks. Partial packs never become recovery points.
- Unexpected end of media fails closed; normal spanning occurs at planned pack/slice boundaries.
- Worker/container restart does not resume a partially written member in alpha.9; touched media is quarantined and retry creates a new archive set.
- No in-console or selected-file restore, encryption, changer automation, incrementals, automatic snapshots, native ZFS send, or append-after-interruption is available.
- ACLs, xattrs, sparse files, hard links, unusual names, all codecs/generations, spanning, refill, interruption, and clean-host recovery remain physically unqualified.

See [NATIVE_FORMAT.md](NATIVE_FORMAT.md), [SECURITY.md](SECURITY.md), and [QUALIFICATION.md](QUALIFICATION.md).
