# Qualification and acceptance ledger

This ledger separates implemented behavior from evidence collected on physical tape hardware. It applies to TapeScale `0.2.0-alpha.11` and TapeScale Native v2; Native v1 is recovery-compatible history.

## Status vocabulary

- **Implemented**: the code path exists and has automated repository evidence. It does not imply physical qualification.
- **Simulated**: deterministic control-plane or file-backed evidence exists.
- **Not run**: required evidence has not been collected.
- **Blocked**: a named prerequisite prevents the test.
- **Qualified**: reviewed evidence exists for the exact named TrueNAS host, TapeScale build, drive, HBA, firmware, media, filesystem, and recovery environment.

Nothing in this alpha is physically **Qualified**. Do not use it as the only copy of data.

## Active implementation

New backups use TapeScale Native v2. DAR 2.8.6 packages ordered, hard-link-closed source ranges into independently tested batches in a bounded rolling spool. TapeScale alternates pack production and tape drain at complete filemarks on a rotational spool. Complete pack tape files, chained volume indexes, and the final archive-set seal are written directly to the non-rewinding tape device. PostgreSQL records operations but is not required by the clean-host recovery utility.

Bacula is retired from the runtime. Earlier failed Bacula rows remain audit history only and are not valid recovery points. Native v1/v2 are intentionally incompatible with earlier Bacula media.

## Current ledger

| ID | Acceptance case | Software status | Physical evidence | Remaining work |
|---|---|---|---|---|
| AT-01 | Clean installation and preflight | Implemented/Simulated | Not run | Execute on the target TrueNAS release and retain container, device, mount, and dependency evidence. |
| AT-02 | LTO generation rules | Simulated | Not run | Exercise every claimed generation and exact drive/media combination. No LTO generation is yet qualified. |
| AT-03 | Attended multi-volume archive | Implemented/Simulated | Not run | Force real volume handoffs and validate ordering, identity, EOM behavior, readback, and clean-host restore. |
| AT-04 | Automated changer inventory/motion | Not implemented | Not run | Native v2 alpha targets one attended standalone SAS drive. |
| AT-05 | Concurrent drives | Control-plane only | Not run | Prove leases, fencing, routing, and isolation with two physical drives. |
| AT-06 | Snapshot protection and cleanup | Partial | Not run | The source must already be an immutable read-only snapshot; automatic snapshot lifecycle remains open. |
| AT-07 | File content and metadata round trip | Implemented/Simulated | Not run | Run representative POSIX/NFSv4 ACL, xattr, sparse, link, unusual-name, unreadable, and metadata-only fixtures on TrueNAS. |
| AT-08 | Native dataset reconstruction | Not implemented | Not run | Native ZFS send/receive is outside this alpha. |
| AT-09 | Zvol coverage | Not implemented | Not run | Add and qualify an explicit native-stream representation. |
| AT-10 | Exact recovery point | Implemented for full archive | Not run | Web-operated and selected-path restore remain open; the independent CLI restores a complete archive. |
| AT-11 | Object/archive larger than one cartridge | Implemented/Simulated | Not run | Prove physical spanning. Native v2 rolls independent packs through bounded disk and keeps each pack/slice allocation boundary whole. |
| AT-12 | Cancellation and process/host failure | Partial/Simulated | Not run | Exercise every boundary before and after media write, including lease loss, reset, restart, and power failure. |
| AT-13 | Wrong/unknown media and device loss | Implemented/Simulated | Not run | Prove native preparation UUID and archive header rejection on real wrong, missing, reset, foreign, protected, and unreadable media. |
| AT-14 | Compression modes | Implemented/Simulated | Not run | Qualify Zstandard, LZ4, GZIP, none, and automatic selection through write, readback, and restore. |
| AT-15 | Corrupt/reordered/truncated content | Implemented/Simulated | Not run | Expand fault injection and prove every envelope, pack, volume-chain, final-seal, and trailer failure is rejected. |
| AT-16 | Immediate physical readback | Implemented | Not run | Run on named drives/media and retain per-tape hash and identity evidence. Readback is not a filesystem restore test. |
| AT-17 | Loss of original host and database | Implemented/Simulated | Not run | On a clean host, capture every tape with `tapescale-recovery`, reconstruct the DAR set, restore, and compare. |
| AT-18 | Encryption and key rotation | Not implemented | Not run | Native v2 alpha is unencrypted. Protect tapes and capture storage accordingly. |
| AT-19 | Retention, holds, custody, and dependencies | Implemented/Simulated | Not run | Exercise the complete operational lifecycle, including protected media and expired archives. |
| AT-20 | Scheduling, overlap, local time, and downtime | Partial/Simulated | Not run | Weekly full scheduling exists; broader policies and named-host DST/restart evidence remain. |
| AT-21 | Confirmed and missed exchanges | Implemented/Simulated | Not run | Run attended continuation and readback exchanges with delayed, missed, and wrong cartridges. |
| AT-22 | Minimum recovery coverage | Simulated | Not run | Exercise emergency recall and replacement-host delivery procedures. |
| AT-23 | Roles and safe destinations | Partial | Not run | Add production authentication/RBAC and adversarial destination tests. |
| AT-24 | Container replacement and migration | Partial | Not run | CI upgrades the prior schema; prove v1 read compatibility and the documented no-mid-volume-resume quarantine behavior. |
| AT-25 | Rolling-spool capacity and backpressure | Implemented/Simulated | Not run | Measure 1 TiB ready/low/high-water operation, metadata-heavy input, near-full capacity, exclusive production/drain batches, and stalled-device cases. |
| AT-26 | Representative workloads and sustained speed | Implemented/Simulated | Not run | Measure small-file pack production, compression, rolling-spool rates, tape streaming, and backhitch behavior. |
| AT-27 | Source errors and destination safety | Implemented/Simulated | Not run | Broaden physical error fixtures; restore remains limited to a caller-provided existing empty directory. |
| AT-28 | Durable events and actionable status | Implemented/Simulated | Not run | Exercise restarts and long attended waits on the deployment host. |
| AT-29 | Runtime dependency and license audit | Implemented | Not run | The image pins DAR/libdar/libthreadar and excludes Bacula; retain final image digest, SBOM, and license review. |
| AT-30 | Safe external-tool execution | Implemented/Simulated | Not run | DAR and `mt-st` use closed argument vectors; exercise hangs, signals, cancellation, and kernel I/O that outlives userspace. |
| AT-31 | Product-specific workflow review | Implemented | Not run | Record product-owner desktop/phone review of preparation, backups, handoffs, failure recovery, and clean-host restore. |

