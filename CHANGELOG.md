# Changelog

All notable TapeScale releases are documented here. TapeScale uses Semantic Versioning prerelease identifiers; alpha releases are not production-qualified.

## [0.2.0-alpha.7] - 2026-11-19

### Native v2 upgrade enforcement

- Fixes the alpha.6 browser-cache collision that could keep serving the alpha.5 whole-archive-staging console after a container upgrade. Console assets now use a new cache key and every response is marked `no-store`, so the UI and API cannot silently drift across future redeployments.
- Normalizes every mutable operational backup profile to TapeScale Native v2 when a request is created and again when tape is assigned. A retained alpha.5 waiting request therefore cannot create another Native v1 whole-archive run; new physical work always receives layout v2 and the bounded rolling spool.
- Re-runs the safe database cutover for profiles and untouched queued work to close an old/new-container overlap window. Running, terminal, or media-touched Native v1 records remain immutable historical evidence.
- Makes retained Native v1 failures explicit in the console. The exact staging-capacity failure is no longer misreported as a drive or tape change, the historical journey is labeled as legacy whole-archive staging, and the action is **Retry with Native v2**.

The failed attempt shown by alpha.5 did not move or alter the tape: its capacity gate stopped before tape motion. After deploying alpha.7, retrying the same immutable snapshot creates a new Native v2 attempt and uses only the bounded rolling spool window.

## [0.2.0-alpha.6] - 2026-11-19

### TapeScale Native v2 rolling archives

- Makes `tapescale-native-v2` the format for new writes. Native v2 builds independently finalized DAR packs inside a bounded rolling spool instead of staging the complete archive before tape motion.
- Ships a 256 GiB pack target, 768 GiB spool high-water, 256 GiB low-water, and 8 GiB free-space reserve. A pack's disk copy is reclaimed after all of its tape files and filemarks close; later full-volume readback remains mandatory before validity, so a multi-tape archive no longer requires disk capacity equal to the full source selection.
- Prefills validated packs to the high-water mark minus one maximum in-progress pack before the first tape motion (or to producer completion for a smaller archive), then continues producing concurrently. At or below low-water it parks at a clean filemark and refills to that resume threshold, providing real startup/refill hysteresis instead of starting on the first pack. It preserves complete slices at media boundaries, immediately reads each volume back, chains ordered packs and volumes, and writes a final archive-set seal before retaining a recovery point.
- Extends clean-host recovery to auto-detect and validate Native v2 packs and the final set seal while retaining Native v1 read compatibility. New and untouched queued operational backup work migrates to v2; the separate G0.2 restore-comparison exercise and running, terminal, or media-touched v1 history keep their v1 format.

### Alpha limits

- One file, or one hard-link allocation group, must fit both the configured pack maximum and one cartridge's usable payload. The writer fails closed rather than splitting that allocation group.
- An interrupted media write is quarantined and is not resumed mid-volume; retry creates a new archive set from the immutable source.
- Native v2 remains physically unqualified. In-console restore and selected-file extraction, encryption, changer automation, incrementals, automatic snapshots, and native ZFS send remain unavailable. This alpha must not be the only copy of data.

## [0.2.0-alpha.5] - 2026-11-19

### Native live-database contract completion

- Updates the complete PostgreSQL-backed operator workflow to treat TapeScale Native v1 recovery as planning-only in the web console until the native restore worker is implemented. The independent `tapescale-recovery` full-archive path remains available without the TapeScale database.
- Makes the destructive tape-preparation feature gate fail closed for every request, including an idempotency replay, when preparation has been administratively disabled.
- Validates the cutover through the full live PostgreSQL migration and end-to-end workflow suite instead of carrying forward executable Bacula restore assumptions.
- Supersedes `v0.2.0-alpha.4`; its native container path passed, but its live catalog suite still expected the retired Bacula restore worker.

## [0.2.0-alpha.4] - 2026-11-19

### Native qualification release-fixture fix

