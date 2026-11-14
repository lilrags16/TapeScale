# Changelog

All notable TapeScale releases are documented here. TapeScale uses Semantic Versioning prerelease identifiers; alpha releases are not production-qualified.

## [0.1.0-alpha.9] - 2026-11-14

### Sustained-transfer visibility and defaults

- New backups using **Automatic** compression now resolve to Bacula LZO instead of GZIP1. Explicit balanced GZIP, LZO, and uncompressed choices remain available and recorded; the codec of an active job cannot be changed in place.
- Every Bacula job receives an isolated run-scoped spool directory. The live backup page reports that directory's aggregate regular-file occupancy, configured ceiling, fill/drain rate, and file count without exposing a host path or filename.
- Bacula Storage Daemon running-job evidence now supplies exact JobId-scoped bytes, files, state, average/current throughput, and spooling/despooling flags. The UI keeps that logical engine flow separate from physical SCSI tape counters and no longer presents either metric as a raw ZFS read rate.
- Running backups identify starting, buffering, streaming, despooling, reading, waiting, idle, and stalled activity. A heartbeat does not count as movement: the page shows the age of the last measured byte/state change and the remaining automatic-cancel window.
- The transfer watchdog now resets only for measured engine, spool, or tape progress. A configurable 30-minute default replaces the previous 12-hour catalog-only wait; `TAPESCALE_TRANSFER_STALL_TIMEOUT_SECS` accepts 300 through 7,200 seconds.

### Truthful spool behavior

- Operator guidance now describes Bacula's spool as segmented fill-and-drain buffering, not a continuously refilling ring. A 64 GiB segment can take hours to produce when traversal or compression is slow and can then drain to LTO much faster than it refills.
- Reaching the spool ceiling still continues the same archive and is not an archive-size limit. A larger segment reduces transition frequency but cannot make a producer sustain a tape drive when its long-term throughput is below the drive's minimum streaming rate; the live metrics now expose that condition directly.

## [0.1.0-alpha.8] - 2026-11-13

### Visible backup preparation

- Backup preflight and source inventory are now separate durable phases. A run no longer remains under the vague **Starting backup** label while TapeScale reads the entire snapshot.
- The running-job view refreshes aggregate source progress every five seconds: SHA-256 bytes read, current read rate, files hashed, files and entries discovered, growing logical bytes, elapsed time, last update, and the current scan/sort/write/digest step.
- The same truthful progress panel covers the post-readback source-stability pass. Stale rates are hidden while cumulative work remains visible, and the UI explicitly avoids a completion percentage until an authoritative total exists.
- Live preparation samples are lease-fenced, bounded, path-free, and ephemeral. They never expose the current filename, create high-frequency audit events, or replace the retained final manifest evidence.

### Faster large manifests

- Streaming sort batches increased from 1,024 to at most 16,384 entries, sharply reducing temporary runs and merge passes. A separate 32 MiB resident-entry budget keeps unusually long paths and symlink targets within a predictable memory envelope.
- Disposable sort runs and hard-link workspace mappings no longer force thousands of unnecessary filesystem syncs. The final retained manifest is still flushed and synced before it is published.
- The source scanner reuses one 1 MiB hashing buffer across files instead of allocating and zeroing it for every file, and long sort/merge and final-digest work now honors cancellation.
- Bacula catalog lock acquisition is bounded so a competing or stale initializer cannot leave preflight waiting indefinitely.

## [0.1.0-alpha.7] - 2026-11-13

### Live transfer telemetry

- Running Backup, recovery-exercise, and Restore views now refresh a prominent live transfer panel while the worker is active.
- Tape write and read rates come from the Linux SCSI tape driver's cumulative byte counters for the exact configured and identity-checked drive. Logical Bacula job progress remains separately labeled and is never presented as disk or physical tape I/O.
- Each direction reports its own availability. A measured idle interval displays `0 B/s`; missing counters display **Unavailable**; the first sample displays **Measuring**; and stale samples are hidden instead of being repeated as current rates.
- Live snapshots remain lease-fenced, replace only ephemeral run state, and are cleared at every physical phase transition. They do not create high-frequency audit events or survive as terminal evidence.

## [0.1.0-alpha.6] - 2026-11-13

### Truthful cartridge status

- The Media Control Center no longer presents the conservative destructive-reuse gate as every cartridge's operational status. Registered media, prepared scratch tapes, verified archives, protected media, reservations, and genuine review states now have distinct headlines.
- Prepared scratch media explicitly explains that its Bacula on-tape identity is intentionally pending until the first verified backup. The strict reuse gate remains unchanged and is displayed separately as a destructive-reuse assessment.
- Blank registered and successfully qualified scratch cartridges no longer create impossible “Reconcile cartridge identity” actions. Foreign or mismatched identity, unsafe health, recorded content with unknown identity, and missing custody for finalized content remain actionable.
- Static asset revision keys advance with the release so TrueNAS browsers load the corrected cartridge UI immediately after redeployment.

