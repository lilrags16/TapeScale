# TapeScale Native v2

Status: **alpha; software write, readback, and clean-host recovery paths are implemented, but physical qualification is pending**

TapeScale `0.2.0-alpha.7` writes `tapescale-native-v2`. Native v2 replaces Native v1's complete-archive disk stage with independently finalized DAR packs in a bounded rolling spool. New writes are v2. The recovery utility keeps explicit v1 read compatibility; neither native format is compatible with the earlier Bacula experiments.

The implementation constants are normative during alpha. DAR 2.8.6 is an archive codec only; TapeScale owns cartridge identity, tape layout, spanning, verification, and archive-set completion.

## Identity and completion

The format binds four identities:

| Identity | Scope |
|---|---|
| cartridge UUID | One prepared physical cartridge |
| archive UUID | One complete backup set |
| archive-volume UUID | One physical member of that set |
| pack UUID and ordinal | One independently finalized DAR pack in archive order |

Preparation continues to write a durable TapeScale cartridge UUID at tape file zero. Before destructive I/O, the writer reads that UUID and requires it to match the reserved inventory record. Printed labels are not destructive authority.

Each pack has its own isolated DAR catalog, source allocation, slice set, lengths, and SHA-256 commitments. A pack is valid independently of later packs, but a recovery point is valid only after every planned pack and volume has passed physical readback and the final archive-set seal is present and valid. A partially written set never becomes a complete recovery point.

## Bounded rolling spool

The release defaults are:

| Control | Default |
|---|---:|
| pack target | 256 GiB (`274877906944`) |
| spool high-water | 768 GiB (`824633720832`) |
| spool low-water | 256 GiB (`274877906944`) |
| derived ready-queue resume threshold | 384 GiB (high-water minus one maximum in-progress pack) |
| filesystem free-space reserve | 8 GiB (`8589934592`) |

The worker inventories the immutable source, groups hard links, and assigns hard-link-closed manifest intervals to ordered packs. It creates, tests, hashes, and seals packs sequentially on disk. Before initial tape motion, the consumer collects validated packs to the derived resume threshold, or waits for producer completion when the entire archive is smaller. Pack production then continues concurrently with tape consumption. After a pack's complete tape files close durably, its spool files can be reclaimed; the pack remains unverified until the containing volume is rewound and passes full physical readback.

Low-water controls refill hysteresis, not merely telemetry. When validated ready bytes reach or fall below low-water, the writer stays at the clean filemark after the current pack until the queue reaches the resume threshold or the producer finishes. It never pauses halfway through a DAR slice or native record.

The spool therefore does **not** need to hold the whole archive. A 1 TiB spool supports the shipped 768/256/8 GiB policy, subject to filesystem overhead and other use. It must remain a dedicated writable POSIX-compatible filesystem outside the source, restore, and evidence trees.

### Allocation limit

One regular file, or one hard-link allocation group that must remain together, must fit both:

1. the configured pack maximum; and
2. the usable payload of one supported cartridge after native metadata and safety allowance.

TapeScale fails closed before writing that group if either condition is false. Native v2 does not split a single file or hard-link group across packs or cartridges. There is no total archive-size policy ceiling: an archive may span many packs and tapes without staging all source bytes at once.

## Pack and volume layout

Every tape member starts with a checksummed v2 header binding the archive, cartridge, archive-volume, ordinal, codec, source-manifest commitment, and format version. Pack records then carry the descriptor, isolated catalog, and complete DAR slices. The volume indexes and final archive-set seal commit to the ordered pack digests, so a missing, duplicate, foreign, or reordered pack is detectable.

Every item is written as one or more bounded physical records and closed with a tape filemark. A DAR slice is never divided between cartridges. A pack may end before a cartridge is full when that is the next safe allocation boundary.

At a volume boundary TapeScale closes and verifies the member, unloads it, and durably requests the next prepared cartridge. The next on-tape cartridge UUID is checked before overwrite. Operator wait time is not an active-transfer stall.

The last volume contains the final archive-set seal. The seal commits to the archive UUID, final pack/volume counts, ordered pack chain, source manifest, and completion facts. A trailer or per-pack index cannot substitute for the final set seal.

## Physical write and readback

TapeScale uses only the configured allowlisted non-rewinding `/dev/nstN` node and bounded 256 KiB physical writes. DAR never owns tape motion. Before writing, the worker verifies preparation identity and records the irreversible media boundary.

Each completed volume is rewound and read back immediately. TapeScale verifies record envelopes, identities, pack descriptors, catalogs, slices, indexes, digest chain, file boundaries, and the volume trailer. The final member also verifies the archive-set seal. Reclaimed spool packs are verified from physical media, not assumed to remain available on disk.

Physical readback proves that the exact encoded bytes and native metadata were readable from that drive/media pass. It does not by itself prove filesystem-level restore fidelity or qualify a hardware family.

## Recovery without PostgreSQL

`tapescale-recovery` auto-detects Native v1 and Native v2 during capture. For v2 it:

1. captures each presented volume and authenticates its header, pack records, indexes, chain, and trailer;
2. accepts volumes in presentation order independent of their declared archive order;
3. rejects duplicate, missing, foreign, or inconsistent members;
4. requires the complete ordered pack set and final archive-set seal;
5. reconstructs and tests each independently finalized DAR pack;
6. extracts into an existing empty destination and compares the result with the authenticated source manifest.

This clean-host path does not require the TapeScale database or the original spool. Keep the compatible TapeScale recovery binary, DAR 2.8.6, all cartridges, software digests, and this runbook with disaster-recovery materials.

Native v1 media remains readable through its v1 decoder. Upgrading the application does not rewrite retained v1 tapes. New operational backups and untouched queued backup requests use v2; the separate G0.2 restore-comparison exercise and terminal, running, or media-touched v1 work retain their v1 format identity.

## Failure rules

- A failure before the first media boundary leaves tape unchanged and removes unneeded temporary packs.
- After media writing starts, the touched cartridge is uncertain until reconciled or successfully verified.
- A pack may be reclaimed from disk only after every one of its complete tape files closes durably; it is not verified and cannot support a recovery point until whole-volume physical readback passes.
- Startup prefill occurs before tape motion, and later refill pauses occur only after a clean filemark; interruption never promotes a partial pack.
- Missing continuation media waits durably for the required volume.
- Unexpected end of media, malformed metadata, broken digest chains, missing final seal, or source mutation prevents recovery-point creation.
- Native v2 does not append to an interrupted archive set; retry creates a new set.
- Alpha.6 does not resume a partially written member after worker/container restart; touched media is quarantined and the retry restages from a new set.

## Alpha boundaries

Physical qualification is still pending. This release has no in-console restore or selected-file extraction, encryption, changer automation, incrementals, automatic snapshot lifecycle, or native ZFS-send format. It targets attended Full backups on one standalone SAS drive with manual tape changes.

The single-file/hard-link allocation limit is a real format constraint, not a UI limit. ACLs, xattrs, sparse files, unusual names, all compression modes, all LTO generations, wrong-media handling, controlled interruptions, refill behavior, and multi-tape clean-host recovery still require recorded physical qualification before production use.
