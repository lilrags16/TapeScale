use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, FileTypeExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tapescale::{
    data_path::{build_manifest_unbounded, compare_extracted_manifests, load_manifest},
    native_archive::{open_tape_reader, read_tape_file, sha256_bytes, DAR_CODEC_VERSION},
    native_format::{
        NativeArchiveRepresentation, NativeSliceDescriptor, NativeVolumeHeader, NativeVolumeIndex,
        NativeVolumeTrailer,
    },
};
use uuid::Uuid;

const MAX_BUNDLE_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
const MAX_NATIVE_METADATA_BYTES: u64 = 16 * 1024 * 1024;
const MAX_CAPTURE_FILE_BYTES: u64 = 2 * 1024 * 1024 * 1024 * 1024;
const COPY_BUFFER_BYTES: usize = 1024 * 1024;
const CAPTURE_FORMAT: &str = "tapescale-native-volume-capture";
const CAPTURE_FORMAT_VERSION: u32 = 1;

#[derive(Debug, Parser)]
#[command(
    name = "tapescale-recovery",
    version,
    about = "Independent TapeScale recovery and evidence utility",
    long_about = "Independently inspects recovery bundles, captures and validates TapeScale Native tape volumes, and restores a complete captured DAR archive without the TapeScale database."
)]
struct Arguments {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Inspect and validate the non-secret manifest of a portable recovery bundle.
    InspectBundle { path: PathBuf },
    /// Print the independent clean-host recovery sequence.
    Runbook,
    /// Print the honest hardware/format qualification boundary.
    Qualification,
    /// Rewind, stream, and independently verify one TapeScale Native volume.
    CaptureNativeVolume {
        /// Exact non-rewinding Linux tape device (for example /dev/nst0).
        #[arg(long)]
        device: PathBuf,
        /// Existing real directory that will hold all volumes of one archive.
        #[arg(long)]
        archive_dir: PathBuf,
        /// Exact mt-st executable used only for rewind.
        #[arg(long, default_value = "/usr/bin/mt-st")]
        mt: PathBuf,
    },
    /// Validate a complete captured native archive and extract it with DAR.
    RestoreNativeArchive {
        /// Existing directory populated by CaptureNativeVolume.
        #[arg(long)]
        archive_dir: PathBuf,
        /// Existing, empty, real directory that will receive restored files.
        #[arg(long)]
        destination: PathBuf,
        /// Exact DAR executable.
        #[arg(long, default_value = "/usr/local/bin/dar")]
        dar: PathBuf,
        /// DAR decompression worker count.
        #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u16).range(1..=64))]
        threads: u16,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct CapturedArtifact {
    file: String,
    bytes: u64,
    sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct CapturedSlice {
    descriptor: NativeSliceDescriptor,
    artifact: CapturedArtifact,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct NativeCaptureReceipt {
    format: String,
    format_version: u32,
    captured_at: DateTime<Utc>,
    header: NativeVolumeHeader,
    index: NativeVolumeIndex,
    trailer: NativeVolumeTrailer,
    header_record: CapturedArtifact,
    catalog: CapturedArtifact,
    source_manifest: CapturedArtifact,
    slices: Vec<CapturedSlice>,
    index_record: CapturedArtifact,
    trailer_record: CapturedArtifact,
}

#[derive(Debug, Deserialize, Serialize)]
struct RecoveryBundleManifest {
    format: String,
    format_version: u32,
    created_at: String,
    installation_id: String,
    catalog_export: Artifact,
    media_map: Artifact,
    key_package_reference: String,
    required_software: Vec<SoftwareRequirement>,
}

#[derive(Debug, Deserialize, Serialize)]
struct Artifact {
    file: String,
    sha256: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct SoftwareRequirement {
    component: String,
    version: String,
}

fn main() -> Result<()> {
    match Arguments::parse().command {
        Command::InspectBundle { path } => inspect_bundle(&path),
        Command::Runbook => {
            println!(
                "1. Start from a clean qualified Linux host.\n\
                 2. Verify this CLI and engine packages against independently stored digests.\n\
                 3. Inspect the bundle; keep the key package separate until requested.\n\
                 4. For each ordered TapeScale Native volume, run capture-native-volume; the CLI reads its identity, payload hashes, index, and trailer directly from tape.\n\
                 5. After every required volume is present, run restore-native-archive into an empty isolated destination.\n\
                 6. Review the retained source manifest and restored tree before any explicit cutover.\n\n\
                 See docs/RECOVERY.md. This path is implemented but still requires qualification on the exact drive, media generation, metadata, and failure matrix."
            );
            Ok(())
        }
        Command::Qualification => {
            println!(
                "{}",
                serde_json::to_string_pretty(&tapescale::qualification::report())?
            );
            Ok(())
        }
        Command::CaptureNativeVolume {
            device,
            archive_dir,
            mt,
        } => capture_native_volume(&device, &archive_dir, &mt),
        Command::RestoreNativeArchive {
            archive_dir,
            destination,
            dar,
            threads,
        } => restore_native_archive(&archive_dir, &destination, &dar, threads),
    }
}

fn inspect_bundle(path: &PathBuf) -> Result<()> {
    let metadata = fs::metadata(path).with_context(|| format!("cannot stat {}", path.display()))?;
    anyhow::ensure!(metadata.is_file(), "bundle manifest must be a regular file");
    anyhow::ensure!(
        metadata.len() <= MAX_BUNDLE_MANIFEST_BYTES,
        "bundle manifest exceeds the 16 MiB safety limit"
    );
    let bytes = fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    let manifest: RecoveryBundleManifest =
        serde_json::from_slice(&bytes).context("invalid recovery bundle manifest")?;
    anyhow::ensure!(
        manifest.format == "tapescale-recovery-bundle",
        "unknown bundle format"
    );
    anyhow::ensure!(
        manifest.format_version == 1,
        "unsupported bundle format version"
    );
    validate_artifact(&manifest.catalog_export)?;
    validate_artifact(&manifest.media_map)?;
    anyhow::ensure!(
        !manifest.key_package_reference.trim().is_empty(),
        "key package reference is missing"
    );

    let digest = Sha256::digest(&bytes);
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "valid": true,
            "manifest_sha256": format!("{digest:x}"),
            "format_version": manifest.format_version,
            "created_at": manifest.created_at,
            "installation_id": manifest.installation_id,
            "catalog_export": manifest.catalog_export,
            "media_map": manifest.media_map,
            "key_package_reference": manifest.key_package_reference,
            "required_software": manifest.required_software,
            "notice": "Manifest inspection does not constitute a physical restore test."
        }))?
    );
    Ok(())
}

fn validate_artifact(artifact: &Artifact) -> Result<()> {
    anyhow::ensure!(!artifact.file.is_empty(), "artifact filename is empty");
    anyhow::ensure!(
        !artifact.file.starts_with('/') && !artifact.file.split('/').any(|part| part == ".."),
        "artifact filename escapes the recovery bundle"
    );
    anyhow::ensure!(
        artifact.sha256.len() == 64 && artifact.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "artifact SHA-256 must contain exactly 64 hexadecimal characters"
    );
    Ok(())
}

#[derive(Debug)]
struct TemporaryArtifact {
    path: PathBuf,
    artifact: CapturedArtifact,
}

#[derive(Debug)]
struct PreparedCapture {
    receipt: NativeCaptureReceipt,
    files: Vec<TemporaryArtifact>,
}

fn capture_native_volume(device: &Path, archive_dir: &Path, mt: &Path) -> Result<()> {
    validate_real_directory(archive_dir, "archive capture directory")?;
    validate_tape_device(device)?;
    validate_executable(mt, "mt-st", "mt-st")?;
    set_variable_block_mode(mt, device)
        .context("could not enable variable-block native tape reads")?;
    rewind_tape(mt, device).context("initial native tape rewind failed")?;

    let temporary_directory = archive_dir.join(format!(".capture-{}", Uuid::now_v7()));
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&temporary_directory)
        .with_context(|| {
            format!(
                "cannot create capture workspace {}",
                temporary_directory.display()
            )
        })?;
    let capture = capture_native_volume_inner(device, &temporary_directory);
    let final_rewind = rewind_tape(mt, device).context("post-capture tape rewind failed");
    let prepared = match (capture, final_rewind) {
        (Ok(prepared), Ok(())) => prepared,
        (Err(error), _) => {
            let _ = fs::remove_dir_all(&temporary_directory);
            return Err(error);
        }
        (Ok(_), Err(error)) => {
            let _ = fs::remove_dir_all(&temporary_directory);
            return Err(error);
        }
    };