## [0.1.0-alpha.5] - 2026-11-13

### Long-running backup lease reliability

- Backup heartbeats now renew the exact first-cartridge lease as well as the drive and every continuation cartridge. A copy/paste regression had queried the unrelated Restore ledger, allowing volume one to expire 30 minutes after assignment during a long source inventory.
- The worker renews its fenced drive and cartridge leases between every bounded pre-write rewind, scratch-header preparation, and final rewind, including continuation-tape preparation.
- Database regression coverage shortens both initial leases and proves that one heartbeat extends both beyond the old failure window.

### Truthful interruption evidence

- A lease loss before the durable media-write boundary now records that no tape-motion or data-write command was dispatched, preserves the drive's reconciled position, and leaves cartridge health unchanged.
- A lease loss after that boundary continues to fail closed: tape position becomes uncertain and every started cartridge is quarantined as write-unverified.
- Backup details render the retained failure explanation instead of raw JSON and give operators tape-state-specific recovery guidance.

## [0.1.0-alpha.4] - 2026-11-13

### Path-selective Restore runs

- Recovery Vault can now turn retained-manifest browser selections into an attended restore plan for one or more files, symlinks, or recursive directories.
- The API verifies the retained manifest, removes redundant descendant requests, includes every member of a selected hard-link group, and persists the normalized scope, selected-manifest digest, and deterministic selection hash. The worker repeats that resolution before reading media.
- Bacula catalog lookups are parameterized and pinned to the original backup JobId. Only server-resolved numeric FileIndex and PathId values enter the restricted BVFS restore-table commands; operator-selected source paths never enter `bconsole`.
- Selected-path execution is intentionally limited to a recovery point with exactly one retained volume. Multi-volume selected plans are blocked until TapeScale can retain an authenticated path-to-volume map and truthful per-volume read evidence; complete-point multi-volume restore remains available. A selected run restores only into a new isolated run directory and compares the recovered selected scope with the authenticated manifest projection.
- Selected-path passes append `selective_restore_test` evidence and cannot satisfy the complete-point `restore_test` gate.

### Alpha boundaries

- Selective execution is limited to compatible, single-volume, unencrypted Full file points with the original Bacula catalog. Native streams, incrementals, encrypted or multi-volume points, lost-catalog recovery, destination replacement/merge, and automatic publication remain planning-only, blocked, or unavailable.
- Path selection does not qualify an engine or format or prove clean-host recovery. Container CI exercises Bacula's BVFS extraction mechanism in spanning mode, but that is not evidence that TapeScale can yet execute a truthful multi-volume selected run.

## [0.1.0-alpha.3] - 2026-11-13

### Sustained tape writing

- Bacula Full jobs now use the existing persistent spool volume instead of feeding tape directly from filesystem traversal and compression.
- The worker proves spool writability and current capacity before the destructive media boundary, preserves an 8 GiB free-space reserve, and selects an effective buffer up to 64 GiB by default.
- Reaching the spool ceiling despools and continues the same archive; it is not a backup-size limit. Run evidence records the effective ceiling and the console explains initial buffering without inventing progress.
- Bacula's one-gigabyte tape-file default is overridden with a 32 GiB logical-file boundary, reducing EOF/filemark stops on modern LTO without limiting the archive or cartridge size.
- Bacula's automatically enabled attribute spool now uses the same measured spool filesystem. Preflight reserves a conservative allowance from manifest size and entry count in addition to the configured free-space reserve before setting the data-spool ceiling.
- The file-backed Bacula CI roundtrip now validates the same `Spool Data`, spool-directory, spool-bound, and tape-file-boundary Device directives.

### Ordered-volume Restore runs

- Compatible operational Full points can now be restored across their complete ordered cartridge set; legacy G0.2 points remain one volume.
- Queueing atomically reserves every required tape and snapshots its inventory, on-tape, Bacula MediaId/label, and record-coordinate binding into a durable restore ledger.
- The active Bacula restore pauses for exact read-only handoffs. Each next tape requires `LOAD <label> FOR RESTORE <run UUID> VOLUME <number>`, a new media fence, a fresh drive probe, rewind, and mount.
- Cancellation, lease loss, wrong order/media, and worker restart fail closed. A pass requires every ordered volume completed, no open handoff, exact all-volume engine/report evidence, and the independent restored-manifest comparison.
- Recovery Vault now presents the ordered tape sequence, current/next tape, durable waiting state, and exact handoff modal on desktop and mobile.

