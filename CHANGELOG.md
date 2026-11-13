# Changelog

All notable TapeScale releases are documented here. TapeScale uses Semantic Versioning prerelease identifiers; alpha releases are not production-qualified.

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

[0.1.0-alpha.2]: https://github.com/lilrags16/TapeScale/releases/tag/v0.1.0-alpha.2
[0.1.0-alpha.1]: https://github.com/lilrags16/TapeScale/releases/tag/v0.1.0-alpha.1
