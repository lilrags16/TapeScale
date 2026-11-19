# TapeScale Native v1

Status: **alpha; write/readback and independent full-archive recovery implemented, physical qualification pending**

TapeScale Native v1 is the media format selected by TapeScale `0.2.0-alpha.1`. It replaces the failed Bacula-backed experiments for all new backups. It is designed so cartridge and archive identity, volume ordering, payload hashes, and the recovery catalog live on the media rather than existing only in PostgreSQL.

The implementation constants are the normative source during this alpha:

- format name: `tapescale-native-v1`
- metadata version: `1`
- archive representation: `file_dar`
- DAR codec version: `2.8.6`
- maximum physical write record: 256 KiB
- archive volumes and slice ordinals are one-based

## Identity model

Four identifiers have distinct meanings:

| Identity | Scope |
|---|---|
| cartridge UUID | One registered physical cartridge, first written in its preparation header |
| archive UUID | One complete backup across all of its cartridges |
| archive-volume UUID | One physical member of one archive set |
| volume ordinal/count | The member's ordered position in the complete set |

TapeScale does not infer cartridge identity from an operator-entered label at a write boundary. It rewinds the selected device, reads the native preparation record from tape file zero, verifies the record envelope, and requires its cartridge UUID to match the reserved database record. A native archive volume replaces that preparation record with an archive header carrying the same cartridge UUID.

## Metadata envelope

Preparation headers, archive headers, volume indexes, and trailers use a 48-byte binary envelope followed by a UTF-8 JSON payload:

| Offset | Bytes | Value |
|---:|---:|---|
| 0 | 8 | ASCII magic `TSNV1REC` |
| 8 | 2 | big-endian format version (`1`) |
| 10 | 1 | record kind: header `1`, index `2`, trailer `3`, preparation `4` |
| 11 | 1 | reserved flags, currently zero |
| 12 | 4 | big-endian JSON payload length |
| 16 | 32 | SHA-256 of the exact JSON payload bytes |
| 48 | variable | JSON payload |

Metadata payloads are bounded to 16 MiB. Decoders reject unknown JSON fields, an unsupported version, an incorrect kind, trailing bytes, a length mismatch, a digest mismatch, nil UUIDs, invalid ordinals, and non-canonical SHA-256 text.

## Prepared scratch cartridge

Native qualification is destructive. It writes deterministic 256 KiB records across two tape files, rewinds, rereads and hashes both files, and validates the filemark boundary. It then rewinds and replaces that test stream with one preparation metadata record at tape file zero.

The preparation payload contains:

- format version
- cartridge UUID
- preparation timestamp
- optional qualification-evidence digest
- optional drive-identity digest

A preparation record is scratch identity only. It has no archive UUID and cannot be mistaken for a completed archive volume.

## Archive-volume layout

Every native archive cartridge is self-identifying. The physical tape files are:

| Tape file | Contents |
|---:|---|
| 0 | Native volume-header envelope |
| 1 | Complete isolated DAR catalog for the archive |
| 2 | TapeScale source-manifest artifact |
| 3 through `N` | One complete DAR archive slice per tape file |
| `N + 1` | Native volume-index envelope |
| `N + 2` | Native volume-trailer envelope |

Each item is terminated by a tape filemark. No DAR slice is divided between cartridges. The catalog and source manifest are repeated on every archive volume so a found member carries the information needed to identify and inspect its archive set.

The header binds:

- cartridge, archive, and archive-volume UUIDs
- volume ordinal and total volume count
- number of complete DAR slices on this volume
- archive representation and compression codec
- physical record ceiling
- source-manifest and isolated-catalog SHA-256 digests
- creation time

The index maps each local slice to its global slice ordinal, physical tape-file number, contiguous encoded-archive offset, encoded length, logical length, entry count, and SHA-256 digest. The current writer records encoded lengths and hashes authoritatively; logical length and entry allocation remain archive-level facts in the manifest rather than estimates assigned to individual compressed slices.

The trailer repeats the volume bindings and totals, then commits to:

- the SHA-256 of the canonical index payload
- the SHA-256 of the ordered raw slice-digest values
- the completion time

Header, index, and trailer must agree on all volume identities. The index must contain strictly increasing slice and tape-file numbers, contiguous archive offsets, correct totals, and canonical digests. A trailer that does not verify against its index is invalid.

## Staging and spanning