## Engineering gates

### G0 — Native architecture

**Implemented; physical acceptance not passed.**

The selected alpha architecture is TapeScale Native v2 with DAR 2.8.6 as a separately executed archive codec. Bounded batched pack production, the on-tape envelope, cartridge/archive identity, pack-boundary spanning, direct tape I/O, immediate whole-volume readback, final set sealing, and independent recovery CLI are implemented and documented in [NATIVE_FORMAT.md](NATIVE_FORMAT.md). Real-hardware throughput, filesystem fidelity, EOM, wrong-media, interruption, and clean-host evidence remain required.

### G1 — Recovery safety

**Implemented in software; physical matrix not run.**

Native metadata and every archive pack are committed and every completed volume is physically reread before a recovery point is retained. A pack's disk copy may be reclaimed after its complete tape files close, but it remains unverified until that readback passes. The source is checked again after readback. This proves neither a filesystem restore nor every failure boundary.

### G2 — Independent recovery

**CLI implemented; clean-host physical exercise not run.**

`tapescale-recovery capture-native-volume` authenticates and captures each volume without PostgreSQL. `restore-native-archive` requires the complete ordered set, tests and extracts it with pinned DAR, then independently compares the restored tree with the on-tape manifest.

### G3 — Operations

**Alpha implementation; not passed.**

One attended standalone SAS drive, manual Full backups, bounded rolling spool, ordered continuation, immediate readback, final archive-set sealing, retention evidence, and weekly Full scheduling exist. Mid-volume restart/resume is not implemented: partial media is quarantined and a retry creates a new set. Authentication/RBAC, encryption, changers, incrementals, native ZFS streams, automated snapshot lifecycle, web restore, monitoring, and production operations remain open.

### G4 — Compatibility claims

**Modeled only; no generation physically qualified.**

Compatibility rules are not qualification. Every untested drive, HBA, firmware, media generation, filesystem, compression mode, capacity boundary, and recovery environment must remain disclosed as unqualified.

## Required physical campaign

Before a production claim, retain evidence for at least:

1. Tape preparation, identity reread, backup, immediate readback, and unload on the exact target hardware.
2. Full filesystem restoration and manifest comparison for representative TrueNAS POSIX and NFSv4 workloads.
3. Real multi-volume operation, wrong continuation rejection, and an unexpected-capacity failure.
4. Cancellation, worker/container restart, SCSI reset, disconnect, and power-loss cases at documented boundaries.
5. Clean replacement-host recovery with no original PostgreSQL database.
6. Sustained throughput and shoe-shine measurements for large-file, small-file, compressible, and incompressible sources.
7. Retention, hold, reuse, custody, and operator-error exercises.

## Evidence report template

```text
Acceptance ID:
Date and operator:
TapeScale commit/image digest:
TrueNAS/OpenZFS/kernel/driver versions:
DAR and native-format versions:
Drive/changer/HBA/firmware/media identifiers:
Fixture version and checksum:
Preconditions:
Steps:
Expected result:
Observed result:
Artifacts and checksums:
Limitations/deviations:
Reviewer and decision:
```

Store large logs and images in the controlled evidence system and retain a checksum-addressed index. Never commit credentials, recovery keys, unredacted path inventories, or sensitive source data.

## Claim policy

- **Supported** describes implemented behavior only when its prerequisites and alpha limits are explicit.
- **Compatible** describes a modeled rule, not a physical test result.
- **Qualified** names the exact tested combination.
- **Verified** describes the exact physical byte-readback scope for one recovery point.
- **Restore tested** names the exact archive, destination, metadata scope, and date actually restored and compared.

Product copy, diagnostics, release notes, and evidence must preserve these distinctions.