    if let Err(error) = validate_new_capture(archive_dir, &prepared.receipt) {
        let _ = fs::remove_dir_all(&temporary_directory);
        return Err(error);
    }
    let archive_id = prepared.receipt.header.archive_id;
    let volume_ordinal = prepared.receipt.header.volume_ordinal;
    let volume_count = prepared.receipt.header.volume_count;
    let slice_count = prepared.receipt.header.slice_count;
    commit_capture(archive_dir, &temporary_directory, prepared)?;
    let captured_volumes = load_capture_receipts(archive_dir)?.len();
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "valid": true,
            "archive_id": archive_id,
            "captured_volume": volume_ordinal,
            "required_volumes": volume_count,
            "captured_volumes": captured_volumes,
            "captured_slices": slice_count,
            "notice": "The captured volume passed its TapeScale header, payload, index, and trailer checks. Restore requires every volume in the declared set."
        }))?
    );
    Ok(())
}

fn capture_native_volume_inner(device: &Path, temporary: &Path) -> Result<PreparedCapture> {
    let mut tape = open_tape_reader(device).context("cannot open native tape for readback")?;
    let header_bytes = read_tape_file(&mut tape, MAX_NATIVE_METADATA_BYTES)
        .context("cannot read native volume header from tape file 0")?;
    let header = NativeVolumeHeader::decode(&header_bytes)
        .context("tape file 0 is not a valid TapeScale Native volume header")?;
    anyhow::ensure!(
        header.representation == NativeArchiveRepresentation::FileDar,
        "the recovery CLI currently accepts only TapeScale Native DAR archives"
    );
    let volume = header.volume_ordinal;
    let mut files = Vec::with_capacity(header.slice_count as usize + 5);

    files.push(temporary_bytes_artifact(
        temporary,
        "header.tmp",
        native_header_filename(volume),
        &header_bytes,
    )?);
    let catalog = capture_tape_file(
        &mut tape,
        &temporary.join("catalog.tmp"),
        "catalog.1.dar".to_owned(),
    )
    .context("cannot capture the isolated DAR catalog from tape file 1")?;
    let source_manifest = capture_tape_file(
        &mut tape,
        &temporary.join("source-manifest.tmp"),
        "source-manifest.json".to_owned(),
    )
    .context("cannot capture the source manifest from tape file 2")?;

    let mut temporary_slices = Vec::with_capacity(header.slice_count as usize);
    for local_index in 0..header.slice_count {
        let path = temporary.join(format!("slice-{local_index:06}.tmp"));
        let artifact = capture_tape_file(&mut tape, &path, String::new()).with_context(|| {
            format!(
                "cannot capture native archive slice from tape file {}",
                local_index + 3
            )
        })?;
        temporary_slices.push(TemporaryArtifact { path, artifact });
    }

    let index_bytes = read_tape_file(&mut tape, MAX_NATIVE_METADATA_BYTES)
        .context("cannot read the native volume index")?;
    let index =
        NativeVolumeIndex::decode(&index_bytes).context("the native volume index is invalid")?;
    let trailer_bytes = read_tape_file(&mut tape, MAX_NATIVE_METADATA_BYTES)
        .context("cannot read the native volume trailer")?;
    let trailer = NativeVolumeTrailer::decode(&trailer_bytes)
        .context("the native volume trailer is invalid")?;
    drop(tape);

    anyhow::ensure!(
        temporary_slices.len() == index.slices.len(),
        "header and index disagree on the number of archive slices"
    );
    let mut slices = Vec::with_capacity(index.slices.len());
    for (local_index, (mut temporary_slice, descriptor)) in temporary_slices
        .into_iter()
        .zip(index.slices.iter())
        .enumerate()
    {
        temporary_slice.artifact.file = dar_slice_filename(descriptor.slice_ordinal);
        anyhow::ensure!(
            descriptor.tape_file_number == u32::try_from(local_index + 3)?,
            "slice {} has the wrong physical tape-file number",
            descriptor.slice_ordinal
        );
        anyhow::ensure!(
            temporary_slice.artifact.bytes == descriptor.encoded_bytes
                && temporary_slice.artifact.sha256 == descriptor.sha256,
            "slice {} does not match its volume index",
            descriptor.slice_ordinal
        );
        slices.push(CapturedSlice {
            descriptor: descriptor.clone(),
            artifact: temporary_slice.artifact.clone(),
        });
        files.push(temporary_slice);
    }

    files.push(TemporaryArtifact {
        path: temporary.join("catalog.tmp"),
        artifact: catalog.clone(),
    });
    files.push(TemporaryArtifact {
        path: temporary.join("source-manifest.tmp"),
        artifact: source_manifest.clone(),
    });
    files.push(temporary_bytes_artifact(
        temporary,
        "index.tmp",
        native_index_filename(volume),
        &index_bytes,
    )?);
    files.push(temporary_bytes_artifact(
        temporary,
        "trailer.tmp",
        native_trailer_filename(volume),
        &trailer_bytes,
    )?);

    let receipt = NativeCaptureReceipt {
        format: CAPTURE_FORMAT.to_owned(),
        format_version: CAPTURE_FORMAT_VERSION,
        captured_at: Utc::now(),
        header,
        index,
        trailer,
        header_record: files[0].artifact.clone(),
        catalog,
        source_manifest,
        slices,
        index_record: files[files.len() - 2].artifact.clone(),
        trailer_record: files[files.len() - 1].artifact.clone(),
    };
    validate_volume_contract(&receipt)?;
    Ok(PreparedCapture { receipt, files })
}

