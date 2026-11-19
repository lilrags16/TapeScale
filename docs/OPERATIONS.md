# TapeScale Native operations

This guide describes TapeScale `0.2.0-alpha.2`. It is an attended alpha for one standalone SAS tape drive, not a production-qualification claim. The active writer is **TapeScale Native v1** and DAR 2.8.6 is used only as the file-archive codec. Bacula is not installed or used by the current runtime.

Do not make this alpha the only copy of data. A result is evidence for the exact source snapshot, drive, firmware, cartridge set, format, and software revision recorded by the run.

## Operating model

A native Full backup follows one closed sequence:

1. Select one existing named ZFS snapshot with the graphical browser.
2. Inventory and SHA-256 hash the immutable source.
3. Build large DAR slices and an isolated catalog on the dedicated spool dataset.
4. Test the complete staged archive while tape remains idle.
5. Plan whole slices across the required prepared cartridges.
6. Read the loaded cartridge UUID from tape file zero and reject a mismatch.
7. Stream completed large files sequentially to tape.
8. Rewind and immediately read every tape file back, checking identities, lengths, and SHA-256 digests.
9. Retain a recovery point only after every volume passes and the source snapshot is still unchanged.

This design deliberately spends disk capacity to separate small-file traversal and compression from tape motion. It does not use a partial fill/drain spool or a small RAM buffer as a substitute for sustained throughput.

## Required deployment flags

The TrueNAS alpha Compose enables these canonical controls:

- `TAPESCALE_TAPE_MOTION_ENABLED=true` for guarded rewind and unload;
- `TAPESCALE_TAPE_PREPARATION_ENABLED=true` for destructive native preparation;
- `TAPESCALE_NATIVE_BACKUP_ENABLED=true` for native Full backups;
- `TAPESCALE_DAR_PATH=/usr/local/bin/dar`;
- `TAPESCALE_NATIVE_SPOOL_DIR=/var/lib/tapescale/spool`;
- `TAPESCALE_NATIVE_SPOOL_RESERVE_BYTES=8589934592`;
- `TAPESCALE_NATIVE_SLICE_BYTES=68719476736`;
- `TAPESCALE_NATIVE_COMPRESSION_THREADS=4`.

Older `TAPESCALE_LAB_*` flag names remain compatibility aliases only. New deployments should use the canonical names above.

## Prepare a cartridge

> **Destructive operation:** preparation overwrites the loaded cartridge. Use only blank or expendable scratch media. Never prepare a retained, foreign, WORM, or irreplaceable tape.

1. Load one scratch cartridge and wait for the drive to report ready.
2. Open **Prepare tape** from the drive or tape view.
3. Register or select the inventory record and LTO generation.
4. Review the exact detected drive and cartridge, then invoke the single destructive action.
5. Leave the worker and drive available until the result is terminal.

The worker uses the exact configured `/dev/nstN` path, enables variable-block I/O, writes deterministic 256 KiB records across two tape files, rewinds, and rereads every record and file boundary. A pass then replaces the test data with a TapeScale preparation record containing the cartridge UUID and reads it back. That UUID, not a repeated typed assertion, identifies the tape at later destructive boundaries.

A pass is narrow evidence for that drive/cartridge tuple. It is not backup, spanning, restore, ACL, or general hardware qualification.

## Create and run a backup

1. In **Backups**, browse beneath `/source` to a path such as `pool/dataset/.zfs/snapshot/snapshot-name`.
2. Save the selection. TapeScale resolves it beneath the held source root and verifies kernel read-only mount flags.
3. Choose retention, optional scratch pool, media spanning, and compression:
   - **Automatic** → fast Zstandard;
   - **Zstandard** → fast Zstd;
   - **LZ4** → fast LZ4;
   - **GZIP** → balanced GZIP;
   - **None** → no archive compression.
4. Create the request and review the server-calculated eligible drive/cartridge pair.
5. Start the destructive run. The API and worker repeat the source, lease, reservation, media, hold, WORM, qualification, and on-tape identity checks under lock.

Only complete disk staging is supported by the native writer. Direct source-to-tape delivery is a legacy policy value and is rejected for new native runs.

### Spool sizing