- Updates the live PostgreSQL end-to-end qualification fixture to use the TapeScale Native v1 destructive confirmation and engine evidence contract instead of the retired Bacula values.
- Supersedes `v0.2.0-alpha.3`; its complete container/DAR validation passed, while this stale live-database fixture correctly failed closed before queueing media work.

## [0.2.0-alpha.3] - 2026-11-19

### Reproducible container and live-upgrade fixes

- Aligns both Debian container stages with the official `bookworm-20250721-slim` image used by the pinned July 2025 package snapshot, eliminating libc and Perl development-package skew without downgrading essential packages.
- Corrects the live PostgreSQL migration fixture so its native-run insertion continues to exercise the production one-active-run safety indexes instead of colliding with the earlier queued fixture.
- Supersedes `v0.2.0-alpha.2`, whose native implementation passed local validation but whose container and live-database release gates did not complete.

## [0.2.0-alpha.2] - 2026-11-19

### Release pipeline and legacy-media cutover fixes

- Restores Debian snapshot package bootstrap from the signed HTTP archive so the minimal runtime and DAR-builder stages can install `ca-certificates` before using HTTPS for the hash-pinned DAR and libthreadar source downloads.
- Retains failed Bacula-era recovery and volume rows as audit history while retiring those volume rows from active protection, reconciliation, and prepared-media checks. A non-WORM cartridge with no independent hold can therefore be reused only through the existing explicit destructive **Prepare tape** confirmation.
- Supersedes `v0.2.0-alpha.1`, whose container build could not bootstrap its CA bundle.

## [0.2.0-alpha.1] - 2026-11-19

### TapeScale Native v1

- New backups no longer depend on Bacula. TapeScale inventories and hashes the immutable source, uses DAR 2.8.6 as a separate archive-codec process, and retains TapeScale-owned media, archive, volume, segment, and verification records.
- Small-file workloads are packed into large, checksummed DAR slices before the tape moves. The complete archive is staged and tested on disk first, so source traversal, metadata latency, and compression cannot starve an active LTO write.
- The native tape layout writes a versioned binary header, isolated DAR catalog, authenticated source manifest, whole archive slices, volume index, and trailer as separate tape files. Every record has an envelope checksum, and the header, index, and trailer bind the same archive and volume identities.
- Multi-tape archives split only between complete slices. Each continuation has its own UUID and ordinal, is associated with the cartridge UUID read from the preparation header, and is immediately rewound and read back in full before it can be retained as verified.
- Tape preparation and qualification are native operations. A successful destructive qualification writes and reads deterministic records across two tape files, then writes a durable TapeScale preparation header containing the cartridge UUID. The worker reads that identity directly at every media boundary instead of asking an operator to repeatedly attest which tape is loaded.
- Live progress distinguishes source inventory, archive packing, disk validation, tape write, tape readback, current source path, staged bytes, physical tape rates, and attended volume waits.

### Breaking format transition

- `tapescale-native-v1` is a new and incompatible on-tape format. This deployment has no valid Bacula recovery media to preserve, so saved backup profiles and untouched queued requests migrate to the native engine. Every legacy Bacula recovery point is demoted from verified to failed while its rows remain as audit history; it can no longer appear valid or protect cartridges from reuse.
- Blank and expendable media must be prepared again before a native backup. Preparation is destructive. Never prepare a cartridge containing data that must be preserved.
- The runtime image no longer needs Bacula for new backup or qualification work. DAR remains a separately executed GPL-2.0-or-later program; TapeScale itself remains Apache-2.0.

### Alpha limits

- Native writes, spanning, and immediate physical readback are implemented. The independent `tapescale-recovery` utility can capture every ordered native volume on a clean host, authenticate all metadata and slices, rebuild the DAR archive, extract it into an empty destination, and compare the result with the captured source manifest. That CLI path is not yet physically qualified or wired into the web worker; selected-file recovery remains unavailable. Do not rely on this alpha as the only copy of data.
- The first release is for attended, manually started Full backups on one standalone SAS tape drive. Incrementals, encryption, changers, automatic snapshot creation, native ZFS send streams, and production qualification remain open.
- DAR archive fidelity and the TapeScale Native layout still require real-hardware qualification across supported filesystems, ACL/xattr combinations, drive generations, compression modes, volume boundaries, wrong-media handling, and controlled interruptions.

