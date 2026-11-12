# Independent recovery runbook

Status: recovery contract for the engineering preview. A clean-host physical recovery has not been run. Do not treat this document as G2 evidence.

The preview CLI validates the documented recovery-bundle envelope and points operators to this runbook. Media identification, engine catalog reconstruction, key-package decryption, and data restore are target behavior pending engine selection and qualification; the imperative steps below must not be read as claims that those paths are already implemented.

TapeScale recovery must remain possible after loss of the original TrueNAS host, TapeScale PostgreSQL database, and application catalog. Successful disaster recovery depends on three separately protected sets:

1. the required tapes and their complete dependency chain;
2. a recent authenticated recovery bundle with catalog/configuration metadata and compatible software information;
3. the independently protected key package and any original ZFS dataset keys required for raw encrypted streams.

Keeping all three on the source NAS defeats this design.

## Recovery bundle contract

A versioned bundle contains no plaintext usable keys. It is expected to include:

- bundle, manifest, engine-adapter, and on-tape format versions;
- TapeScale release/image digests and a dependency inventory;
- engine installation and compatible recovery-tool information;
- protected product configuration and catalog export references;
- source, snapshot, recovery-point, dependency, volume, and record maps;
- cartridge internal IDs, labels, barcodes where present, format/generation, sequence, and last confirmed location;
- manifest authentication information and encrypted metadata;
- required key identifiers and key-lineage references;
- required native dataset keys/features and destination constraints;
- checksums for every bundle artifact and creation/freshness evidence.

Bundles are created at least daily and after protected-job finalization, key changes, and relevant configuration changes. A closed export set should carry an updated recovery export where the selected format permits. A separately stored copy is mandatory.

## Before a disaster

- Export the bundle and key package to independent storage.
- Verify that the key package can be decrypted with the documented recovery method.
- Retain compatible `tapescale-recovery`, engine tools, database schema/migrations, codecs, dictionaries, and format documentation outside the original host.
- Print or independently retain the tape/dependency and bootstrap-location summary.
- Perform a quarterly clean-install exercise into isolated test destinations.
- Record exact TrueNAS, OpenZFS, engine, adapter, image, device, and format versions.

A green backup status does not prove these materials are usable. Only the clean-host exercise does.

## Clean replacement procedure

### 1. Establish an isolated recovery environment

Use a supported x86-64 Linux/TrueNAS environment with no production shares or automatic mountpoints. Record its host, kernel, OpenZFS, driver, HBA, drive, changer, firmware, and media details. Connect only the devices and destination pools needed for the exercise.

Do not load or write unknown media. Do not initialize an empty catalog over recovered evidence.

### 2. Install compatible software

Install the pinned TapeScale release, PostgreSQL 16, selected engine recovery tools, OpenZFS tools, and the exact codecs/dictionaries named by the bundle. Verify image and package digests against independently stored values.

The engineering-preview CLI is `tapescale-recovery`. Inspect its local `--help` for the implemented commands; absent commands are a release-blocking gap, not permission to improvise destructive engine operations.

### 3. Inspect the bundle without importing

Validate the outer bundle version, artifact checksums, authentication, and compatibility. Produce a report of:

- available recovery points and representations;
- full/incremental dependency chains;
- required tapes in order and last confirmed locations;
- drive read compatibility;
- required key identifiers;
- estimated read/staging capacity and destination constraints;
- missing, stale, or unsupported prerequisites.

Never display usable keys in the report or logs.

### 4. Recover keys separately

Obtain the key package through the independently controlled process. Decrypt it into the approved ephemeral secret facility, not the working directory or shell history. Match key identifiers and historical lineage to retained recovery points. Obtain any original raw-ZFS dataset keys separately.

A missing or wrong key is a clear blocked condition. It never downgrades encryption or verification.

### 5. Inventory and identify media read-only

Reconcile drive identity and generation before loading. Read media identity through the qualified engine path. Compare internal identity, format, sequence, and authenticated manifest information with the bundle. A printed label or barcode is not sufficient.

Unknown, foreign, mismatched, or unexpectedly writable media remains protected. Do not relabel or erase it during recovery.

### 6. Rebuild or import catalogs

Use only the selected engine's supported import/rebuild interface. Bootstrap from local volume/record metadata and the authenticated bundle; do not rely on a single trailing tape index. Populate TapeScale state through the documented recovery import, preserving stable identifiers and evidence.

Reconciliation differences remain visible and block destructive work. A reconstructed database is an aid to recovery, not retroactive proof that all tape writes completed.

### 7. Plan the restore

Select the recovery point and representation. Preview the complete chain, tapes, locations, drive compatibility, key requirements, verification history, read/staging estimate, destination mapping, and conflicts.

- File restore defaults to a new empty directory.
- Native restore maps dataset names and mountpoints below an isolated approved dataset and does not publish shares.
- File extraction from native streams first receives the complete required chain into approved temporary ZFS capacity.
- Zvols restore as block volumes; do not advertise file browsing.

### 8. Execute and validate

Load tapes only when requested and confirm exact IDs. File restores compare content with recorded SHA-256 digests and validate promised metadata. Native restores validate the entire stream/receive and declared dataset state. If a separate source-file index exists, record that additional comparison without confusing it with stream validation.

Unsupported ACL semantics, missing identities, corrupt records, authentication failures, and missing volumes are explicit failures or limitations. Preserve partial recoverable output and evidence; do not publish it as complete.

### 9. Review before publication

Keep recovered datasets isolated and shares disabled. Review content, properties, mountpoints, ACLs, identities, keys, and application consistency. Publication or cutover is a separate authorized operation. Destructive replacement of live data requires administrator reauthentication and a new affected-object preview.

### 10. Retain exercise evidence

Record the acceptance report fields in [QUALIFICATION.md](QUALIFICATION.md), restored manifest checksums, comparison results, elapsed active/operator time, problems, and reviewer. Return recalled archive media to its retained location through a custody task.

## Recovery modes

### Original database available

Restore PostgreSQL using the documented consistent backup, run versioned migrations, reconcile engine/device state, and inspect active operations before resuming. Restoring an older container image is not a database rollback.

### TapeScale database lost, engine catalog available

Import the authenticated recovery bundle, join engine records through stable IDs, reconcile all media and dependencies, and restore only after conflicts are resolved.

### Both catalogs lost

Use the recovery bundle, selected engine's supported tape scan/import path, independent key package, and required tapes. This is the decisive G2 path.

### Recovery bundle unavailable

Follow the selected engine's documented catalog reconstruction path and scan media read-only. Expect slower identification and potentially missing product policy/custody history. This path must be demonstrated before any release claims catalog-independent recovery; the initial preview has not demonstrated it.

## Failure handling

- Wrong tape: leave unchanged, retain the request, and explain the expected identities.
- Read/authentication/checksum failure: fail the affected scope, preserve evidence, and try an independently valid copy only through a new recorded attempt.
- Device reset/disconnect: mark position uncertain and reconcile before further I/O.
- Insufficient staging/destination capacity: stop before receive/extraction where it can be predicted.
- Unsupported format/version: use the maintained compatible reader; do not migrate the only copy in place.
- Missing key: block and identify the exact lineage; never expose ciphertext as a successful restore.

## Current limitations

The repository currently exposes a simulator adapter and has not selected or pinned Bacula 15 or another production engine. No clean-host tape restoration, physical multi-volume recovery, ACL round trip, or independent-key recovery has been evidenced. These are explicit G0/G1/G2 blockers.