fn temporary_bytes_artifact(
    directory: &Path,
    temporary_name: &str,
    final_name: String,
    bytes: &[u8],
) -> Result<TemporaryArtifact> {
    let path = directory.join(temporary_name);
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&path)
        .with_context(|| format!("cannot create {}", path.display()))?;
    output.write_all(bytes)?;
    output.sync_all()?;
    Ok(TemporaryArtifact {
        path,
        artifact: CapturedArtifact {
            file: final_name,
            bytes: u64::try_from(bytes.len())?,
            sha256: sha256_bytes(bytes),
        },
    })
}

fn capture_tape_file(tape: &mut File, path: &Path, final_name: String) -> Result<CapturedArtifact> {
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("cannot create {}", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    let mut total = 0_u64;
    loop {
        let read = match tape.read(&mut buffer) {
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        };
        if read == 0 {
            break;
        }
        total = total
            .checked_add(u64::try_from(read)?)
            .context("captured tape-file byte count overflowed")?;
        anyhow::ensure!(
            total <= MAX_CAPTURE_FILE_BYTES,
            "a native tape file exceeded the 2 TiB recovery safety bound"
        );
        output.write_all(&buffer[..read])?;
        digest.update(&buffer[..read]);
    }
    anyhow::ensure!(total > 0, "native tape file was unexpectedly empty");
    output.sync_all()?;
    Ok(CapturedArtifact {
        file: final_name,
        bytes: total,
        sha256: format!("{:x}", digest.finalize()),
    })
}

fn validate_volume_contract(receipt: &NativeCaptureReceipt) -> Result<()> {
    anyhow::ensure!(
        receipt.format == CAPTURE_FORMAT && receipt.format_version == CAPTURE_FORMAT_VERSION,
        "unsupported native volume capture receipt"
    );
    let header = &receipt.header;
    let index = &receipt.index;
    anyhow::ensure!(
        header.format_version == index.format_version
            && header.cartridge_id == index.cartridge_id
            && header.archive_id == index.archive_id
            && header.archive_volume_id == index.archive_volume_id
            && header.volume_ordinal == index.volume_ordinal
            && header.volume_count == index.volume_count
            && usize::try_from(header.slice_count)? == index.slices.len()
            && header.source_manifest_sha256 == index.source_manifest_sha256
            && header.archive_catalog_sha256 == index.archive_catalog_sha256,
        "native volume header does not match its index"
    );
    receipt
        .trailer
        .verify_index(index)
        .context("native volume trailer does not commit to its index")?;
    anyhow::ensure!(
        receipt.catalog.sha256 == header.archive_catalog_sha256,
        "captured catalog does not match the native header"
    );
    anyhow::ensure!(
        receipt.source_manifest.sha256 == header.source_manifest_sha256,
        "captured source manifest does not match the native header"
    );
    anyhow::ensure!(
        receipt.slices.len() == index.slices.len(),
        "capture receipt has the wrong slice count"
    );
    for (local_index, (captured, indexed)) in
        receipt.slices.iter().zip(index.slices.iter()).enumerate()
    {
        anyhow::ensure!(
            captured.descriptor == *indexed
                && captured.artifact.file == dar_slice_filename(indexed.slice_ordinal)
                && captured.artifact.bytes == indexed.encoded_bytes
                && captured.artifact.sha256 == indexed.sha256
                && indexed.tape_file_number == u32::try_from(local_index + 3)?,
            "captured slice {} does not match its index",
            indexed.slice_ordinal
        );
    }
    for artifact in [
        &receipt.header_record,
        &receipt.catalog,
        &receipt.source_manifest,
        &receipt.index_record,
        &receipt.trailer_record,
    ] {
        validate_captured_artifact_name(artifact)?;
    }
    anyhow::ensure!(
        receipt.header_record.file == native_header_filename(header.volume_ordinal)
            && receipt.index_record.file == native_index_filename(header.volume_ordinal)
            && receipt.trailer_record.file == native_trailer_filename(header.volume_ordinal),
        "native metadata record filenames do not match the volume ordinal"
    );
    Ok(())
}

fn validate_capture_set(receipts: &[NativeCaptureReceipt], complete: bool) -> Result<()> {
    let first = receipts
        .first()
        .context("no captured native volumes were found")?;
    let expected_count = usize::try_from(first.header.volume_count)?;
    anyhow::ensure!(expected_count > 0, "native archive declares zero volumes");
    if complete {
        anyhow::ensure!(
            receipts.len() == expected_count,
            "archive {} requires {} volumes, but {} were captured",
            first.header.archive_id,
            expected_count,
            receipts.len()
        );
    } else {
        anyhow::ensure!(
            receipts.len() <= expected_count,
            "more capture receipts exist than the archive declares"
        );
    }

    let mut ordered = receipts.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|receipt| receipt.header.volume_ordinal);
    let mut volume_ordinals = std::collections::HashSet::new();
    let mut archive_volume_ids = std::collections::HashSet::new();
    let mut cartridge_ids = std::collections::HashSet::new();
    let mut slice_ordinals = std::collections::HashSet::new();
    let mut expected_slice = 1_u32;
    let mut expected_offset = 0_u64;
    for (position, receipt) in ordered.into_iter().enumerate() {
        validate_volume_contract(receipt)?;
        anyhow::ensure!(
            receipt.header.archive_id == first.header.archive_id
                && receipt.header.volume_count == first.header.volume_count
                && receipt.header.representation == first.header.representation
                && receipt.header.compression == first.header.compression
                && receipt.header.source_manifest_sha256 == first.header.source_manifest_sha256
                && receipt.header.archive_catalog_sha256 == first.header.archive_catalog_sha256,
            "captured volumes do not belong to one consistent archive"
        );
        anyhow::ensure!(
            volume_ordinals.insert(receipt.header.volume_ordinal),
            "duplicate native archive volume {}",
            receipt.header.volume_ordinal
        );
        anyhow::ensure!(
            archive_volume_ids.insert(receipt.header.archive_volume_id),
            "duplicate native archive-volume identity"
        );
        anyhow::ensure!(
            cartridge_ids.insert(receipt.header.cartridge_id),
            "one physical cartridge appears more than once in the archive set"
        );
        if complete {
            anyhow::ensure!(
                receipt.header.volume_ordinal == u32::try_from(position + 1)?,
                "captured native volume ordinals are not contiguous"
            );
            for slice in &receipt.index.slices {
                anyhow::ensure!(
                    slice_ordinals.insert(slice.slice_ordinal),
                    "duplicate archive slice {}",
                    slice.slice_ordinal
                );
                anyhow::ensure!(
                    slice.slice_ordinal == expected_slice
                        && slice.archive_offset_bytes == expected_offset,
                    "native archive slices are missing, reordered, or have non-contiguous offsets"
                );
                expected_slice = expected_slice
                    .checked_add(1)
                    .context("native archive slice count overflowed")?;
                expected_offset = expected_offset
                    .checked_add(slice.encoded_bytes)
                    .context("native archive byte offset overflowed")?;
            }
        } else {
            for slice in &receipt.index.slices {
                anyhow::ensure!(
                    slice_ordinals.insert(slice.slice_ordinal),
                    "duplicate archive slice {}",
                    slice.slice_ordinal
                );
            }
        }
    }
    Ok(())
}