## [0.1.0-alpha.15] - 2026-11-18

### Database-backed identity fix

- Corrects the loaded-tape health merge statement found by the PostgreSQL-backed release test, allowing SAS drive observations and automatic on-tape identity state to persist correctly.

## [0.1.0-alpha.14] - 2026-11-18

### Automatic on-tape identity

- A successful **Prepare tape** run now writes and immediately reads back a durable internal `TS-PREP-V1-<cartridge UUID>` volume identity after the destructive setup test. The worker retains that exact identity against the inventory record and periodically probes an idle loaded drive, so a prepared cartridge can identify itself without repeated printed-label attestations.
- The first operational Bacula label replaces the preparation label as the cartridge's current internal identity. TapeScale retains both the cartridge association and each archive volume label, and re-reads the loaded label immediately before every destructive write or physical readback boundary. A missing, foreign, ambiguous, or wrong label blocks the operation before data I/O.
- Drive discovery preserves a freshly observed loaded-tape identity only while the same loaded device remains continuously observed. Unload, device loss, identity conflict, or failed probing clears or invalidates the hint; the just-in-time label probe remains authoritative at the physical boundary.

### One-action standalone-drive workflow

- Tape preparation, initial backup assignment, continuation, and readback now present the one relevant drive/tape fact and one explicit action instead of exposing duplicated checkboxes and typed confirmation phrases. The API and worker still repeat every eligibility, lease, identity, and destructive-boundary check.
- A one-drive deployment automatically narrows eligible media to the cartridge identified from tape. The running job continues to retain ordered volume requests and blocks a wrong cartridge, but the operator no longer has to manually reassert the identity already stored on the media.
- Existing tapes prepared by an earlier alpha do not contain the new preparation identity. Before their next destructive write, run **Prepare tape** once with a blank or expendable scratch cartridge; that operation overwrites the tape. Never re-prepare a retained archive. Previously written Bacula volumes can still be resolved by their retained operational volume labels when those records are present.

### Engine boundary

- Bacula remains the alpha's pinned physical archive adapter for write, spanning, catalog, readback, and restore behavior. The console no longer treats Bacula's confirmation mechanics as the product workflow, but this release does not claim a new native packed archive format or remove the Bacula catalog dependency.

## [0.1.0-alpha.13] - 2026-11-18

### Anti-shoe-shine tape delivery

- **Whole-archive staging** is now the safe default for new and existing saved backup policies. TapeScale budgets a conservative stage target of 125% of logical payload plus 64 KiB per entry and 4 GiB of framing, separate from attribute and filesystem reserves. It blocks before dispatch when that point-in-time estimate cannot fit and cancels near the hard limit or on observed early drain rather than knowingly continuing cyclic partial batches. Already assigned runs retain their immutable delivery choice.
- A spool maximum of `0` now selects all currently usable staging capacity after attribute and filesystem reserves. The TrueNAS deployment bind-mounts the required dedicated `/mnt/tapescale-spool` dataset instead of hiding staging inside a Docker-managed 64 GiB batch buffer.
- Direct source-to-tape delivery remains an explicit advanced option for sources proven to sustain the drive. The console warns that slow or small-file workloads can still starve and backhitch the drive; TapeScale does not yet claim a packed-object small-file format.
- The generated Bacula runtime and destructive `btape` setup test use a 256 KiB maximum tape block size. Existing installations must rerun **Prepare tape** with blank/scratch media before its next write so the exact drive/media pair exercises the new physical record ceiling. Existing archives remain readable and must not be destructively prepared.

### Operator feedback and compatibility

- Live backup status distinguishes complete-archive staging from the sustained tape pass and labels staging capacity, occupancy, and tape-wait behavior without presenting any of them as archive completion.
- Upgrade migration `0018` moves saved direct policies to the safe whole-stage default while preserving already assigned run evidence and permitting an operator to deliberately select direct delivery again.

