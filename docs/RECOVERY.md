# Independent TapeScale Native recovery

Status: the database-independent Native v1/v2 volume capture and full DAR restore path is implemented in `0.2.0-alpha.11`. It has not completed the physical G2 qualification matrix. A successful run is evidence for its exact host, drive, media, archive, metadata, and software revision; it is not a general production claim.

The current web console does not dispatch native restores. Use the separately installed `tapescale-recovery` executable on an isolated recovery host.

## What must survive a disaster

Every TapeScale Native cartridge contains its own archive/volume identity and authenticated recovery metadata. Native v2 members carry the source manifest, independently finalized pack descriptors/catalogs/slices, a chained volume index, and a trailer; the final member carries the archive-set seal. PostgreSQL and the original rolling spool are useful operational state but are not required by the recovery utility.

Protect these separately:

1. every required cartridge in the archive set;
2. the compatible TapeScale release or recovery binary and DAR 2.8.6 runtime;
3. this format/runbook documentation and verified software/image digests;
4. custody records that let an operator find the cartridges;
5. enough isolated disk capacity for the complete encoded archive and restored tree.

Native v1 and v2 are unencrypted, so there is no key package for either format. Captured pack data and restored files are plaintext and must receive source-equivalent protection.

## Recovery commands

Inspect the locally installed interface before use:

```sh
tapescale-recovery --help
tapescale-recovery runbook
```

The implemented native commands are:

```sh
tapescale-recovery capture-native-volume \
  --device /dev/nst0 \
  --archive-dir /recovery/archive

tapescale-recovery restore-native-archive \
  --archive-dir /recovery/archive \
  --destination /recovery/restored \
  --dar /usr/local/bin/dar \
  --threads 4
```

`--archive-dir` must already exist and be a real directory. Reuse that same directory while capturing every volume from one archive. `--destination` must already exist, be real, and be empty.

## Clean-host procedure

### 1. Establish an isolated environment

Use a supported Linux/TrueNAS host with no production shares or automatic publication. Record the kernel, HBA, tape driver, drive model/serial/WWN, firmware, media generation, TapeScale version, DAR version, and recovery-host identity.

Attach only the required tape device and destination storage. Do not initialize PostgreSQL, prepare media, relabel tapes, or enable any write workflow.

### 2. Verify the software

Obtain `tapescale-recovery`, DAR 2.8.6, libthreadar, `mt-st`, and their dependencies from independently retained artifacts. Verify image/package/source digests before loading media.

The release container includes the recovery binary and DAR runtime. If using another host installation, preserve the same executable versions and library feature set.

### 3. Provision capture and restore capacity

Create two isolated real directories:

```sh
mkdir -p /recovery/archive /recovery/restored
chmod 0700 /recovery/archive /recovery/restored
```

The capture filesystem must hold the complete encoded archive plus metadata and receipts. Unlike the backup writer's bounded 1 TiB rolling spool, clean-host capture retains all packs required for restore. The destination must hold the restored logical tree. Account for sparse-file expansion and filesystem metadata.

### 4. Capture every volume

For each cartridge in the archive:

1. load it in the recorded compatible drive;
2. wait for the kernel to report ready;
3. run `capture-native-volume` against the exact non-rewinding device;
4. retain the JSON result and command log;
5. unload only after the command completes and its final rewind succeeds.

The capture command:

- rejects device paths outside exact `/dev/nstN` syntax;
- enables variable-block mode and rewinds through fixed `mt-st` arguments;
- auto-detects Native v1 or v2 and decodes its typed identity records;
- streams v1 catalog/slices or v2 pack descriptors/catalogs/slices to disk rather than buffering them in memory;
- reads and decodes the index and trailer;
- checks every identity, ordinal, offset, length, and SHA-256 binding;
- rejects a duplicate or inconsistent member already present in the directory;
- commits canonical files and a volume receipt only after validation;
- rewinds before returning.

Volume loading order is not security authority; receipts are sorted and checked by their on-tape ordinals. Printed labels alone are not trusted.