fn validate_new_capture(archive_dir: &Path, receipt: &NativeCaptureReceipt) -> Result<()> {
    let mut existing = load_capture_receipts(archive_dir)?;
    anyhow::ensure!(
        !existing.iter().any(|value| {
            value.header.volume_ordinal == receipt.header.volume_ordinal
                || value.header.archive_volume_id == receipt.header.archive_volume_id
        }),
        "this native archive volume has already been captured"
    );
    existing.push(receipt.clone());
    validate_capture_set(&existing, false)
}

fn commit_capture(
    archive_dir: &Path,
    temporary_directory: &Path,
    prepared: PreparedCapture,
) -> Result<()> {
    for temporary in &prepared.files {
        validate_captured_artifact_name(&temporary.artifact)?;
        let destination = archive_dir.join(&temporary.artifact.file);
        if let Ok(metadata) = fs::symlink_metadata(&destination) {
            anyhow::ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "capture target {} is not a regular file",
                destination.display()
            );
            verify_artifact(archive_dir, &temporary.artifact)?;
        }
    }
    let receipt_name = native_receipt_filename(prepared.receipt.header.volume_ordinal);
    anyhow::ensure!(
        fs::symlink_metadata(archive_dir.join(&receipt_name)).is_err(),
        "native volume receipt already exists"
    );

    for temporary in &prepared.files {
        let destination = archive_dir.join(&temporary.artifact.file);
        match fs::symlink_metadata(&destination) {
            Ok(_) => {
                verify_artifact(archive_dir, &temporary.artifact)?;
                fs::remove_file(&temporary.path)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::hard_link(&temporary.path, &destination).with_context(|| {
                    format!(
                        "cannot atomically commit captured artifact {}",
                        temporary.artifact.file
                    )
                })?;
                fs::remove_file(&temporary.path)?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    let receipt_path = temporary_directory.join("receipt.tmp");
    let receipt_bytes = serde_json::to_vec_pretty(&prepared.receipt)?;
    let mut receipt_file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&receipt_path)?;
    receipt_file.write_all(&receipt_bytes)?;
    receipt_file.write_all(b"\n")?;
    receipt_file.sync_all()?;
    fs::hard_link(&receipt_path, archive_dir.join(receipt_name))?;
    fs::remove_file(&receipt_path)?;
    File::open(archive_dir)?.sync_all()?;
    fs::remove_dir(temporary_directory)?;
    Ok(())
}

fn restore_native_archive(
    archive_dir: &Path,
    destination: &Path,
    dar: &Path,
    threads: u16,
) -> Result<()> {
    validate_real_directory(archive_dir, "archive capture directory")?;
    validate_empty_real_directory(destination)?;
    validate_executable(dar, "DAR", "dar")?;
    validate_dar_version(dar)?;
    anyhow::ensure!(
        (1..=64).contains(&threads),
        "DAR thread count must be 1..=64"
    );
    let receipts = load_capture_receipts(archive_dir)?;
    validate_capture_set(&receipts, true)?;
    verify_capture_files(archive_dir, &receipts)?;
    let first = receipts.first().context("native archive has no volumes")?;

    run_dar(
        dar,
        &dar_test_args(archive_dir, threads),
        "archive validation",
    )?;
    run_dar(
        dar,
        &dar_restore_args(archive_dir, destination, threads),
        "archive extraction",
    )?;

    let expected = load_manifest(&archive_dir.join(&first.source_manifest.file))
        .context("captured source manifest is not a valid TapeScale manifest")?;
    let actual = build_manifest_unbounded(destination)
        .context("could not inventory the independently restored destination")?;
    let comparison = compare_extracted_manifests(&expected, &actual);
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "valid": comparison.matched,
            "archive_id": first.header.archive_id,
            "volume_count": first.header.volume_count,
            "dar_version": DAR_CODEC_VERSION,
            "destination": destination,
            "destination_root_metadata": "caller_owned_container_not_compared",
            "manifest_comparison": comparison
        }))?
    );
    anyhow::ensure!(
        comparison.matched,
        "DAR extraction completed, but the restored tree did not match the retained source manifest"
    );
    Ok(())
}