## [0.1.0-alpha.12] - 2026-11-18

### Operator command center

- Overview now opens with one prioritized next action and a unified queue of backup work plus system alerts. Tape handoffs lead, then media/custody failures and unresolved backup attempts; healthy active transfers remain visible without inflating the alert badge.
- Backup Center places **Needs you** and **In progress** ahead of saved selections, shows live phase/tape/progress facts, and offers direct continuation, retry, and newer-snapshot paths. History filters use operator outcomes—active, waiting on me, needs attention, protected, and cancelled—instead of raw engine states.
- Saved items are explicitly described as exact snapshot selections. Repeating one is labeled **Back up same snapshot**; existing scheduled records disclose that they repeat that exact snapshot rather than silently implying rolling protection.
- The responsive console gains a real mobile **More** navigation control, current-work badges with accessible labels, stacked narrow-screen actions, operational quick links, and a compact assurance card instead of leading day-to-day operations with the G0.2 laboratory workflow.

### Action trust and refresh behavior

- Only the latest attempt for each job contributes a failed/waiting required action, so a verified retry clears the earlier alarm and repeated failures do not inflate counts. Backup actions now deep-link to the exact request while distinct system actions sharing a top-level route remain visible.
- Frontend work items merge fresher polled backup state with the bounded server action feed, deduplicate exact deep links before urgency sorting, and never let a stale waiting action override a run that has already resumed.
- Manual refresh now requests only the endpoint group used by the current route. Active-backup polling refreshes the run summary and selected live detail, fetches drive/media state only during a tape handoff, and refreshes overview/recovery points once when a run becomes terminal.

## [0.1.0-alpha.11] - 2026-11-14

### Honest live backup progress

- Active write and physical-readback phases now retain monotonic archive-entry progress instead of comparing Bacula's encoded stream bytes with source logical bytes. Write progress uses the correlated Bacula `JobFiles` count against the immutable manifest entry count; readback progress uses the verification job's `JobFiles` count against the completed backup job's entry count.
- The running-job page presents that value as **archive entries reached/read back**, reports fractional entry rates for slow metadata-heavy workloads, caps an active phase below 100% until the correlated engine job actually completes, and keeps Bacula stream bytes plus physical tape rates as separate telemetry. It does not derive a byte ETA from counters with incompatible compression, sparse-file, or hard-link semantics.
- The exact correlated File Daemon status supplies the currently processed source-relative filename while a sample is fresh. The complete path wraps on narrow screens instead of hiding the basename. It is bounded, lease-fenced, and live-only: it is not copied into terminal evidence, audit history, or the retained recovery point.
- Transfer position survives UI refreshes through a durable monotonic snapshot, while stale samples, operator media waits, cancellation requests, and completed phases retain their honest state instead of appearing active or regressing to an earlier step.

### Complete running-job experience

- The backup page now distinguishes source scanning, manifest sorting/writing/digesting, writing, ordered media waits, physical readback, and cooperative cancellation. Phase aliases and event evidence resolve to the same canonical timeline, including terminal readback failures.
- Live metrics identify Bacula write/read streams, physical tape direction, disk-buffer occupancy when staging is enabled, current tape, entry position, and current filename without labeling any of them as raw ZFS throughput or logical-byte completion.
- Automatic refresh preserves the selected run, expanded controls, focused input, and cursor position. A terminal list result supersedes an older cached detail response, and cancellation controls disappear once a cancellation is already pending.

### Small-file boundary

- This release makes per-file traversal visible but does not introduce a packed-object archive format. Bacula still walks and records the selected filesystem entries individually, preserving manifest browsing, metadata, hard links, ordered multi-tape restore, and physical readback semantics. Workloads dominated by tiny files can therefore remain source-metadata/engine limited; a future packed-stream mode requires an explicit restore-format and recovery-browser design rather than an unverified throughput shortcut.