TapeScale does not feed DAR directly into the drive. It creates all DAR slices and the on-the-fly isolated catalog under one run-scoped spool directory, discovers only expected regular files, hashes every output, and runs DAR's archive test over the completed set.

Only then does TapeScale calculate cartridge allocation. It uses the selected media generation's native capacity with a fixed safety reserve and accounts for repeated catalog, manifest, header, index, and trailer overhead. Planning fails if any one complete slice cannot fit on an otherwise empty eligible volume.

This policy intentionally uses more disk and repeats recovery metadata in exchange for three properties:

- directory traversal, small-file metadata work, and compression happen before tape motion;
- the active drive reads large completed files sequentially instead of depending on producer timing;
- a volume boundary never bisects an archive slice.

## Write and physical verification

Before writing a volume, the worker verifies the preparation UUID from tape and rewinds again. It writes each tape file through the exact allowlisted non-rewinding device using bounded 256 KiB records. TapeScale, not DAR, owns tape motion, file boundaries, cartridge selection, and volume handoff.

After the trailer is committed, the worker rewinds the cartridge and reads every tape file in order. It verifies:

- header envelope, digest, type, and complete identity binding
- isolated catalog length and SHA-256
- source-manifest length and SHA-256
- every slice length and SHA-256
- index envelope and exact expected value
- trailer envelope, exact expected value, and its index commitment

A volume is marked verified only after this complete physical pass. An archive recovery point is retained only after every planned volume is verified and the immutable source is rechecked for stability.

Physical readback establishes that the encoded archive bytes and recovery metadata can be read from the just-written cartridges. It does **not** establish filesystem-level restore fidelity.

## Database independence and recovery plan

The on-tape fields are sufficient in principle for a replacement-host reader to:

1. scan tape file zero and group cartridges by archive UUID;
2. reject duplicate, missing, foreign, or inconsistent volume members;
3. verify every member's catalog, manifest, slices, index, and trailer;
4. order the complete slice set by global ordinal and reconstruct DAR's expected slice names;
5. use the repeated isolated catalog for listing and selection;
6. extract through DAR into an isolated destination;
7. compare the restored tree with the authenticated source manifest.

`tapescale-recovery` implements that database-independent full-archive path in `0.2.0-alpha.1`:

1. `capture-native-volume --device /dev/nstN --archive-dir /capture` rewinds one cartridge, reads the authenticated header, and uses its `slice_count` to stream the catalog, manifest, slices, index, and trailer without guessing at end of data.
2. The capture rejects mismatched identities, ordinals, tape-file numbers, offsets, lengths, hashes, index commitments, duplicate media, or artifacts from another archive. It writes canonical DAR slice names into a private capture directory.
3. Repeat capture for every declared volume. The volumes may be presented in any order; restore requires the complete contiguous volume and slice set.
4. `restore-native-archive --archive-dir /capture --destination /empty` re-verifies every captured byte, requires pinned DAR 2.8.6, tests the reconstructed archive, extracts only into an existing empty destination, and independently compares the restored tree with the authenticated source manifest.

This is an attended clean-host CLI, not yet an in-console restore worker or selected-file browser. Its tape behavior, filesystem metadata fidelity, multi-volume handoffs, and failure cases still require qualification on real supported hardware.

## Failure behavior

- Failure before the media-write boundary deletes the run-scoped staging area and does not touch tape.
- Failure after tape writing begins retains the staged archive for diagnosis/retry and marks the touched cartridge uncertain.
- A wrong or unreadable preparation UUID blocks before destructive I/O.
- A missing continuation waits durably for the exact reserved ordinal without consuming an active-transfer timeout.
- Partial, cancelled, mismatched, or unreadable volumes never create a recovery point.

The first native release deliberately does not attempt append-after-interruption or physical end-of-media recovery. It plans within conservative capacity and treats an unexpected write boundary as a failed, uncertain volume.

## Compatibility

TapeScale Native v1 cannot read Bacula volumes, and Bacula cannot read TapeScale Native v1 volumes. The `0.2.0-alpha.1` transition assumes that the deployment has zero valid Bacula tapes: failed and touched Bacula runs remain audit history, not recovery points. The runtime does not include a legacy Bacula recovery engine.

Do not destructively prepare older media merely to make it appear native. If an older cartridge might contain data worth recovering, remove it from this alpha workflow and preserve it with the software needed for its original format.