The spool must fit the complete encoded archive plus the configured reserve. Preflight conservatively budgets source logical bytes, compression headroom, entry overhead, the retained manifest, and native metadata. Actual compression may use less space, but TapeScale will not begin tape motion based on an incomplete archive.

Use a dedicated fast SSD/NVMe dataset when possible. The stage is deleted after successful finalization. It is retained for diagnosis after a media-write failure; a pre-write failure removes the unused stage.

### Multi-tape handoff

DAR slices are assigned whole to volumes. When another cartridge is required:

1. the completed volume has already been rewound and fully verified;
2. unload it and load the exact requested prepared cartridge;
3. authorize the displayed continuation;
4. TapeScale reads the new preparation UUID before accepting or overwriting it.

The operator wait is durable and is not counted as an active-transfer stall. A wrong, missing, foreign, protected, held, unqualified, reserved, or already-used cartridge is rejected before data I/O.

## Understand the live page

The UI keeps incompatible counters separate:

- **Inventory**: source entries and logical bytes discovered;
- **Packing**: regular files and bytes created in the run-scoped disk stage;
- **Current file**: the bounded source-relative path most recently reported by DAR;
- **Tape write**: physical write rate for the exact SCSI tape device;
- **Tape read**: physical rate during immediate volume readback;
- **Waiting for tape**: the required next volume, with no invented transfer rate.

Logical source bytes, compressed DAR bytes, and physical device bytes are not interchangeable. Compression, sparse files, hard links, metadata, record framing, and repeated per-volume metadata prevent a truthful single synthetic byte counter.

If packing is slow, inspect source metadata latency, CPU/compression, and spool I/O. Tape should be idle during this phase. If the drive shoe-shines during the native tape pass, the completed spool filesystem, host contention, HBA/device path, media condition, or physical block behavior is the likely bottleneck; the source filesystem is no longer in the active data path.

## Cancellation and failure

- Before the media-write boundary, cancellation leaves tape unchanged.
- Physical record I/O cannot always be interrupted safely. After cancellation, the worker finishes the current tape file before stopping when possible.
- A failed or interrupted write creates no recovery point and leaves touched media uncertain/write-unverified.
- A device reset, disconnect, lease loss, identity change, or unproven cleanup position requires reconciliation before further destructive work.
- An unexpected end-of-media is a failed volume in this first native release; capacity planning intentionally leaves a reserve rather than relying on EOM continuation.

## Recovery

The web Recovery Vault can browse retained manifests and evidence. In-console restore dispatch remains pending for native points. The independent `tapescale-recovery` executable can operate without PostgreSQL:

```sh
tapescale-recovery capture-native-volume \
  --device /dev/nst0 \
  --archive-dir /recovery/archive

tapescale-recovery restore-native-archive \
  --archive-dir /recovery/archive \
  --destination /recovery/restored
```

Run the capture command once for every cartridge in the archive. It reads and verifies the header, repeated catalog and source manifest, slices, volume index, and trailer. Restore is accepted only when every declared volume and every contiguous slice is present and all captured files still match their digests. The destination must be an existing empty real directory. DAR tests the reconstructed archive before extraction, then TapeScale inventories the result and compares it with the retained source manifest.

This implemented utility still requires physical qualification across drive generations, failure modes, and metadata combinations. Follow [RECOVERY.md](RECOVERY.md) and do not publish restored data without review.

## Media management

Retention, holds, custody, pools, and overwrite evaluation are separate from physical tape identity. A server result that media appears reusable does not itself erase or prepare it. Archive and legal holds never silently expire. Unknown identity, incomplete dependency evidence, unverified writes, WORM state, active reservations, and retained recovery points block destructive reuse.

## Current limits

- attended manually started Full file archives only;
- one standalone drive, with manual cartridge changes;
- existing named snapshots only; no automatic snapshot creation or rolling selection;
- no incrementals, encryption, tape library/changer automation, native ZFS send, or append-after-interruption;
- in-console native restore and selected-file extraction are pending;
- no production claim for ACL/xattr fidelity, hard links, sparse files, unusual names, compression, spanning, interruption, or clean-host recovery until the qualification ledger records physical evidence.

See [NATIVE_FORMAT.md](NATIVE_FORMAT.md), [SECURITY.md](SECURITY.md), and [QUALIFICATION.md](QUALIFICATION.md).
