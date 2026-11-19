# Architecture

TapeScale is a control plane for durable LTO protection on TrueNAS. The `0.2.0` alpha line introduces **TapeScale Native v1**, an incompatible on-tape format owned by TapeScale. DAR 2.8.6 is a separately executed archive codec; it does not own cartridge identity, tape motion, media handoff, verification evidence, retention, or recovery policy.

The active runtime has no Bacula package or daemon dependency. Bacula-specific source modules and historical database values remain only so failed earlier-alpha rows can be read as audit history; new profiles, preparation runs, backups, and recovery media use the native contract.

## Service topology

```text
browser
   |
   v
tapescale-api  ------> PostgreSQL <------ tapescale-scheduler
   |                       ^
   | read-only views       |
   v                       |
/source, /restore          |
                           |
                    tapescale-worker
                      |    |    |
                      |    |    +--> /var/lib/tapescale/recovery
                      |    +-------> native spool (read/write)
                      +------------> exact /dev/nstN + Linux SCSI sysfs
```

- **API**: JSON API, web console, bounded source/manifest browsing, planning, operator intent, and evidence projection. It has no tape device.
- **Scheduler**: durable fixed-source schedule evaluation and catch-up requests. It has no source, restore, spool, or device mount.
- **Worker**: the only process allowed to inventory source content, execute DAR, move tape, write/read native records, and retain artifacts.
- **PostgreSQL**: source of truth for inventory, leases, requests, archives, ordered volumes, segments, events, holds, custody, and recovery points.
- **`tapescale-recovery`**: independent clean-host volume capture and DAR restore utility. It requires neither the API nor PostgreSQL.

## Authority and fencing

An API response never grants continuing physical authority. Destructive work uses durable intent plus database leases and monotonically increasing fencing tokens for the exact drive and cartridge. The worker claims a request, reloads it under lock, re-observes the SAS device, verifies its stable identity and current state, and rechecks media eligibility immediately before the physical boundary.

The worker accepts only configured non-rewinding device paths matching `/dev/nstN`. Motion and block-mode changes use closed `mt-st` argument vectors. Arbitrary tape commands, shell fragments, engine consoles, and user-provided executable arguments are absent from the API.

## Native preparation

Preparation is both a destructive drive/media exercise and the bootstrap identity step:

1. enable variable-block I/O and rewind;
2. write two tape files containing deterministic 256 KiB records;
3. rewind and hash every record, including the filemark transition;
4. rewind and replace the test stream with a versioned preparation record;
5. reread the preparation record and require its cartridge UUID;
6. rewind and require a known ready-at-BOT postcondition.

The archive writer later replaces that preparation record with an archive-volume header carrying the same cartridge UUID. Thus a scratch cartridge identifies itself before overwrite and a completed archive identifies its archive set afterward.

## Native backup pipeline

### 1. Immutable source inventory

The selected source must resolve beneath `/source` to a named `.zfs/snapshot` path and the kernel must report its open mount read-only. Inventory uses bounded-memory external sorting and disk-backed hard-link grouping, producing a content-addressed manifest with per-file SHA-256 and declared metadata coverage. TapeScale imposes no total file-count or logical-byte policy ceiling; filesystem, artifact, storage, and time limits remain real.

### 2. Complete archive staging

Each run receives an empty private directory beneath `TAPESCALE_NATIVE_SPOOL_DIR`. The worker invokes the absolute DAR executable with a closed noninteractive argument vector:

- Full archive rooted at the selected snapshot;
- fixed-size large slices (64 GiB by default);
- on-the-fly isolated recovery catalog;
- `auto` resolved to fast Zstandard, or explicit Zstd/LZ4/GZIP/none;
- bounded compression workers;
- no user or system DAR configuration.

Only expected regular `archive.<ordinal>.dar` and catalog files are accepted. TapeScale hashes every slice, verifies contiguous ordinals and checked byte totals, then asks DAR to test the complete staged archive. A segment ledger is committed before tape is touched.

The stage-first contract is the anti-shoe-shine mechanism. Source traversal, tiny-file packaging, metadata work, compression, and archive testing finish before an active tape data pass. A small RAM ring cannot compensate for a source whose long-term rate is below an LTO drive's minimum; complete staging can.

### 3. Capacity and volume planning

TapeScale maps recorded LTO generation/format to native decimal capacity, subtracts a five-percent reserve, repeated catalog/manifest/native metadata, and additional framing allowance. Every DAR slice remains whole. Planning fails before write if a slice cannot fit an empty volume or if multiple volumes are required while spanning is disabled.

Unexpected physical EOM is not used as normal flow in Native v1. It fails and quarantines the volume instead of guessing how much of a slice reached media.

### 4. Physical write

For each planned volume, the worker:

1. reads and verifies the expected preparation UUID;
2. commits the irreversible media-write boundary;
3. enables variable-block mode and rewinds;
4. writes the native header, catalog, source manifest, complete DAR slices, index, and trailer as distinct tape files;
5. uses bounded 256 KiB physical records;
6. closes each non-rewinding descriptor to commit the file boundary.

The exact on-tape layout and binary envelope are documented in [NATIVE_FORMAT.md](NATIVE_FORMAT.md).

### 5. Immediate physical readback

The same volume is rewound immediately. The worker reads every tape file in order and verifies:

- decoded header and complete identity binding;
- catalog and source-manifest length/SHA-256;
- every archive slice length/SHA-256;
- decoded index equals the planned slice map;
- decoded trailer equals the expected value and commits to the index.

Only then is the volume ledger marked verified. Multi-volume operation unloads that completed member and creates a durable request for the next ordinal. The next preparation UUID is checked before its media-loaded boundary.

After all volumes pass, TapeScale rescans the source for stability and atomically creates the recovery point, ordered volume records, write-complete verification, full-readback verification, and terminal result. No native backup path claims a restore test.

## On-tape identity and reconstruction

The native format distinguishes:

- **cartridge UUID**: stable physical inventory identity;
- **archive UUID**: one complete backup;
- **archive-volume UUID**: one physical member of that archive;
- **volume ordinal/count**: required member ordering;
- **slice ordinal and encoded offset**: global archive reconstruction ordering.

Every member repeats the isolated DAR catalog and authenticated source manifest. The header declares how many local slices follow, so a database-independent reader can locate the index and trailer without an ambiguous EOD scan. Index and trailer fields make duplicate, missing, swapped, foreign, and reordered members detectable.

## Independent recovery

`tapescale-recovery capture-native-volume` reads one cartridge into an existing capture directory, verifies the complete volume contract, writes canonical DAR slice names, and commits a receipt only after a final rewind. Existing captures must agree on archive UUID, volume count, representation, compression, catalog/manifest digests, unique cartridge/archive-volume UUIDs, and volume ordering.

`tapescale-recovery restore-native-archive` requires every declared volume and a contiguous slice/offset set. It rehashes all capture files, decodes native metadata again, tests the reconstructed DAR archive, extracts to an existing empty real directory, inventories the restored tree, and compares it to the captured source manifest.

This is an implemented database-independent path, but it is not yet exposed as a web Restore run and does not constitute physical qualification. Capture storage contains plaintext archive data and must be protected.

## Persistence model

PostgreSQL stores engine-neutral archive identities in addition to historical fields:

- profile and run archive codec;
- archive UUID per run/recovery point;
- archive-volume UUID, header/index digests, and tape-file count;
- one row per staged DAR slice with hash, size, state, volume ordinal, and tape-file number;
- append-only phase/media events and content-addressed artifacts.

Saved profiles migrate to TapeScale Native/DAR. Untouched queued requests can migrate because no media format has been committed. Failed, interrupted, or media-touched Bacula-era runs preserve their original engine/format as audit history and are not promoted as native recovery points.

## Progress semantics

Live telemetry has explicitly different bases:

- source inventory entries/logical bytes;
- staged native archive bytes and files;
- most recently reported source-relative DAR path;
- Linux SCSI tape read/write byte-counter deltas;
- current native stage and attended volume request.

The system does not divide compressed stream bytes by logical source bytes or label a tape counter as ZFS throughput. A missing/stale counter remains unavailable, not zero. Waiting for an operator is distinct from a transfer stall.

## Media policy

Inventory labels and barcodes are human/custody metadata; the native UUID read from tape is the destructive-boundary identity. Pools, holds, retention, custody, reservations, and overwrite analysis remain product-owned and independent of DAR. An `allowed` reuse evaluation does not dispatch an erase or preparation operation.

## Failure model

- pre-write failures delete the unused run stage and leave tape untouched;
- post-boundary failures retain the stage and mark touched media uncertain;
- cancellation is cooperative at safe boundaries because blocking kernel tape I/O cannot be forcibly made atomic;
- wrong tape, identity conflict, lease loss, device replacement, reset position, malformed native metadata, length/hash mismatch, source mutation, or failed cleanup prevents recovery-point creation;
- terminal evidence never upgrades a broader hardware/format qualification state.

## Current scope

Implemented: native preparation, graphical snapshot selection, bounded inventory, DAR packing/compression, complete disk staging/testing, manual multi-tape write, immediate physical readback, recovery point/evidence retention, manifest browsing, and independent offline full-archive capture/restore.

Open: in-console native restore, selected-file native extraction, incrementals, encryption, changer automation, automatic snapshots, native ZFS send, append/retry after interrupted media, metadata signing, authentication/RBAC, and the physical qualification matrix.