fn verify_capture_files(archive_dir: &Path, receipts: &[NativeCaptureReceipt]) -> Result<()> {
    for receipt in receipts {
        for artifact in [
            &receipt.header_record,
            &receipt.catalog,
            &receipt.source_manifest,
            &receipt.index_record,
            &receipt.trailer_record,
        ] {
            verify_artifact(archive_dir, artifact)?;
        }
        for slice in &receipt.slices {
            verify_artifact(archive_dir, &slice.artifact)?;
        }

        let header_bytes = read_bounded_regular_file(
            &archive_dir.join(&receipt.header_record.file),
            MAX_NATIVE_METADATA_BYTES,
        )?;
        let index_bytes = read_bounded_regular_file(
            &archive_dir.join(&receipt.index_record.file),
            MAX_NATIVE_METADATA_BYTES,
        )?;
        let trailer_bytes = read_bounded_regular_file(
            &archive_dir.join(&receipt.trailer_record.file),
            MAX_NATIVE_METADATA_BYTES,
        )?;
        anyhow::ensure!(
            NativeVolumeHeader::decode(&header_bytes)? == receipt.header
                && NativeVolumeIndex::decode(&index_bytes)? == receipt.index
                && NativeVolumeTrailer::decode(&trailer_bytes)? == receipt.trailer,
            "captured native metadata records do not match their receipt"
        );
    }
    Ok(())
}

