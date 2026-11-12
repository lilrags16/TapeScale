# Tape engine evaluation gate

Decision status: **open; no production engine or on-tape format is qualified**.

The requirements identify Bacula Community as the initial feasibility candidate, not a pre-certified dependency. TapeScale must reuse a mature engine's records, spanning, checksums, spooling, and restart boundaries unless test evidence demonstrates that no qualified format can satisfy the contract. This repository's simulator is not an on-tape format and is not a production engine.

## Gate question

Select and pin an engine release and durable format only after a named TrueNAS host, real drives/changer, and designated scratch tapes demonstrate the contract below. The result is an architecture decision record containing versions, licenses, packaging, privileges, structured interfaces, failure evidence, and the final go/no-go decision.

## Candidate set

### Bacula Community 15 feasibility candidate

The Bacula 15 documentation referenced by the specification is the initial study baseline. The eventual package release and digest must be pinned only after qualification. Advantages to investigate:

- established file backup, media pools, multi-volume operation, catalog, Accurate-mode namespace tracking, checksums, spooling, and recovery tools;
- documented tape and changer support;
- an independently installable recovery path.

Questions that must be answered with pinned-version evidence:

- Is there a supported structured control surface for every required operation, or would critical state require parsing presentation-oriented `bconsole` output?
- Does Accurate mode reproduce deletions, renames, directory changes, and metadata-only changes on the TrueNAS fixture?
- Are TrueNAS NFSv4 ACL inheritance and allow/deny semantics preserved, not merely generic ACL flags?
- How are arbitrary native ZFS send streams represented without replacing Bacula's proven tape-record layer?
- What are the documented durable restart units for files, native streams, volume changes, verification, and catalog finalization?
- Can a clean host rebuild or import the necessary catalog from tape-resident and separately exported metadata?
- Which encryption modes cover filenames, paths, and catalog exports, and how are keys recovered independently?

### Bareos

Evaluate under exactly the same fixture and evidence rubric. Do not infer compatibility from shared ancestry or documentation. Record the supported API/control surface, licensing, packaged versions, recovery tooling, Accurate-equivalent semantics, ACL behavior, and format support.

### Other established engines

Additional engines may enter the gate if they support Linux tape libraries, multi-volume recovery, catalog reconstruction, exact incrementals, and maintained independent restore tooling. Cloud-only, LTFS-only, or LTO-5-and-later-only paths cannot satisfy the all-generation baseline.

## Mandatory laboratory fixture

Use TrueNAS 25.10.8 on x86-64 initially and record exact OpenZFS, kernel, tape driver, engine, image, drive, changer, firmware, HBA, and media versions. The source fixture includes:

- POSIX ACL and native TrueNAS NFSv4 ACL datasets with inheritance and allow/deny cases;
- empty, small, large, sparse, hard-linked, symlinked, extended-attribute, unusual-name, and unreadable files;
- additions, content changes, moves, deletions, directory changes, and metadata-only changes across snapshots;
- a native filesystem dataset and a zvol, both full and incremental;
- a file and a native stream larger than one cartridge;
- compressible, incompressible, already compressed, and raw-encrypted data;
- wrong, unknown, protected, WORM where available, and physically full media;
- a library path with slots/import-export where available and a standalone manual-drive path.

## Required demonstrations

The gate cannot pass on documentation or simulation alone. Retain logs, manifests, hashes, screenshots, media inventory, and restore comparisons for:

1. File full/incremental backup and exact dated reconstruction.
2. POSIX and NFSv4 ACL/metadata round trips on the actual TrueNAS dataset.
3. Native ZFS full/incremental send and receive, including zvol.
4. Multi-volume write and restore with physical end-of-media.
5. Wrong continuation tape rejection without modifying it.
6. Failed source/producer propagation through the engine.
7. Cancellation and power/process interruption at documented boundaries.
8. Actual read-back verification through the recovery decoding path.
9. Software encryption and independent historical-key recovery.
10. Clean replacement host catalog-loss recovery without the original database.
11. Stable logical source identity across changing snapshot mount paths.
12. Safe device ownership and changer coordination across restart.

## Scoring rubric

Each candidate is scored pass/fail for mandatory items and comparatively for operational quality:

| Area | Mandatory evidence |
|---|---|
| Format durability | Versioned maintained format; independent reader/tools; prior-format policy |
| Exact recovery | Complete namespace and metadata fixture reconstructed |
| TrueNAS fidelity | Real POSIX/NFSv4 ACL, xattr, links, sparse behavior |
| Native ZFS | Full/incremental dataset and zvol round trips |
| Media behavior | Physical spanning, EOM, wrong media, unknown media, WORM handling |
| Failure truth | Producer errors, resets, timeouts, cancellation, restart states |
| Control interface | Supported structured interface with stable identifiers |
| Catalog loss | Clean-host identification, import/rebuild, restore |
| Encryption | Authenticated format, metadata coverage, portable key recovery |
| Operations | Packaging, diagnostics, performance, licensing, security updates |

Any mandatory failure is a no-go or a documented release-blocking gap. A workaround that replaces the engine's record layer creates a new format qualification burden and is not accepted by convenience.

## Adapter acceptance

After engine selection, pin:

- engine version and package/image digest;
- engine catalog schema ownership and supported interface;
- adapter protocol version;
- on-tape and manifest format identifiers;
- required external tools and versions;
- privileges, devices, mounts, and network flows;
- timeout, cancellation, restart, and reconciliation semantics;
- clean-host installation and recovery artifacts.

The adapter must prove hostile argument handling, missing tools, hung commands, cancellation, and failed producer/consumer propagation as required by AT-30.

## Current conclusion

No engine is selected and no physical evidence is present. Production writes must remain disabled or clearly gated until this document is replaced by a signed G0 decision record backed by the named laboratory evidence.