## [0.1.0-alpha.10] - 2026-11-14

### Continuous tape delivery

- Backup plans now select either **Continuous streaming** or **Disk staged** delivery. Continuous streaming is the default for fast local storage: Bacula sends the processed File Daemon stream directly to the Storage Daemon and tape instead of first writing and then rereading a disk data spool.
- Direct delivery keeps Bacula attribute spooling and the run-scoped working directory, but it does not require the 8 GiB minimum data spool, collect misleading data-spool occupancy, or wait for a segment to fill before tape writing can begin.
- Disk-staged delivery remains available for slow, bursty, or remote sources when the spool is on storage fast enough to drain at the tape's streaming rate. It retains the existing bounded fill-and-drain behavior; it is disk staging, not a continuously consumed RAM ring.
- The selected delivery mode is copied into each physical run and retained with its engine and recovery evidence. An already-running job cannot change modes; a new media assignment uses the current plan choice.
- Live status distinguishes direct source-to-tape streaming from disk fill/despool activity. Direct mode removes spool double I/O and batch pauses, but it cannot prevent shoe-shining if source traversal plus compression remains below the drive's minimum sustained speed.

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

[0.2.0-alpha.7]: https://github.com/lilrags16/TapeScale/releases/tag/v0.2.0-alpha.7
[0.2.0-alpha.6]: https://github.com/lilrags16/TapeScale/releases/tag/v0.2.0-alpha.6
[0.2.0-alpha.5]: https://github.com/lilrags16/TapeScale/releases/tag/v0.2.0-alpha.5
[0.2.0-alpha.4]: https://github.com/lilrags16/TapeScale/releases/tag/v0.2.0-alpha.4
[0.2.0-alpha.3]: https://github.com/lilrags16/TapeScale/releases/tag/v0.2.0-alpha.3
[0.2.0-alpha.2]: https://github.com/lilrags16/TapeScale/releases/tag/v0.2.0-alpha.2
[0.2.0-alpha.1]: https://github.com/lilrags16/TapeScale/releases/tag/v0.2.0-alpha.1
[0.1.0-alpha.15]: https://github.com/lilrags16/TapeScale/releases/tag/v0.1.0-alpha.15
[0.1.0-alpha.14]: https://github.com/lilrags16/TapeScale/releases/tag/v0.1.0-alpha.14
[0.1.0-alpha.13]: https://github.com/lilrags16/TapeScale/releases/tag/v0.1.0-alpha.13
[0.1.0-alpha.12]: https://github.com/lilrags16/TapeScale/releases/tag/v0.1.0-alpha.12
[0.1.0-alpha.11]: https://github.com/lilrags16/TapeScale/releases/tag/v0.1.0-alpha.11
[0.1.0-alpha.10]: https://github.com/lilrags16/TapeScale/releases/tag/v0.1.0-alpha.10
[0.1.0-alpha.9]: https://github.com/lilrags16/TapeScale/releases/tag/v0.1.0-alpha.9
[0.1.0-alpha.8]: https://github.com/lilrags16/TapeScale/releases/tag/v0.1.0-alpha.8
[0.1.0-alpha.7]: https://github.com/lilrags16/TapeScale/releases/tag/v0.1.0-alpha.7
[0.1.0-alpha.6]: https://github.com/lilrags16/TapeScale/releases/tag/v0.1.0-alpha.6
[0.1.0-alpha.5]: https://github.com/lilrags16/TapeScale/releases/tag/v0.1.0-alpha.5
[0.1.0-alpha.4]: https://github.com/lilrags16/TapeScale/releases/tag/v0.1.0-alpha.4
[0.1.0-alpha.3]: https://github.com/lilrags16/TapeScale/releases/tag/v0.1.0-alpha.3
[0.1.0-alpha.2]: https://github.com/lilrags16/TapeScale/releases/tag/v0.1.0-alpha.2
[0.1.0-alpha.1]: https://github.com/lilrags16/TapeScale/releases/tag/v0.1.0-alpha.1