fn load_capture_receipts(archive_dir: &Path) -> Result<Vec<NativeCaptureReceipt>> {
    let mut receipts = Vec::new();
    for entry in fs::read_dir(archive_dir)? {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if !name.starts_with("volume-") || !name.ends_with(".capture.json") {
            continue;
        }
        let bytes = read_bounded_regular_file(&path, MAX_NATIVE_METADATA_BYTES)?;
        let receipt: NativeCaptureReceipt = serde_json::from_slice(&bytes)
            .with_context(|| format!("invalid native capture receipt {}", path.display()))?;
        validate_volume_contract(&receipt)?;
        anyhow::ensure!(
            name == native_receipt_filename(receipt.header.volume_ordinal),
            "native receipt filename does not match its volume ordinal"
        );
        receipts.push(receipt);
    }
    receipts.sort_by_key(|receipt| receipt.header.volume_ordinal);
    Ok(receipts)
}

fn verify_artifact(root: &Path, artifact: &CapturedArtifact) -> Result<()> {
    validate_captured_artifact_name(artifact)?;
    let path = root.join(&artifact.file);
    let metadata = fs::symlink_metadata(&path)
        .with_context(|| format!("captured artifact {} is missing", artifact.file))?;
    anyhow::ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "captured artifact {} is not a regular file",
        artifact.file
    );
    anyhow::ensure!(
        metadata.len() == artifact.bytes,
        "captured artifact {} has the wrong length",
        artifact.file
    );
    let actual = sha256_file(&path)?;
    anyhow::ensure!(
        actual == artifact.sha256,
        "captured artifact {} failed SHA-256 verification",
        artifact.file
    );
    Ok(())
}

fn validate_captured_artifact_name(artifact: &CapturedArtifact) -> Result<()> {
    let path = Path::new(&artifact.file);
    anyhow::ensure!(
        !artifact.file.is_empty()
            && artifact.file.len() <= 128
            && !artifact.file.bytes().any(|byte| byte.is_ascii_control())
            && path.components().count() == 1
            && matches!(
                path.components().next(),
                Some(std::path::Component::Normal(_))
            ),
        "captured artifact filename is unsafe"
    );
    anyhow::ensure!(
        artifact.bytes > 0,
        "captured artifact {} is empty",
        artifact.file
    );
    anyhow::ensure!(
        artifact.sha256.len() == 64
            && artifact
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "captured artifact {} has a non-canonical SHA-256",
        artifact.file
    );
    Ok(())
}

fn read_bounded_regular_file(path: &Path, maximum: u64) -> Result<Vec<u8>> {
    let metadata =
        fs::symlink_metadata(path).with_context(|| format!("cannot inspect {}", path.display()))?;
    anyhow::ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "{} must be a regular file",
        path.display()
    );
    anyhow::ensure!(
        metadata.len() <= maximum,
        "{} exceeds its {} byte safety bound",
        path.display(),
        maximum
    );
    fs::read(path).with_context(|| format!("cannot read {}", path.display()))
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn validate_real_directory(path: &Path, description: &str) -> Result<()> {
    anyhow::ensure!(path.is_absolute(), "{description} must be absolute");
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("cannot inspect {description} {}", path.display()))?;
    anyhow::ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "{description} must be an existing real directory"
    );
    Ok(())
}

fn validate_empty_real_directory(path: &Path) -> Result<()> {
    validate_real_directory(path, "restore destination")?;
    anyhow::ensure!(
        fs::read_dir(path)?.next().is_none(),
        "restore destination must be empty"
    );
    Ok(())
}

fn validate_tape_device(path: &Path) -> Result<()> {
    let text = path
        .to_str()
        .context("tape device path must be valid UTF-8")?;
    anyhow::ensure!(
        text.starts_with("/dev/nst")
            && text.len() > 8
            && text[8..].bytes().all(|byte| byte.is_ascii_digit()),
        "native recovery requires an exact /dev/nstN device"
    );
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("cannot inspect tape device {}", path.display()))?;
    anyhow::ensure!(
        metadata.file_type().is_char_device() && !metadata.file_type().is_symlink(),
        "native recovery device must be a real character device"
    );
    Ok(())
}

