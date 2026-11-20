# Tape engine decision record

Decision status: **TapeScale Native v2 selected for the `0.2.0-alpha.11` architecture; physical production qualification remains open**.

## Decision

The Bacula-backed alpha path failed to provide the performance, operational simplicity, identity model, and dependable recovery contract required for TapeScale. The deployment has no valid Bacula recovery media, so the `0.2.0` native alpha line makes a clean, intentionally incompatible transition:

- TapeScale owns the on-tape format, cartridge/archive identities, volume ordering, motion, direct device I/O, physical byte readback, evidence, and recovery contract.
- DAR 2.8.6 is a pinned, separately executed archive codec used to package and restore file trees.
- The source is split into independently tested DAR packs produced in bounded disk batches; a 1 TiB spool does not need to hold a 30 TiB archive.
- Every native volume carries an authenticated header and source manifest, ordered pack descriptors/catalogs/slices, a chained index, and a trailer; the final member carries the archive-set seal.
- A clean-host `tapescale-recovery` utility captures and authenticates tapes, reconstructs the archive, extracts it, and compares the restored tree without PostgreSQL.
- The runtime image contains no Bacula package, daemon, console, or catalog utility.

The exact media contract is documented in [NATIVE_FORMAT.md](NATIVE_FORMAT.md). Implementation and service boundaries are documented in [ARCHITECTURE.md](ARCHITECTURE.md).

## Why this direction

The new architecture directly addresses the observed failure modes:

- Independently finalized DAR packs turn tiny files into long sequential tape files while bounded disk absorbs short source, metadata, and compression stalls.
- Exclusive production and drain batches avoid rotational seek contention while ready/low/high watermarks bound disk use. Pack production below the drive's consumption rate causes an intentional safe-filemark pause and must be measured physically.
- TapeScale reads identity from media at destructive boundaries instead of asking the operator to attest repeatedly to the only loaded cartridge.
- Native headers and trailers make a cartridge self-describing and bind it to one archive and volume ordinal.
- Immediate readback verifies the bytes actually readable from every written tape.
- Database-independent recovery avoids making the original service catalog a prerequisite for disaster recovery.

## Rejected active alternatives

### Bacula Community

Bacula was the earlier feasibility engine. It is no longer an active runtime or recovery dependency. Its prior jobs and catalog fields can remain in the database as historical audit records, but they are not native recovery points and the native image cannot read Bacula media.

### Amanda/Zmanda and Bareos

These remain possible external tools for separate deployments, but adopting either would reintroduce an engine catalog, adapter lifecycle, media conventions, and operator surface that TapeScale would still need to reconcile. Neither is part of Native v2 and neither can read its media without a TapeScale-aware reader.

### LTFS

LTFS does not cover the intended LTO-1-through-current generation model and does not by itself solve multi-volume archive identity, complete-set validation, clean-host catalog reconstruction, or the required recovery evidence. It is not the Native v2 format.

## Mandatory qualification before production claims

The architecture decision does not constitute physical qualification. The named TrueNAS host and supported hardware must demonstrate:

1. representative file and metadata round trips, including POSIX/NFSv4 ACLs, xattrs, hard links, symlinks, sparse files, and unusual names;
2. sustained native write/read throughput without pathological backhitching;
3. real multi-volume backup and clean-host restore across physical capacity boundaries;
4. rejection of wrong, foreign, protected, reordered, corrupt, truncated, missing, and duplicate media;
5. safe behavior across cancellation, process/container restart, device reset/disconnect, and power interruption;
6. a complete replacement-host recovery with no original database;
7. operational retention, custody, reuse, and diagnostics evidence.

The current status for each acceptance case is maintained in [QUALIFICATION.md](QUALIFICATION.md).

## Alpha limits

Native v2 currently targets attended Full backups to one standalone SAS tape drive. It is unencrypted. It does not yet provide incrementals, changers, automatic snapshot lifecycle, native ZFS streams/zvols, selected-file restore, or web-operated restore. Physical EOM is treated as a failed uncertain volume rather than a normal spanning signal; planned volume placement uses conservative generation capacity and keeps packs/slices whole. Worker restart does not resume or append a partially written member: it is quarantined and a retry restages a new archive set.

Do not rely on this alpha as the only copy of data. Do not destructively prepare a cartridge that may contain data worth preserving in another format.

## Superseded research record

The pre-`0.2.0` repository evaluated Bacula, Bareos, and other established engines under a G0 laboratory gate. That research is superseded as an implementation decision by TapeScale Native v2. Historical commits and the changelog preserve why the earlier adapter existed; current operator documentation must not present it as enabled.