### Alpha boundaries

- Spooling, compression, end-of-media handling, and multi-volume restore are implemented but not physically qualified across the hardware matrix.
- Restore remains complete-job, unencrypted, catalog-dependent, attended, and isolated; selective and clean-catalog recovery remain unavailable.

## [0.1.0-alpha.2] - 2026-11-13

### Restore workflow

- Recovery Vault can now start an attended, read-only Restore run for a verified single-volume operational Full backup as well as a legacy G0.2 recovery point.
- The first Restore run no longer requires a prior passing restore test; a passing run creates that independent `restore_test` verification after Bacula restores the complete JobId and TapeScale matches the authenticated manifest.
- Restored-tree inventory now uses the bounded-memory external sorter and no longer inherits the G0.2 100,000-entry or 100 GiB policy ceiling.
- Planning, queueing, worker claim, and finalization recheck the exact origin workflow, full-readback record, drive, cartridge, Bacula JobId/catalog, media identity, manifest, destination, lease, and preview hash.
- Recovery Vault now presents this as a four-stage Restore run and clearly identifies its full-point, one-cartridge, isolated-destination boundary.

### Alpha boundaries

- Multi-volume, path-selective, encrypted, and clean-catalog restore remain unavailable. Authenticated manifest decoding retains a 256 MiB serialized-artifact safety bound.
- This release remains for attended evaluation on a trusted isolated network and makes no production or broad physical-qualification claim.

## [0.1.0-alpha.1] - 2026-11-13

First public alpha release.

### Operator workflows

- Graphical browsing and selection of existing read-only ZFS snapshot directories.
- Durable attended Full file backups through Bacula Community 15.0.3.
- Automatic fast GZIP, balanced GZIP, LZO, or uncompressed archives.
- Ordered multi-cartridge continuation in a standalone SAS drive with a separate confirmation for every destructive media boundary.
- Complete physical Data readback of every recorded volume before a recovery point is marked verified.
- Graphical retained-manifest browsing, recovery-point dossiers, restore-readiness planning, media holds, pools, and custody movements.
- Guided tape preparation, exact drive/cartridge qualification, guarded rewind/unload, and the narrower G0.2 write/readback/restore/compare exercise.

### Release engineering

- One consistent `0.1.0-alpha.1` identity across Rust binaries, API responses, OCI labels, local Compose, TrueNAS Compose, and the recovery fixture.
- TrueNAS deployment is pinned to the immutable `v0.1.0-alpha.1` source tag.
- CI validates PostgreSQL migrations, Rust tests/lints, web contracts, Compose rendering, container hardening, and a restarted source-independent Bacula file-backed restore.
- CI now tests release tags and rejects version/tag drift.

### Alpha boundaries

- Run only on an isolated, attended network. There is no application authentication, TLS termination, or multi-user authorization yet.
- Backups are Full, manually started, and unencrypted. TapeScale does not create or retain snapshots, and incrementals and changer automation are unavailable.
- Backup spanning and compression are implemented but not yet physically qualified across the supported hardware matrix.
- The general backup path proves complete write/readback, not a restore. The separate G0.2 recovery exercise remains single-cartridge and bounded.
- Clean-host catalog reconstruction, independent key recovery, native ZFS streaming, path-selective restore, and production qualification remain open.

[0.1.0-alpha.9]: https://github.com/lilrags16/TapeScale/releases/tag/v0.1.0-alpha.9
[0.1.0-alpha.8]: https://github.com/lilrags16/TapeScale/releases/tag/v0.1.0-alpha.8
[0.1.0-alpha.7]: https://github.com/lilrags16/TapeScale/releases/tag/v0.1.0-alpha.7
[0.1.0-alpha.6]: https://github.com/lilrags16/TapeScale/releases/tag/v0.1.0-alpha.6
[0.1.0-alpha.5]: https://github.com/lilrags16/TapeScale/releases/tag/v0.1.0-alpha.5
[0.1.0-alpha.4]: https://github.com/lilrags16/TapeScale/releases/tag/v0.1.0-alpha.4
[0.1.0-alpha.3]: https://github.com/lilrags16/TapeScale/releases/tag/v0.1.0-alpha.3
[0.1.0-alpha.2]: https://github.com/lilrags16/TapeScale/releases/tag/v0.1.0-alpha.2
[0.1.0-alpha.1]: https://github.com/lilrags16/TapeScale/releases/tag/v0.1.0-alpha.1