fn validate_executable(path: &Path, description: &str, expected_name: &str) -> Result<()> {
    anyhow::ensure!(path.is_absolute(), "{description} path must be absolute");
    anyhow::ensure!(
        path.file_name().and_then(|value| value.to_str()) == Some(expected_name),
        "{description} executable must be named {expected_name}"
    );
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("cannot inspect {description} at {}", path.display()))?;
    anyhow::ensure!(
        metadata.is_file()
            && !metadata.file_type().is_symlink()
            && metadata.permissions().mode() & 0o111 != 0,
        "{description} must be a real executable file"
    );
    Ok(())
}

fn rewind_tape(mt: &Path, device: &Path) -> Result<()> {
    let output = ProcessCommand::new(mt)
        .arg("-f")
        .arg(device)
        .arg("rewind")
        .output()
        .context("could not execute mt-st")?;
    anyhow::ensure!(
        output.status.success(),
        "mt-st rewind failed: {}",
        bounded_diagnostic(&output.stderr)
    );
    Ok(())
}

fn set_variable_block_mode(mt: &Path, device: &Path) -> Result<()> {
    let output = ProcessCommand::new(mt)
        .arg("-f")
        .arg(device)
        .arg("setblk")
        .arg("0")
        .output()
        .context("could not execute mt-st")?;
    anyhow::ensure!(
        output.status.success(),
        "mt-st setblk 0 failed: {}",
        bounded_diagnostic(&output.stderr)
    );
    Ok(())
}

fn dar_test_args(archive_dir: &Path, threads: u16) -> Vec<OsString> {
    vec![
        "-N".into(),
        "-Q".into(),
        "-t".into(),
        archive_dir.join("archive").into_os_string(),
        "--min-digits".into(),
        "6".into(),
        "--multi-thread".into(),
        threads.to_string().into(),
    ]
}

fn dar_restore_args(archive_dir: &Path, destination: &Path, threads: u16) -> Vec<OsString> {
    vec![
        "-N".into(),
        "-Q".into(),
        "-n".into(),
        "-x".into(),
        archive_dir.join("archive").into_os_string(),
        "-R".into(),
        destination.as_os_str().to_owned(),
        "--min-digits".into(),
        "6".into(),
        "--multi-thread".into(),
        threads.to_string().into(),
    ]
}

fn run_dar(dar: &Path, args: &[OsString], operation: &str) -> Result<()> {
    let output = ProcessCommand::new(dar)
        .args(args)
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("HOME", "/var/empty")
        .output()
        .with_context(|| format!("could not start DAR {operation}"))?;
    anyhow::ensure!(
        output.status.success(),
        "DAR {operation} failed with {}: {}",
        output.status,
        bounded_diagnostic(&output.stderr)
    );
    Ok(())
}

fn validate_dar_version(dar: &Path) -> Result<()> {
    let output = ProcessCommand::new(dar)
        .arg("-V")
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("HOME", "/var/empty")
        .output()
        .context("could not query the DAR version")?;
    anyhow::ensure!(
        output.status.success(),
        "DAR version query failed: {}",
        bounded_diagnostic(&output.stderr)
    );
    anyhow::ensure!(
        dar_version_output_matches(&output.stdout, &output.stderr),
        "native archive recovery requires DAR {DAR_CODEC_VERSION}"
    );
    Ok(())
}

fn dar_version_output_matches(stdout: &[u8], stderr: &[u8]) -> bool {
    let expected = format!("dar version {DAR_CODEC_VERSION}");
    [&stdout, &stderr].into_iter().any(|output| {
        String::from_utf8_lossy(output).lines().any(|line| {
            line.trim_start()
                .strip_prefix(&expected)
                .is_some_and(|suffix| {
                    suffix.is_empty()
                        || suffix.starts_with(',')
                        || suffix.starts_with(char::is_whitespace)
                })
        })
    })
}

fn bounded_diagnostic(bytes: &[u8]) -> String {
    String::from_utf8_lossy(&bytes[..bytes.len().min(2048)])
        .chars()
        .map(|character| {
            if character.is_control() && !matches!(character, '\n' | '\r' | '\t') {
                '\u{fffd}'
            } else {
                character
            }
        })
        .collect::<String>()
        .trim()
        .to_owned()
}

fn native_header_filename(volume: u32) -> String {
    format!("volume-{volume:04}.header.tsn")
}

fn native_index_filename(volume: u32) -> String {
    format!("volume-{volume:04}.index.tsn")
}

fn native_trailer_filename(volume: u32) -> String {
    format!("volume-{volume:04}.trailer.tsn")
}

fn native_receipt_filename(volume: u32) -> String {
    format!("volume-{volume:04}.capture.json")
}