### 5. Restore the complete archive

After every required volume has been captured, run `restore-native-archive`. It refuses to proceed unless:

- the captured set contains every member required by the v1 header or v2 final archive-set seal;
- volume ordinals are complete and contiguous;
- cartridge and archive-volume UUIDs are unique;
- every member has the same archive UUID, representation, compression, and manifest commitment;
- v1 global slice ordinals or v2 pack ordinals, volume predecessor links, catalog roots, and ordered commitments are complete and contiguous;
- every captured artifact still matches its retained length and SHA-256;
- native header/index/trailer records still decode and bind consistently;
- the destination is an existing empty real directory;
- the DAR executable is the exact expected basename and an absolute executable path.

DAR tests the reconstructed v1 archive or each ordered v2 pack before extraction. After extraction, TapeScale independently inventories the destination and compares it to the retained source manifest. A mismatch returns failure even when DAR itself exited successfully.

### 6. Review before publication

Keep the destination isolated. Review:

- the final JSON `valid` value and manifest comparison;
- missing, changed, or extra paths;
- covered ownership, mode, timestamps, links, and content hashes;
- ACL/xattr behavior on the specific source/destination filesystem pair;
- application consistency and any metadata outside the manifest's declared coverage.

Publication, share creation, or replacement of live data is a separate authorized operation. The recovery utility never performs cutover.

### 7. Retain evidence

Record the fields required by [QUALIFICATION.md](QUALIFICATION.md), including archive UUID, volume/archive-volume/cartridge UUIDs, capture receipts, software digests, restored-manifest comparison, duration, errors, and reviewer. Protect logs against unintended filename disclosure.

## Failure handling

- **Wrong or foreign archive member:** leave unchanged and load the expected set member. Do not rename receipts or edit metadata to force acceptance.
- **Read, length, checksum, index, or trailer failure:** stop, preserve the failed evidence, clean the uncommitted temporary capture, and try an independently valid copy only as a new recorded attempt.
- **Device reset/disconnect or failed final rewind:** treat tape position as uncertain and reconcile before another read.
- **Duplicate ordinal/UUID:** investigate inventory and media substitution; do not choose one arbitrarily.
- **Missing volume or slice:** restore remains blocked. Partial extraction is not a complete recovery claim.
- **Insufficient capture/destination capacity:** add approved storage; do not redirect output into the source or a live share.
- **DAR test/extract failure:** retain the capture set read-only for diagnosis. Never present partial output as recovered.
- **Manifest mismatch:** fail the recovery scope, even when every native tape hash and DAR test passed.
- **Unsupported format/version/representation:** use a maintained compatible reader; never migrate the only copy in place.
- **Interrupted writer or partial member:** the alpha writer does not resume or append mid-volume. Quarantine the touched cartridge and create a new archive set; clean-host recovery never treats the partial set as complete.

## Recovery bundle compatibility

`inspect-bundle` remains available for the broader portable recovery-bundle envelope:

```sh
tapescale-recovery inspect-bundle /path/to/bundle-manifest.json
```

Bundle inspection validates its outer manifest and references; it does not read tape or constitute a restore test. TapeScale Native full recovery depends primarily on the self-describing cartridges and compatible software. Custody and policy history may be unavailable when PostgreSQL and external bundle records are lost.

## Current limitations

- full native DAR archive restoration only; no selected-path recovery command;
- manual standalone-drive cartridge changes only;
- no encrypted archive or key recovery;
- no native ZFS-send recovery;
- no web-console native restore execution;
- no automatic catalog import back into a new TapeScale database;
- no writer restart/resume within a partially written v2 volume;
- no general qualification yet for ACLs, xattrs, sparse files, hard links, unusual names, all compression modes, all LTO generations, multi-volume boundaries, wrong-media handling, or controlled interruption.

Older Bacula-formatted experimental media is incompatible and cannot be recovered by the native utility. This deployment declared zero valid Bacula tapes; Bacula is therefore not packaged in the 0.2 runtime.