fn dar_slice_filename(slice: u32) -> String {
    format!("archive.{slice:06}.dar")
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use tapescale::native_format::{NativeCompressionCodec, NATIVE_FORMAT_VERSION};

    use super::*;

    fn digest(value: char) -> String {
        value.to_string().repeat(64)
    }

    fn artifact(file: String, bytes: u64, digest_value: char) -> CapturedArtifact {
        CapturedArtifact {
            file,
            bytes,
            sha256: digest(digest_value),
        }
    }

    fn capture_fixture(volume: u32, slice: u32, offset: u64, bytes: u64) -> NativeCaptureReceipt {
        let archive_id = Uuid::from_u128(1);
        let cartridge_id = Uuid::from_u128(100 + u128::from(volume));
        let archive_volume_id = Uuid::from_u128(200 + u128::from(volume));
        let slice_digest = if slice == 1 { digest('c') } else { digest('d') };
        let descriptor = NativeSliceDescriptor {
            slice_ordinal: slice,
            tape_file_number: 3,
            archive_offset_bytes: offset,
            encoded_bytes: bytes,
            logical_bytes: 0,
            entry_count: 0,
            sha256: slice_digest.clone(),
        };
        let index = NativeVolumeIndex {
            format_version: NATIVE_FORMAT_VERSION,
            cartridge_id,
            archive_id,
            archive_volume_id,
            volume_ordinal: volume,
            volume_count: 2,
            source_manifest_sha256: digest('a'),
            archive_catalog_sha256: digest('b'),
            total_encoded_bytes: bytes,
            total_logical_bytes: 0,
            slices: vec![descriptor.clone()],
        };
        let trailer = NativeVolumeTrailer::from_index(
            &index,
            Utc.with_ymd_and_hms(2026, 11, 19, 12, 0, 0).unwrap(),
        )
        .unwrap();
        let header = NativeVolumeHeader {
            format_version: NATIVE_FORMAT_VERSION,
            cartridge_id,
            archive_id,
            archive_volume_id,
            volume_ordinal: volume,
            volume_count: 2,
            slice_count: 1,
            representation: NativeArchiveRepresentation::FileDar,
            compression: NativeCompressionCodec::Zstd,
            tape_block_bytes: 256 * 1024,
            source_manifest_sha256: digest('a'),
            archive_catalog_sha256: digest('b'),
            created_at: Utc.with_ymd_and_hms(2026, 11, 19, 11, 0, 0).unwrap(),
        };
        NativeCaptureReceipt {
            format: CAPTURE_FORMAT.to_owned(),
            format_version: CAPTURE_FORMAT_VERSION,
            captured_at: Utc.with_ymd_and_hms(2026, 11, 19, 13, 0, 0).unwrap(),
            header,
            index,
            trailer,
            header_record: artifact(native_header_filename(volume), 100, 'e'),
            catalog: artifact("catalog.1.dar".to_owned(), 200, 'b'),
            source_manifest: artifact("source-manifest.json".to_owned(), 300, 'a'),
            slices: vec![CapturedSlice {
                descriptor,
                artifact: CapturedArtifact {
                    file: dar_slice_filename(slice),
                    bytes,
                    sha256: slice_digest,
                },
            }],
            index_record: artifact(native_index_filename(volume), 400, 'f'),
            trailer_record: artifact(native_trailer_filename(volume), 500, '0'),
        }
    }

    #[test]
    fn artifact_paths_cannot_escape_bundle() {
        let artifact = Artifact {
            file: "../catalog.sql".to_owned(),
            sha256: "0".repeat(64),
        };
        assert!(validate_artifact(&artifact).is_err());
    }

    #[test]
    fn complete_capture_set_requires_every_ordered_volume_and_slice() {
        let first = capture_fixture(1, 1, 0, 10);
        let second = capture_fixture(2, 2, 10, 20);
        assert!(validate_capture_set(&[second.clone(), first.clone()], true).is_ok());
        assert!(validate_capture_set(std::slice::from_ref(&first), true).is_err());

        let mut skipped = second;
        skipped.index.slices[0].slice_ordinal = 3;
        skipped.slices[0].descriptor.slice_ordinal = 3;
        skipped.slices[0].artifact.file = dar_slice_filename(3);
        skipped.trailer =
            NativeVolumeTrailer::from_index(&skipped.index, skipped.trailer.completed_at).unwrap();
        assert!(validate_capture_set(&[first, skipped], true).is_err());
    }

    #[test]
    fn dar_restore_command_is_closed_noninteractive_and_no_overwrite() {
        let args = dar_restore_args(Path::new("/capture"), Path::new("/restore"), 4)
            .into_iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(args[0..4], ["-N", "-Q", "-n", "-x"]);
        assert!(args.contains(&"/capture/archive".to_owned()));
        assert!(args.contains(&"/restore".to_owned()));
        assert!(args.contains(&"--multi-thread".to_owned()));
        assert!(!args.iter().any(|value| value.contains(';')));
    }

    #[test]
    fn captured_artifact_paths_cannot_escape_archive_directory() {
        let unsafe_artifact = CapturedArtifact {
            file: "../archive.000001.dar".to_owned(),
            bytes: 1,
            sha256: digest('a'),
        };
        assert!(validate_captured_artifact_name(&unsafe_artifact).is_err());
    }

    #[test]
    fn dar_recovery_requires_the_pinned_codec_version() {
        assert!(dar_version_output_matches(
            b"dar version 2.8.6, Copyright (C)\n",
            b""
        ));
        assert!(!dar_version_output_matches(
            b"dar version 2.7.15, Copyright (C)\n",
            b""
        ));
        assert!(!dar_version_output_matches(
            b"dar version 2.8.60, Copyright (C)\n",
            b""
        ));
    }
}
