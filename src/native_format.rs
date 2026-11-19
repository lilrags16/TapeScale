//! Versioned, self-describing records for the TapeScale native tape format.
//!
//! These records are deliberately independent of a database or backup engine. A
//! replacement-host recovery tool can read the volume header, index, and trailer
//! directly from tape and verify every byte before trusting any media metadata.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Binary envelope magic for every TapeScale native metadata record.
pub const NATIVE_RECORD_MAGIC: [u8; 8] = *b"TSNV1REC";
/// TapeScale Native v1 on-tape metadata format revision.
pub const NATIVE_FORMAT_VERSION: u16 = 1;
/// Binary envelope magic for bounded-spool TapeScale Native v2 records.
pub const NATIVE_ROLLING_RECORD_MAGIC: [u8; 8] = *b"TSNV2REC";
/// TapeScale Native v2 bounded-spool metadata format revision.
pub const NATIVE_ROLLING_FORMAT_VERSION: u16 = 2;
/// Default physical record ceiling used by the first native writer.
pub const DEFAULT_TAPE_BLOCK_BYTES: u32 = 256 * 1024;

const RECORD_PREFIX_BYTES: usize = 48;
const MAX_METADATA_RECORD_BYTES: usize = 16 * 1024 * 1024;
const MAX_INDEX_SLICES: usize = 1_000_000;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeArchiveRepresentation {
    FileDar,
    NativeZfsSend,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeCompressionCodec {
    None,
    Zstd,
    Lz4,
    Gzip,
}

/// The identity record written when blank or expendable scratch media is prepared.
///
/// This record deliberately has its own wire kind and does not contain an archive
/// identifier. The archive writer replaces it with a [`NativeVolumeHeader`] at
/// the destructive write boundary; a recovery reader can never confuse the two.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativePreparationHeader {
    pub format_version: u16,
    pub cartridge_id: Uuid,
    pub prepared_at: DateTime<Utc>,
    pub qualification_sha256: Option<String>,
    pub drive_identity_sha256: Option<String>,
}

impl NativePreparationHeader {
    pub fn validate(&self) -> Result<(), NativeFormatError> {
        if self.format_version != NATIVE_FORMAT_VERSION {
            return Err(NativeFormatError::UnsupportedVersion(self.format_version));
        }
        if self.cartridge_id.is_nil() {
            return Err(NativeFormatError::Invalid(
                "preparation cartridge_id must be non-nil".to_owned(),
            ));
        }
        if let Some(digest) = &self.qualification_sha256 {
            validate_sha256("qualification_sha256", digest)?;
        }
        if let Some(digest) = &self.drive_identity_sha256 {
            validate_sha256("drive_identity_sha256", digest)?;
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>, NativeFormatError> {
        self.validate()?;
        encode_record(RecordKind::Preparation, self)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, NativeFormatError> {
        let value: Self = decode_record(RecordKind::Preparation, bytes)?;
        value.validate()?;
        Ok(value)
    }
}

/// The first tape file on every native archive volume.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeVolumeHeader {
    pub format_version: u16,
    pub cartridge_id: Uuid,
    pub archive_id: Uuid,
    /// Stable identity for this physical member of the archive volume set.
    pub archive_volume_id: Uuid,
    /// One-based position in the archive's ordered media set.
    pub volume_ordinal: u32,
    pub volume_count: u32,
    /// Number of complete DAR slices carried by this volume. This lets an
    /// independent reader locate the index and trailer without a database or
    /// an ambiguous end-of-data scan.
    pub slice_count: u32,
    pub representation: NativeArchiveRepresentation,
    pub compression: NativeCompressionCodec,
    pub tape_block_bytes: u32,
    pub source_manifest_sha256: String,
    pub archive_catalog_sha256: String,
    pub created_at: DateTime<Utc>,
}

impl NativeVolumeHeader {
    pub fn validate(&self) -> Result<(), NativeFormatError> {
        validate_binding(
            self.format_version,
            self.cartridge_id,
            self.archive_id,
            self.archive_volume_id,
            self.volume_ordinal,
            self.volume_count,
        )?;
        if self.slice_count == 0 || self.slice_count as usize > MAX_INDEX_SLICES {
            return Err(NativeFormatError::Invalid(format!(
                "slice_count must be from 1 through {MAX_INDEX_SLICES}"
            )));
        }
        if self.tape_block_bytes < 64 * 1024
            || self.tape_block_bytes > 4 * 1024 * 1024
            || !self.tape_block_bytes.is_power_of_two()
        {
            return Err(NativeFormatError::Invalid(
                "tape_block_bytes must be a power of two from 64 KiB through 4 MiB".to_owned(),
            ));
        }
        validate_sha256("source_manifest_sha256", &self.source_manifest_sha256)?;
        validate_sha256("archive_catalog_sha256", &self.archive_catalog_sha256)
    }

    pub fn encode(&self) -> Result<Vec<u8>, NativeFormatError> {
        self.validate()?;
        encode_record(RecordKind::Header, self)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, NativeFormatError> {
        let value: Self = decode_record(RecordKind::Header, bytes)?;
        value.validate()?;
        Ok(value)
    }
}

/// One independently hashed archive slice stored as one tape file.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeSliceDescriptor {
    /// One-based position in the complete archive slice sequence.
    pub slice_ordinal: u32,
    /// Zero-based physical tape-file number. Files 0, 1, and 2 are the native
    /// header, isolated DAR catalog, and source manifest respectively.
    pub tape_file_number: u32,
    /// Offset of this slice in the logical encoded archive stream.
    pub archive_offset_bytes: u64,
    pub encoded_bytes: u64,
    pub logical_bytes: u64,
    pub entry_count: u64,
    pub sha256: String,
}

impl NativeSliceDescriptor {
    fn validate(&self) -> Result<(), NativeFormatError> {
        if self.slice_ordinal == 0 {
            return Err(NativeFormatError::Invalid(
                "slice_ordinal must be one-based".to_owned(),
            ));
        }
        if self.tape_file_number < 3 {
            return Err(NativeFormatError::Invalid(
                "slice tape_file_number must leave files zero through two for recovery metadata"
                    .to_owned(),
            ));
        }
        if self.encoded_bytes == 0 {
            return Err(NativeFormatError::Invalid(
                "slice encoded_bytes must be non-zero".to_owned(),
            ));
        }
        validate_sha256("slice sha256", &self.sha256)
    }
}

/// Volume-local mapping from archive slices to physical tape files.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeVolumeIndex {
    pub format_version: u16,
    pub cartridge_id: Uuid,
    pub archive_id: Uuid,
    pub archive_volume_id: Uuid,
    pub volume_ordinal: u32,
    pub volume_count: u32,
    pub source_manifest_sha256: String,
    pub archive_catalog_sha256: String,
    pub total_encoded_bytes: u64,
    pub total_logical_bytes: u64,
    pub slices: Vec<NativeSliceDescriptor>,
}

impl NativeVolumeIndex {
    pub fn validate(&self) -> Result<(), NativeFormatError> {
        validate_binding(
            self.format_version,
            self.cartridge_id,
            self.archive_id,
            self.archive_volume_id,
            self.volume_ordinal,
            self.volume_count,
        )?;
        validate_sha256("source_manifest_sha256", &self.source_manifest_sha256)?;
        validate_sha256("archive_catalog_sha256", &self.archive_catalog_sha256)?;
        if self.slices.is_empty() {
            return Err(NativeFormatError::Invalid(
                "volume index must contain at least one archive slice".to_owned(),
            ));
        }
        if self.slices.len() > MAX_INDEX_SLICES {
            return Err(NativeFormatError::Invalid(format!(
                "volume index exceeds the {MAX_INDEX_SLICES} slice limit"
            )));
        }

        let mut encoded_total = 0_u64;
        let mut logical_total = 0_u64;
        let mut previous_slice = None;
        let mut previous_tape_file = None;
        let mut expected_offset = None;
        for slice in &self.slices {
            slice.validate()?;
            if previous_slice.is_some_and(|previous| slice.slice_ordinal <= previous) {
                return Err(NativeFormatError::Invalid(
                    "slice ordinals must be strictly increasing".to_owned(),
                ));
            }
            if previous_tape_file.is_some_and(|previous| slice.tape_file_number <= previous) {
                return Err(NativeFormatError::Invalid(
                    "slice tape file numbers must be strictly increasing".to_owned(),
                ));
            }
            if expected_offset.is_some_and(|offset| slice.archive_offset_bytes != offset) {
                return Err(NativeFormatError::Invalid(
                    "slice archive offsets must be contiguous".to_owned(),
                ));
            }
            encoded_total = encoded_total
                .checked_add(slice.encoded_bytes)
                .ok_or_else(|| {
                    NativeFormatError::Invalid("encoded byte total overflowed".into())
                })?;
            logical_total = logical_total
                .checked_add(slice.logical_bytes)
                .ok_or_else(|| {
                    NativeFormatError::Invalid("logical byte total overflowed".into())
                })?;
            expected_offset = Some(
                slice
                    .archive_offset_bytes
                    .checked_add(slice.encoded_bytes)
                    .ok_or_else(|| {
                        NativeFormatError::Invalid("slice archive offset overflowed".into())
                    })?,
            );
            previous_slice = Some(slice.slice_ordinal);
            previous_tape_file = Some(slice.tape_file_number);
        }
        if encoded_total != self.total_encoded_bytes {
            return Err(NativeFormatError::Invalid(
                "total_encoded_bytes does not match the slice ledger".to_owned(),
            ));
        }
        if logical_total != self.total_logical_bytes {
            return Err(NativeFormatError::Invalid(
                "total_logical_bytes does not match the slice ledger".to_owned(),
            ));
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>, NativeFormatError> {
        self.validate()?;
        encode_record(RecordKind::Index, self)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, NativeFormatError> {
        let value: Self = decode_record(RecordKind::Index, bytes)?;
        value.validate()?;
        Ok(value)
    }

    pub fn payload_sha256(&self) -> Result<String, NativeFormatError> {
        self.validate()?;
        payload_sha256(self)
    }

    pub fn ordered_slice_sha256(&self) -> Result<String, NativeFormatError> {
        self.validate()?;
        let mut digest = Sha256::new();
        for slice in &self.slices {
            let raw = decode_hex_sha256(&slice.sha256).ok_or_else(|| {
                NativeFormatError::Invalid("slice sha256 is not canonical hexadecimal".to_owned())
            })?;
            digest.update(raw);
        }
        Ok(format!("{:x}", digest.finalize()))
    }
}

/// End-of-volume commitment binding the header, index, and physical payload.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeVolumeTrailer {
    pub format_version: u16,
    pub cartridge_id: Uuid,
    pub archive_id: Uuid,
    pub archive_volume_id: Uuid,
    pub volume_ordinal: u32,
    pub volume_count: u32,
    pub slice_count: u32,
    pub total_encoded_bytes: u64,
    pub total_logical_bytes: u64,
    pub index_sha256: String,
    /// SHA-256 over the ordered raw SHA-256 values of all slice payloads.
    pub ordered_slice_sha256: String,
    pub completed_at: DateTime<Utc>,
}

impl NativeVolumeTrailer {
    pub fn from_index(
        index: &NativeVolumeIndex,
        completed_at: DateTime<Utc>,
    ) -> Result<Self, NativeFormatError> {
        index.validate()?;
        let slice_count = u32::try_from(index.slices.len()).map_err(|_| {
            NativeFormatError::Invalid("slice count cannot be represented on tape".to_owned())
        })?;
        Ok(Self {
            format_version: index.format_version,
            cartridge_id: index.cartridge_id,
            archive_id: index.archive_id,
            archive_volume_id: index.archive_volume_id,
            volume_ordinal: index.volume_ordinal,
            volume_count: index.volume_count,
            slice_count,
            total_encoded_bytes: index.total_encoded_bytes,
            total_logical_bytes: index.total_logical_bytes,
            index_sha256: index.payload_sha256()?,
            ordered_slice_sha256: index.ordered_slice_sha256()?,
            completed_at,
        })
    }

    pub fn validate(&self) -> Result<(), NativeFormatError> {
        validate_binding(
            self.format_version,
            self.cartridge_id,
            self.archive_id,
            self.archive_volume_id,
            self.volume_ordinal,
            self.volume_count,
        )?;
        if self.slice_count == 0 {
            return Err(NativeFormatError::Invalid(
                "slice_count must be non-zero".to_owned(),
            ));
        }
        validate_sha256("index_sha256", &self.index_sha256)?;
        validate_sha256("ordered_slice_sha256", &self.ordered_slice_sha256)
    }

    pub fn verify_index(&self, index: &NativeVolumeIndex) -> Result<(), NativeFormatError> {
        self.validate()?;
        index.validate()?;
        if self.format_version != index.format_version
            || self.cartridge_id != index.cartridge_id
            || self.archive_id != index.archive_id
            || self.archive_volume_id != index.archive_volume_id
            || self.volume_ordinal != index.volume_ordinal
            || self.volume_count != index.volume_count
            || u64::from(self.slice_count) != index.slices.len() as u64
            || self.total_encoded_bytes != index.total_encoded_bytes
            || self.total_logical_bytes != index.total_logical_bytes
            || self.index_sha256 != index.payload_sha256()?
            || self.ordered_slice_sha256 != index.ordered_slice_sha256()?
        {
            return Err(NativeFormatError::Integrity(
                "volume trailer does not match its index".to_owned(),
            ));
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>, NativeFormatError> {
        self.validate()?;
        encode_record(RecordKind::Trailer, self)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, NativeFormatError> {
        let value: Self = decode_record(RecordKind::Trailer, bytes)?;
        value.validate()?;
        Ok(value)
    }
}

/// Payload representation used by one independently finalized Native v2 pack.
///
/// A DAR pack carries its own isolated catalogue and can be tested before tape
/// motion. A native file chunk is reserved for a single source object that is
/// larger than the bounded spool window.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NativePackKind {
    DarArchive,
    NativeFileChunk,
}

/// One independently finalized pack in the Native v2 archive set.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativePackDescriptor {
    pub pack_id: Uuid,
    /// One-based position in the complete archive pack sequence.
    pub pack_ordinal: u32,
    pub kind: NativePackKind,
    /// Physical tape file containing this descriptor. Tape file zero is the
    /// volume header and file one is the authenticated source manifest. The
    /// catalogue/payload files immediately follow this descriptor.
    pub first_tape_file_number: u32,
    /// Number of contiguous tape files occupied by this pack.
    pub tape_file_count: u32,
    /// Offset in the complete encoded archive-set payload.
    pub archive_offset_bytes: u64,
    pub encoded_bytes: u64,
    pub logical_bytes: u64,
    pub entry_count: u64,
    /// Commitment to the ordered catalogue/payload bytes after the descriptor.
    pub content_sha256: String,
    /// DAR packs require an independently generated catalogue. Native file
    /// chunks deliberately have no DAR catalogue.
    pub catalog_sha256: Option<String>,
}

impl NativePackDescriptor {
    pub fn validate(&self) -> Result<(), NativeFormatError> {
        if self.pack_id.is_nil() || self.pack_ordinal == 0 {
            return Err(NativeFormatError::Invalid(
                "pack_id must be non-nil and pack_ordinal must be one-based".to_owned(),
            ));
        }
        if self.first_tape_file_number < 2 || self.tape_file_count == 0 {
            return Err(NativeFormatError::Invalid(
                "pack tape files must begin after header/manifest and have non-zero length"
                    .to_owned(),
            ));
        }
        if self.encoded_bytes == 0 || self.entry_count == 0 {
            return Err(NativeFormatError::Invalid(
                "pack encoded_bytes and entry_count must be non-zero".to_owned(),
            ));
        }
        self.first_tape_file_number
            .checked_add(self.tape_file_count)
            .ok_or_else(|| NativeFormatError::Invalid("pack tape-file range overflowed".into()))?;
        self.archive_offset_bytes
            .checked_add(self.encoded_bytes)
            .ok_or_else(|| NativeFormatError::Invalid("pack archive offset overflowed".into()))?;
        validate_sha256("pack content_sha256", &self.content_sha256)?;
        match (self.kind, &self.catalog_sha256) {
            (NativePackKind::DarArchive, Some(digest)) => {
                validate_sha256("pack catalog_sha256", digest)?;
                if self.tape_file_count < 3 {
                    return Err(NativeFormatError::Invalid(
                        "a DAR pack must contain descriptor, catalogue, and payload tape files"
                            .to_owned(),
                    ));
                }
            }
            (NativePackKind::DarArchive, None) => {
                return Err(NativeFormatError::Invalid(
                    "a DAR pack requires catalog_sha256".to_owned(),
                ));
            }
            (NativePackKind::NativeFileChunk, Some(_)) => {
                return Err(NativeFormatError::Invalid(
                    "a native file chunk cannot claim a DAR catalogue".to_owned(),
                ));
            }
            (NativePackKind::NativeFileChunk, None) if self.tape_file_count >= 2 => {}
            (NativePackKind::NativeFileChunk, None) => {
                return Err(NativeFormatError::Invalid(
                    "a native file chunk must contain descriptor and payload tape files".to_owned(),
                ));
            }
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>, NativeFormatError> {
        self.validate()?;
        encode_v2_record(RecordKind::Pack, self)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, NativeFormatError> {
        let value: Self = decode_v2_record(RecordKind::Pack, bytes)?;
        value.validate()?;
        Ok(value)
    }
}

/// Open-ended Native v2 member header. The final number of cartridges is not
/// known when early members are written, so ordering is committed by a chain of
/// prior volume identities and index hashes instead of a provisional count.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeV2VolumeHeader {
    pub format_version: u16,
    pub cartridge_id: Uuid,
    pub archive_id: Uuid,
    pub archive_volume_id: Uuid,
    pub volume_ordinal: u32,
    pub previous_archive_volume_id: Option<Uuid>,
    pub previous_volume_index_sha256: Option<String>,
    /// Optional when a writer intentionally leaves the member open until the
    /// bounded queue reaches a safe filemark boundary.
    pub declared_pack_count: Option<u32>,
    pub representation: NativeArchiveRepresentation,
    pub compression: NativeCompressionCodec,
    pub tape_block_bytes: u32,
    pub source_manifest_sha256: String,
    pub created_at: DateTime<Utc>,
}

impl NativeV2VolumeHeader {
    pub fn validate(&self) -> Result<(), NativeFormatError> {
        validate_v2_binding(
            self.format_version,
            self.cartridge_id,
            self.archive_id,
            self.archive_volume_id,
            self.volume_ordinal,
            self.previous_archive_volume_id,
            self.previous_volume_index_sha256.as_deref(),
        )?;
        if self.declared_pack_count == Some(0) {
            return Err(NativeFormatError::Invalid(
                "declared_pack_count must be non-zero when present".to_owned(),
            ));
        }
        if self
            .declared_pack_count
            .is_some_and(|count| count as usize > MAX_INDEX_SLICES)
        {
            return Err(NativeFormatError::Invalid(format!(
                "declared_pack_count exceeds {MAX_INDEX_SLICES}"
            )));
        }
        validate_tape_block_bytes(self.tape_block_bytes)?;
        validate_sha256("source_manifest_sha256", &self.source_manifest_sha256)
    }

    pub fn encode(&self) -> Result<Vec<u8>, NativeFormatError> {
        self.validate()?;
        encode_v2_record(RecordKind::Header, self)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, NativeFormatError> {
        let value: Self = decode_v2_record(RecordKind::Header, bytes)?;
        value.validate()?;
        Ok(value)
    }
}

/// Volume-local Native v2 pack ledger.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeV2VolumeIndex {
    pub format_version: u16,
    pub cartridge_id: Uuid,
    pub archive_id: Uuid,
    pub archive_volume_id: Uuid,
    pub volume_ordinal: u32,
    pub previous_archive_volume_id: Option<Uuid>,
    pub previous_volume_index_sha256: Option<String>,
    pub source_manifest_sha256: String,
    pub total_encoded_bytes: u64,
    pub total_logical_bytes: u64,
    pub total_entries: u64,
    pub packs: Vec<NativePackDescriptor>,
}

impl NativeV2VolumeIndex {
    pub fn validate(&self) -> Result<(), NativeFormatError> {
        validate_v2_binding(
            self.format_version,
            self.cartridge_id,
            self.archive_id,
            self.archive_volume_id,
            self.volume_ordinal,
            self.previous_archive_volume_id,
            self.previous_volume_index_sha256.as_deref(),
        )?;
        validate_sha256("source_manifest_sha256", &self.source_manifest_sha256)?;
        if self.packs.is_empty() || self.packs.len() > MAX_INDEX_SLICES {
            return Err(NativeFormatError::Invalid(format!(
                "volume index must contain from 1 through {MAX_INDEX_SLICES} packs"
            )));
        }

        let mut encoded_total = 0_u64;
        let mut logical_total = 0_u64;
        let mut entry_total = 0_u64;
        let mut expected_pack = self.packs[0].pack_ordinal;
        let mut expected_tape_file = self.packs[0].first_tape_file_number;
        let mut expected_offset = self.packs[0].archive_offset_bytes;
        let mut pack_ids = HashSet::with_capacity(self.packs.len());
        for pack in &self.packs {
            pack.validate()?;
            if !pack_ids.insert(pack.pack_id) {
                return Err(NativeFormatError::Invalid(
                    "pack identities must be unique within a volume".to_owned(),
                ));
            }
            if pack.pack_ordinal != expected_pack {
                return Err(NativeFormatError::Invalid(
                    "pack ordinals must be contiguous within a volume".to_owned(),
                ));
            }
            if pack.first_tape_file_number != expected_tape_file {
                return Err(NativeFormatError::Invalid(
                    "pack tape-file ranges must be contiguous within a volume".to_owned(),
                ));
            }
            if pack.archive_offset_bytes != expected_offset {
                return Err(NativeFormatError::Invalid(
                    "pack archive offsets must be contiguous within a volume".to_owned(),
                ));
            }
            encoded_total = encoded_total
                .checked_add(pack.encoded_bytes)
                .ok_or_else(|| {
                    NativeFormatError::Invalid("encoded pack byte total overflowed".into())
                })?;
            logical_total = logical_total
                .checked_add(pack.logical_bytes)
                .ok_or_else(|| {
                    NativeFormatError::Invalid("logical pack byte total overflowed".into())
                })?;
            entry_total = entry_total
                .checked_add(pack.entry_count)
                .ok_or_else(|| NativeFormatError::Invalid("pack entry total overflowed".into()))?;
            expected_pack = expected_pack
                .checked_add(1)
                .ok_or_else(|| NativeFormatError::Invalid("pack ordinal overflowed".into()))?;
            expected_tape_file = expected_tape_file
                .checked_add(pack.tape_file_count)
                .ok_or_else(|| NativeFormatError::Invalid("pack tape files overflowed".into()))?;
            expected_offset = expected_offset
                .checked_add(pack.encoded_bytes)
                .ok_or_else(|| NativeFormatError::Invalid("pack offset overflowed".into()))?;
        }
        if encoded_total != self.total_encoded_bytes
            || logical_total != self.total_logical_bytes
            || entry_total != self.total_entries
        {
            return Err(NativeFormatError::Invalid(
                "volume totals do not match the pack ledger".to_owned(),
            ));
        }
        Ok(())
    }

    pub fn verify_header(&self, header: &NativeV2VolumeHeader) -> Result<(), NativeFormatError> {
        self.validate()?;
        header.validate()?;
        if self.format_version != header.format_version
            || self.cartridge_id != header.cartridge_id
            || self.archive_id != header.archive_id
            || self.archive_volume_id != header.archive_volume_id
            || self.volume_ordinal != header.volume_ordinal
            || self.previous_archive_volume_id != header.previous_archive_volume_id
            || self.previous_volume_index_sha256 != header.previous_volume_index_sha256
            || self.source_manifest_sha256 != header.source_manifest_sha256
            || header
                .declared_pack_count
                .is_some_and(|count| count as usize != self.packs.len())
        {
            return Err(NativeFormatError::Integrity(
                "Native v2 volume index does not match its header".to_owned(),
            ));
        }
        Ok(())
    }

    pub fn payload_sha256(&self) -> Result<String, NativeFormatError> {
        self.validate()?;
        payload_sha256(self)
    }

    pub fn ordered_pack_sha256(&self) -> Result<String, NativeFormatError> {
        self.validate()?;
        ordered_sha256_digest(self.packs.iter().map(|pack| pack.content_sha256.as_str()))
    }

    pub fn encode(&self) -> Result<Vec<u8>, NativeFormatError> {
        self.validate()?;
        encode_v2_record(RecordKind::Index, self)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, NativeFormatError> {
        let value: Self = decode_v2_record(RecordKind::Index, bytes)?;
        value.validate()?;
        Ok(value)
    }
}

/// End-of-member Native v2 commitment.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeV2VolumeTrailer {
    pub format_version: u16,
    pub cartridge_id: Uuid,
    pub archive_id: Uuid,
    pub archive_volume_id: Uuid,
    pub volume_ordinal: u32,
    pub pack_count: u32,
    pub total_encoded_bytes: u64,
    pub total_logical_bytes: u64,
    pub total_entries: u64,
    pub index_sha256: String,
    pub ordered_pack_sha256: String,
    /// Present only on the final volume. The seal is written immediately
    /// before this trailer, so the trailer can bind it without a digest cycle.
    pub archive_set_seal_sha256: Option<String>,
    pub completed_at: DateTime<Utc>,
}

impl NativeV2VolumeTrailer {
    pub fn from_index(
        index: &NativeV2VolumeIndex,
        completed_at: DateTime<Utc>,
    ) -> Result<Self, NativeFormatError> {
        index.validate()?;
        Ok(Self {
            format_version: index.format_version,
            cartridge_id: index.cartridge_id,
            archive_id: index.archive_id,
            archive_volume_id: index.archive_volume_id,
            volume_ordinal: index.volume_ordinal,
            pack_count: u32::try_from(index.packs.len()).map_err(|_| {
                NativeFormatError::Invalid("pack count cannot be represented".to_owned())
            })?,
            total_encoded_bytes: index.total_encoded_bytes,
            total_logical_bytes: index.total_logical_bytes,
            total_entries: index.total_entries,
            index_sha256: index.payload_sha256()?,
            ordered_pack_sha256: index.ordered_pack_sha256()?,
            archive_set_seal_sha256: None,
            completed_at,
        })
    }

    pub fn from_index_with_archive_set_seal(
        index: &NativeV2VolumeIndex,
        seal: &NativeArchiveSetSeal,
        completed_at: DateTime<Utc>,
    ) -> Result<Self, NativeFormatError> {
        index.validate()?;
        seal.validate()?;
        let index_sha256 = index.payload_sha256()?;
        if seal.archive_id != index.archive_id
            || seal.final_archive_volume_id != index.archive_volume_id
            || seal
                .volumes
                .last()
                .is_none_or(|volume| volume.index_sha256 != index_sha256)
        {
            return Err(NativeFormatError::Integrity(
                "archive-set seal does not bind the final volume index".to_owned(),
            ));
        }
        let mut trailer = Self::from_index(index, completed_at)?;
        trailer.archive_set_seal_sha256 = Some(seal.archive_set_sha256.clone());
        trailer.validate()?;
        Ok(trailer)
    }

    pub fn validate(&self) -> Result<(), NativeFormatError> {
        if self.format_version != NATIVE_ROLLING_FORMAT_VERSION {
            return Err(NativeFormatError::UnsupportedVersion(self.format_version));
        }
        if self.cartridge_id.is_nil()
            || self.archive_id.is_nil()
            || self.archive_volume_id.is_nil()
            || self.volume_ordinal == 0
            || self.pack_count == 0
        {
            return Err(NativeFormatError::Invalid(
                "Native v2 trailer identities, ordinal, and pack_count must be present".to_owned(),
            ));
        }
        validate_sha256("index_sha256", &self.index_sha256)?;
        validate_sha256("ordered_pack_sha256", &self.ordered_pack_sha256)?;
        if let Some(digest) = &self.archive_set_seal_sha256 {
            validate_sha256("archive_set_seal_sha256", digest)?;
        }
        Ok(())
    }

    pub fn verify_index(&self, index: &NativeV2VolumeIndex) -> Result<(), NativeFormatError> {
        self.validate()?;
        index.validate()?;
        if self.format_version != index.format_version
            || self.cartridge_id != index.cartridge_id
            || self.archive_id != index.archive_id
            || self.archive_volume_id != index.archive_volume_id
            || self.volume_ordinal != index.volume_ordinal
            || self.pack_count as usize != index.packs.len()
            || self.total_encoded_bytes != index.total_encoded_bytes
            || self.total_logical_bytes != index.total_logical_bytes
            || self.total_entries != index.total_entries
            || self.index_sha256 != index.payload_sha256()?
            || self.ordered_pack_sha256 != index.ordered_pack_sha256()?
        {
            return Err(NativeFormatError::Integrity(
                "Native v2 volume trailer does not match its index".to_owned(),
            ));
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>, NativeFormatError> {
        self.validate()?;
        encode_v2_record(RecordKind::Trailer, self)
    }

    pub fn verify_archive_set_seal(
        &self,
        seal: &NativeArchiveSetSeal,
    ) -> Result<(), NativeFormatError> {
        self.validate()?;
        seal.validate()?;
        let final_volume = seal.volumes.last().ok_or_else(|| {
            NativeFormatError::Integrity("archive-set seal has no final volume".to_owned())
        })?;
        if self.archive_set_seal_sha256.as_deref() != Some(&seal.archive_set_sha256)
            || self.archive_id != seal.archive_id
            || self.archive_volume_id != seal.final_archive_volume_id
            || self.volume_ordinal != seal.volume_count
            || self.pack_count != final_volume.pack_count
            || self.total_encoded_bytes != final_volume.encoded_bytes
            || self.total_logical_bytes != final_volume.logical_bytes
            || self.total_entries != final_volume.entry_count
        {
            return Err(NativeFormatError::Integrity(
                "final volume trailer does not match the archive-set seal".to_owned(),
            ));
        }
        Ok(())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, NativeFormatError> {
        let value: Self = decode_v2_record(RecordKind::Trailer, bytes)?;
        value.validate()?;
        Ok(value)
    }
}

/// One member in the final archive-set commitment.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeSealedVolume {
    pub volume_ordinal: u32,
    pub archive_volume_id: Uuid,
    pub previous_archive_volume_id: Option<Uuid>,
    pub previous_volume_index_sha256: Option<String>,
    pub index_sha256: String,
    pub first_pack_ordinal: u32,
    pub pack_count: u32,
    pub encoded_bytes: u64,
    pub logical_bytes: u64,
    pub entry_count: u64,
}

/// Final Native v2 archive-set seal. No recovery point is complete until this
/// record and every volume/index commitment below it have been verified.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeArchiveSetSeal {
    pub format_version: u16,
    pub archive_id: Uuid,
    pub final_archive_volume_id: Uuid,
    pub source_manifest_sha256: String,
    /// SHA-256 over the ordered raw catalogue digests of every DAR pack.
    pub pack_catalog_root_sha256: String,
    pub volume_count: u32,
    pub pack_count: u32,
    pub total_encoded_bytes: u64,
    pub total_logical_bytes: u64,
    pub total_entries: u64,
    pub ordered_volume_index_sha256: String,
    pub ordered_pack_sha256: String,
    pub archive_set_sha256: String,
    pub volumes: Vec<NativeSealedVolume>,
    pub completed_at: DateTime<Utc>,
}

impl NativeArchiveSetSeal {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        archive_id: Uuid,
        source_manifest_sha256: String,
        pack_catalog_root_sha256: String,
        ordered_pack_sha256: String,
        total_encoded_bytes: u64,
        total_logical_bytes: u64,
        total_entries: u64,
        volumes: Vec<NativeSealedVolume>,
        completed_at: DateTime<Utc>,
    ) -> Result<Self, NativeFormatError> {
        let final_archive_volume_id = volumes
            .last()
            .map(|volume| volume.archive_volume_id)
            .ok_or_else(|| {
                NativeFormatError::Invalid("seal requires at least one volume".into())
            })?;
        let volume_count = u32::try_from(volumes.len()).map_err(|_| {
            NativeFormatError::Invalid("volume count cannot be represented".to_owned())
        })?;
        let pack_count = volumes
            .iter()
            .try_fold(0_u32, |total, volume| total.checked_add(volume.pack_count))
            .ok_or_else(|| NativeFormatError::Invalid("pack count overflowed".to_owned()))?;
        let ordered_volume_index_sha256 =
            ordered_sha256_digest(volumes.iter().map(|volume| volume.index_sha256.as_str()))?;
        let mut seal = Self {
            format_version: NATIVE_ROLLING_FORMAT_VERSION,
            archive_id,
            final_archive_volume_id,
            source_manifest_sha256,
            pack_catalog_root_sha256,
            volume_count,
            pack_count,
            total_encoded_bytes,
            total_logical_bytes,
            total_entries,
            ordered_volume_index_sha256,
            ordered_pack_sha256,
            archive_set_sha256: String::new(),
            volumes,
            completed_at,
        };
        seal.archive_set_sha256 = seal.calculated_archive_set_sha256()?;
        seal.validate()?;
        Ok(seal)
    }

    pub fn validate(&self) -> Result<(), NativeFormatError> {
        if self.format_version != NATIVE_ROLLING_FORMAT_VERSION {
            return Err(NativeFormatError::UnsupportedVersion(self.format_version));
        }
        if self.archive_id.is_nil() || self.final_archive_volume_id.is_nil() {
            return Err(NativeFormatError::Invalid(
                "seal archive identities must be non-nil".to_owned(),
            ));
        }
        validate_sha256("source_manifest_sha256", &self.source_manifest_sha256)?;
        validate_sha256("pack_catalog_root_sha256", &self.pack_catalog_root_sha256)?;
        validate_sha256(
            "ordered_volume_index_sha256",
            &self.ordered_volume_index_sha256,
        )?;
        validate_sha256("ordered_pack_sha256", &self.ordered_pack_sha256)?;
        validate_sha256("archive_set_sha256", &self.archive_set_sha256)?;
        if self.volumes.is_empty()
            || self.volume_count as usize != self.volumes.len()
            || self.pack_count == 0
            || self.total_encoded_bytes == 0
            || self.total_entries == 0
        {
            return Err(NativeFormatError::Invalid(
                "seal counts and encoded totals must describe a non-empty archive".to_owned(),
            ));
        }

        let mut expected_pack = 1_u32;
        let mut encoded_total = 0_u64;
        let mut logical_total = 0_u64;
        let mut entry_total = 0_u64;
        let mut prior_volume_id = None;
        let mut prior_index_sha256: Option<&str> = None;
        let mut volume_ids = HashSet::with_capacity(self.volumes.len());
        for (position, volume) in self.volumes.iter().enumerate() {
            let expected_volume = u32::try_from(position + 1).map_err(|_| {
                NativeFormatError::Invalid("volume ordinal cannot be represented".to_owned())
            })?;
            if volume.volume_ordinal != expected_volume
                || volume.archive_volume_id.is_nil()
                || volume.first_pack_ordinal != expected_pack
                || volume.pack_count == 0
                || volume.encoded_bytes == 0
                || volume.entry_count == 0
                || !volume_ids.insert(volume.archive_volume_id)
                || volume.previous_archive_volume_id != prior_volume_id
                || volume.previous_volume_index_sha256.as_deref() != prior_index_sha256
            {
                return Err(NativeFormatError::Invalid(
                    "sealed volumes must form one contiguous identity, pack, and index chain"
                        .to_owned(),
                ));
            }
            validate_sha256("sealed volume index_sha256", &volume.index_sha256)?;
            expected_pack = expected_pack
                .checked_add(volume.pack_count)
                .ok_or_else(|| {
                    NativeFormatError::Invalid("sealed pack ordinal overflowed".into())
                })?;
            encoded_total = encoded_total
                .checked_add(volume.encoded_bytes)
                .ok_or_else(|| {
                    NativeFormatError::Invalid("sealed encoded byte total overflowed".into())
                })?;
            logical_total = logical_total
                .checked_add(volume.logical_bytes)
                .ok_or_else(|| {
                    NativeFormatError::Invalid("sealed logical byte total overflowed".into())
                })?;
            entry_total = entry_total.checked_add(volume.entry_count).ok_or_else(|| {
                NativeFormatError::Invalid("sealed entry total overflowed".into())
            })?;
            prior_volume_id = Some(volume.archive_volume_id);
            prior_index_sha256 = Some(volume.index_sha256.as_str());
        }
        if prior_volume_id != Some(self.final_archive_volume_id)
            || expected_pack.checked_sub(1) != Some(self.pack_count)
            || encoded_total != self.total_encoded_bytes
            || logical_total != self.total_logical_bytes
            || entry_total != self.total_entries
            || self.ordered_volume_index_sha256
                != ordered_sha256_digest(
                    self.volumes
                        .iter()
                        .map(|volume| volume.index_sha256.as_str()),
                )?
            || self.archive_set_sha256 != self.calculated_archive_set_sha256()?
        {
            return Err(NativeFormatError::Integrity(
                "archive-set seal totals or digest commitments do not match".to_owned(),
            ));
        }
        Ok(())
    }

    pub fn calculated_archive_set_sha256(&self) -> Result<String, NativeFormatError> {
        validate_sha256("source_manifest_sha256", &self.source_manifest_sha256)?;
        validate_sha256("pack_catalog_root_sha256", &self.pack_catalog_root_sha256)?;
        validate_sha256("ordered_pack_sha256", &self.ordered_pack_sha256)?;
        let mut digest = Sha256::new();
        digest.update(self.archive_id.as_bytes());
        digest.update(decode_hex_sha256(&self.source_manifest_sha256).expect("validated digest"));
        digest.update(decode_hex_sha256(&self.pack_catalog_root_sha256).expect("validated digest"));
        digest.update(decode_hex_sha256(&self.ordered_pack_sha256).expect("validated digest"));
        digest.update(self.volume_count.to_be_bytes());
        digest.update(self.pack_count.to_be_bytes());
        digest.update(self.total_encoded_bytes.to_be_bytes());
        digest.update(self.total_logical_bytes.to_be_bytes());
        digest.update(self.total_entries.to_be_bytes());
        for volume in &self.volumes {
            digest.update(volume.archive_volume_id.as_bytes());
            digest.update(decode_hex_sha256(&volume.index_sha256).ok_or_else(|| {
                NativeFormatError::Invalid("sealed volume index_sha256 is invalid".to_owned())
            })?);
        }
        Ok(format!("{:x}", digest.finalize()))
    }

    pub fn encode(&self) -> Result<Vec<u8>, NativeFormatError> {
        self.validate()?;
        encode_v2_record(RecordKind::Seal, self)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, NativeFormatError> {
        let value: Self = decode_v2_record(RecordKind::Seal, bytes)?;
        value.validate()?;
        Ok(value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum NativeFormatError {
    #[error("native metadata record is invalid: {0}")]
    Invalid(String),
    #[error("native metadata record failed integrity validation: {0}")]
    Integrity(String),
    #[error("native metadata record uses unsupported format version {0}")]
    UnsupportedVersion(u16),
    #[error("native metadata record could not be serialized: {0}")]
    Serialization(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum RecordKind {
    Header = 1,
    Index = 2,
    Trailer = 3,
    Preparation = 4,
    Pack = 5,
    Seal = 6,
}

fn validate_binding(
    format_version: u16,
    cartridge_id: Uuid,
    archive_id: Uuid,
    archive_volume_id: Uuid,
    volume_ordinal: u32,
    volume_count: u32,
) -> Result<(), NativeFormatError> {
    if format_version != NATIVE_FORMAT_VERSION {
        return Err(NativeFormatError::UnsupportedVersion(format_version));
    }
    if cartridge_id.is_nil() || archive_id.is_nil() || archive_volume_id.is_nil() {
        return Err(NativeFormatError::Invalid(
            "cartridge_id, archive_id, and archive_volume_id must be non-nil".to_owned(),
        ));
    }
    if volume_count == 0 || volume_ordinal == 0 || volume_ordinal > volume_count {
        return Err(NativeFormatError::Invalid(
            "volume ordinal must be within the one-based volume set".to_owned(),
        ));
    }
    Ok(())
}

fn validate_v2_binding(
    format_version: u16,
    cartridge_id: Uuid,
    archive_id: Uuid,
    archive_volume_id: Uuid,
    volume_ordinal: u32,
    previous_archive_volume_id: Option<Uuid>,
    previous_volume_index_sha256: Option<&str>,
) -> Result<(), NativeFormatError> {
    if format_version != NATIVE_ROLLING_FORMAT_VERSION {
        return Err(NativeFormatError::UnsupportedVersion(format_version));
    }
    if cartridge_id.is_nil() || archive_id.is_nil() || archive_volume_id.is_nil() {
        return Err(NativeFormatError::Invalid(
            "cartridge_id, archive_id, and archive_volume_id must be non-nil".to_owned(),
        ));
    }
    if volume_ordinal == 0 {
        return Err(NativeFormatError::Invalid(
            "volume_ordinal must be one-based".to_owned(),
        ));
    }
    match (
        volume_ordinal,
        previous_archive_volume_id,
        previous_volume_index_sha256,
    ) {
        (1, None, None) => {}
        (1, _, _) => {
            return Err(NativeFormatError::Invalid(
                "the first volume cannot claim a previous volume".to_owned(),
            ));
        }
        (_, Some(previous_id), Some(previous_digest)) if !previous_id.is_nil() => {
            validate_sha256("previous_volume_index_sha256", previous_digest)?;
        }
        _ => {
            return Err(NativeFormatError::Invalid(
                "continuation volumes require both prior volume identity and index digest"
                    .to_owned(),
            ));
        }
    }
    Ok(())
}

fn validate_tape_block_bytes(value: u32) -> Result<(), NativeFormatError> {
    if !(64 * 1024..=4 * 1024 * 1024).contains(&value) || !value.is_power_of_two() {
        return Err(NativeFormatError::Invalid(
            "tape_block_bytes must be a power of two from 64 KiB through 4 MiB".to_owned(),
        ));
    }
    Ok(())
}

fn validate_sha256(field: &str, value: &str) -> Result<(), NativeFormatError> {
    if decode_hex_sha256(value).is_none() {
        return Err(NativeFormatError::Invalid(format!(
            "{field} must be 64 lowercase hexadecimal characters"
        )));
    }
    Ok(())
}

fn decode_hex_sha256(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    let mut decoded = [0_u8; 32];
    for (index, output) in decoded.iter_mut().enumerate() {
        let offset = index * 2;
        let high = hex_nibble(value.as_bytes()[offset])?;
        let low = hex_nibble(value.as_bytes()[offset + 1])?;
        *output = (high << 4) | low;
    }
    Some(decoded)
}

const fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

fn payload_sha256<T: Serialize>(value: &T) -> Result<String, NativeFormatError> {
    let payload = serde_json::to_vec(value)
        .map_err(|error| NativeFormatError::Serialization(error.to_string()))?;
    Ok(format!("{:x}", Sha256::digest(payload)))
}

/// SHA-256 over the raw bytes of a non-empty ordered sequence of canonical
/// SHA-256 digests. Archive writers and clean-host readers share this helper
/// so pack-catalog and volume-index roots cannot drift in representation.
pub fn ordered_sha256_digest<'a>(
    values: impl IntoIterator<Item = &'a str>,
) -> Result<String, NativeFormatError> {
    let mut digest = Sha256::new();
    let mut count = 0_u64;
    for value in values {
        digest.update(decode_hex_sha256(value).ok_or_else(|| {
            NativeFormatError::Invalid("ordered digest member is not canonical SHA-256".to_owned())
        })?);
        count = count
            .checked_add(1)
            .ok_or_else(|| NativeFormatError::Invalid("ordered digest count overflowed".into()))?;
    }
    if count == 0 {
        return Err(NativeFormatError::Invalid(
            "ordered digest requires at least one member".to_owned(),
        ));
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn encode_record<T: Serialize>(kind: RecordKind, value: &T) -> Result<Vec<u8>, NativeFormatError> {
    encode_record_for(NATIVE_RECORD_MAGIC, NATIVE_FORMAT_VERSION, kind, value)
}

fn encode_v2_record<T: Serialize>(
    kind: RecordKind,
    value: &T,
) -> Result<Vec<u8>, NativeFormatError> {
    encode_record_for(
        NATIVE_ROLLING_RECORD_MAGIC,
        NATIVE_ROLLING_FORMAT_VERSION,
        kind,
        value,
    )
}

fn encode_record_for<T: Serialize>(
    magic: [u8; 8],
    version: u16,
    kind: RecordKind,
    value: &T,
) -> Result<Vec<u8>, NativeFormatError> {
    let payload = serde_json::to_vec(value)
        .map_err(|error| NativeFormatError::Serialization(error.to_string()))?;
    if payload.len() > MAX_METADATA_RECORD_BYTES {
        return Err(NativeFormatError::Invalid(format!(
            "metadata payload exceeds {MAX_METADATA_RECORD_BYTES} bytes"
        )));
    }
    let payload_length = u32::try_from(payload.len()).map_err(|_| {
        NativeFormatError::Invalid("metadata payload length cannot be represented".to_owned())
    })?;
    let digest = Sha256::digest(&payload);
    let mut record = Vec::with_capacity(RECORD_PREFIX_BYTES + payload.len());
    record.extend_from_slice(&magic);
    record.extend_from_slice(&version.to_be_bytes());
    record.push(kind as u8);
    record.push(0); // reserved flags; non-zero values require a future format revision
    record.extend_from_slice(&payload_length.to_be_bytes());
    record.extend_from_slice(&digest);
    record.extend_from_slice(&payload);
    Ok(record)
}

fn decode_record<T: DeserializeOwned>(
    expected_kind: RecordKind,
    record: &[u8],
) -> Result<T, NativeFormatError> {
    decode_record_for(
        NATIVE_RECORD_MAGIC,
        NATIVE_FORMAT_VERSION,
        expected_kind,
        record,
    )
}

fn decode_v2_record<T: DeserializeOwned>(
    expected_kind: RecordKind,
    record: &[u8],
) -> Result<T, NativeFormatError> {
    decode_record_for(
        NATIVE_ROLLING_RECORD_MAGIC,
        NATIVE_ROLLING_FORMAT_VERSION,
        expected_kind,
        record,
    )
}

fn decode_record_for<T: DeserializeOwned>(
    magic: [u8; 8],
    expected_version: u16,
    expected_kind: RecordKind,
    record: &[u8],
) -> Result<T, NativeFormatError> {
    if record.len() < RECORD_PREFIX_BYTES {
        return Err(NativeFormatError::Invalid(
            "metadata record is shorter than its fixed prefix".to_owned(),
        ));
    }
    if record[..8] != magic {
        return Err(NativeFormatError::Invalid(
            "metadata record magic does not match the requested TapeScale native format".to_owned(),
        ));
    }
    let version = u16::from_be_bytes([record[8], record[9]]);
    if version != expected_version {
        return Err(NativeFormatError::UnsupportedVersion(version));
    }
    if record[10] != expected_kind as u8 {
        return Err(NativeFormatError::Invalid(
            "metadata record kind does not match the requested decoder".to_owned(),
        ));
    }
    if record[11] != 0 {
        return Err(NativeFormatError::Invalid(
            "metadata record contains unsupported flags".to_owned(),
        ));
    }
    let payload_length =
        u32::from_be_bytes([record[12], record[13], record[14], record[15]]) as usize;
    if payload_length > MAX_METADATA_RECORD_BYTES {
        return Err(NativeFormatError::Invalid(format!(
            "metadata payload exceeds {MAX_METADATA_RECORD_BYTES} bytes"
        )));
    }
    let expected_length = RECORD_PREFIX_BYTES
        .checked_add(payload_length)
        .ok_or_else(|| NativeFormatError::Invalid("metadata record length overflowed".into()))?;
    if record.len() != expected_length {
        return Err(NativeFormatError::Invalid(
            "metadata record length does not match its prefix".to_owned(),
        ));
    }
    let payload = &record[RECORD_PREFIX_BYTES..];
    let actual_digest = Sha256::digest(payload);
    if record[16..48] != actual_digest[..] {
        return Err(NativeFormatError::Integrity(
            "metadata payload SHA-256 does not match".to_owned(),
        ));
    }
    serde_json::from_slice(payload)
        .map_err(|error| NativeFormatError::Serialization(error.to_string()))
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn digest(byte: char) -> String {
        byte.to_string().repeat(64)
    }

    fn fixture_header() -> NativeVolumeHeader {
        NativeVolumeHeader {
            format_version: NATIVE_FORMAT_VERSION,
            cartridge_id: Uuid::parse_str("8b286d3c-46aa-46b2-80c8-131aeb35e43e").unwrap(),
            archive_id: Uuid::parse_str("7c67cabb-f13e-45e6-888d-24ca614af54e").unwrap(),
            archive_volume_id: Uuid::parse_str("b9c07652-32f6-4f7b-b8cf-53dbd848d3f2").unwrap(),
            volume_ordinal: 2,
            volume_count: 3,
            slice_count: 2,
            representation: NativeArchiveRepresentation::FileDar,
            compression: NativeCompressionCodec::Zstd,
            tape_block_bytes: DEFAULT_TAPE_BLOCK_BYTES,
            source_manifest_sha256: digest('a'),
            archive_catalog_sha256: digest('b'),
            created_at: Utc.with_ymd_and_hms(2026, 11, 19, 12, 0, 0).unwrap(),
        }
    }

    fn fixture_preparation_header() -> NativePreparationHeader {
        NativePreparationHeader {
            format_version: NATIVE_FORMAT_VERSION,
            cartridge_id: Uuid::parse_str("8b286d3c-46aa-46b2-80c8-131aeb35e43e").unwrap(),
            prepared_at: Utc.with_ymd_and_hms(2026, 11, 19, 11, 0, 0).unwrap(),
            qualification_sha256: Some(digest('e')),
            drive_identity_sha256: Some(digest('f')),
        }
    }

    fn fixture_index() -> NativeVolumeIndex {
        let header = fixture_header();
        NativeVolumeIndex {
            format_version: header.format_version,
            cartridge_id: header.cartridge_id,
            archive_id: header.archive_id,
            archive_volume_id: header.archive_volume_id,
            volume_ordinal: header.volume_ordinal,
            volume_count: header.volume_count,
            source_manifest_sha256: header.source_manifest_sha256,
            archive_catalog_sha256: header.archive_catalog_sha256,
            total_encoded_bytes: 300,
            total_logical_bytes: 700,
            slices: vec![
                NativeSliceDescriptor {
                    slice_ordinal: 5,
                    tape_file_number: 3,
                    archive_offset_bytes: 1_000,
                    encoded_bytes: 100,
                    logical_bytes: 300,
                    entry_count: 20,
                    sha256: digest('c'),
                },
                NativeSliceDescriptor {
                    slice_ordinal: 6,
                    tape_file_number: 4,
                    archive_offset_bytes: 1_100,
                    encoded_bytes: 200,
                    logical_bytes: 400,
                    entry_count: 30,
                    sha256: digest('d'),
                },
            ],
        }
    }

    fn fixture_v2_header() -> NativeV2VolumeHeader {
        NativeV2VolumeHeader {
            format_version: NATIVE_ROLLING_FORMAT_VERSION,
            cartridge_id: Uuid::parse_str("aaaaaaaa-46aa-46b2-80c8-131aeb35e43e").unwrap(),
            archive_id: Uuid::parse_str("bbbbbbbb-f13e-45e6-888d-24ca614af54e").unwrap(),
            archive_volume_id: Uuid::parse_str("cccccccc-32f6-4f7b-b8cf-53dbd848d3f2").unwrap(),
            volume_ordinal: 2,
            previous_archive_volume_id: Some(
                Uuid::parse_str("dddddddd-32f6-4f7b-b8cf-53dbd848d3f2").unwrap(),
            ),
            previous_volume_index_sha256: Some(digest('1')),
            declared_pack_count: Some(2),
            representation: NativeArchiveRepresentation::FileDar,
            compression: NativeCompressionCodec::Zstd,
            tape_block_bytes: DEFAULT_TAPE_BLOCK_BYTES,
            source_manifest_sha256: digest('2'),
            created_at: Utc.with_ymd_and_hms(2026, 11, 20, 12, 0, 0).unwrap(),
        }
    }

    fn fixture_v2_index() -> NativeV2VolumeIndex {
        let header = fixture_v2_header();
        NativeV2VolumeIndex {
            format_version: header.format_version,
            cartridge_id: header.cartridge_id,
            archive_id: header.archive_id,
            archive_volume_id: header.archive_volume_id,
            volume_ordinal: header.volume_ordinal,
            previous_archive_volume_id: header.previous_archive_volume_id,
            previous_volume_index_sha256: header.previous_volume_index_sha256,
            source_manifest_sha256: header.source_manifest_sha256,
            total_encoded_bytes: 300,
            total_logical_bytes: 700,
            total_entries: 15,
            packs: vec![
                NativePackDescriptor {
                    pack_id: Uuid::parse_str("eeeeeeee-32f6-4f7b-b8cf-53dbd848d3f2").unwrap(),
                    pack_ordinal: 3,
                    kind: NativePackKind::DarArchive,
                    first_tape_file_number: 2,
                    tape_file_count: 3,
                    archive_offset_bytes: 500,
                    encoded_bytes: 100,
                    logical_bytes: 300,
                    entry_count: 7,
                    content_sha256: digest('3'),
                    catalog_sha256: Some(digest('4')),
                },
                NativePackDescriptor {
                    pack_id: Uuid::parse_str("ffffffff-32f6-4f7b-b8cf-53dbd848d3f2").unwrap(),
                    pack_ordinal: 4,
                    kind: NativePackKind::DarArchive,
                    first_tape_file_number: 5,
                    tape_file_count: 3,
                    archive_offset_bytes: 600,
                    encoded_bytes: 200,
                    logical_bytes: 400,
                    entry_count: 8,
                    content_sha256: digest('5'),
                    catalog_sha256: Some(digest('6')),
                },
            ],
        }
    }

    fn fixture_v2_seal(index: &NativeV2VolumeIndex) -> NativeArchiveSetSeal {
        let first_volume_id = index.previous_archive_volume_id.unwrap();
        let first_index_sha256 = index.previous_volume_index_sha256.clone().unwrap();
        NativeArchiveSetSeal::new(
            index.archive_id,
            index.source_manifest_sha256.clone(),
            digest('7'),
            digest('8'),
            800,
            1_600,
            35,
            vec![
                NativeSealedVolume {
                    volume_ordinal: 1,
                    archive_volume_id: first_volume_id,
                    previous_archive_volume_id: None,
                    previous_volume_index_sha256: None,
                    index_sha256: first_index_sha256.clone(),
                    first_pack_ordinal: 1,
                    pack_count: 2,
                    encoded_bytes: 500,
                    logical_bytes: 900,
                    entry_count: 20,
                },
                NativeSealedVolume {
                    volume_ordinal: 2,
                    archive_volume_id: index.archive_volume_id,
                    previous_archive_volume_id: Some(first_volume_id),
                    previous_volume_index_sha256: Some(first_index_sha256),
                    index_sha256: index.payload_sha256().unwrap(),
                    first_pack_ordinal: 3,
                    pack_count: 2,
                    encoded_bytes: 300,
                    logical_bytes: 700,
                    entry_count: 15,
                },
            ],
            Utc.with_ymd_and_hms(2026, 11, 20, 14, 0, 0).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn header_roundtrip_is_deterministic_and_self_validating() {
        let header = fixture_header();
        let first = header.encode().expect("encode header");
        let second = header.encode().expect("encode header twice");
        assert_eq!(first, second);
        assert_eq!(&first[..8], &NATIVE_RECORD_MAGIC);
        assert_eq!(NativeVolumeHeader::decode(&first).unwrap(), header);
    }

    #[test]
    fn preparation_header_roundtrip_is_distinct_from_archive_header() {
        let preparation = fixture_preparation_header();
        let encoded = preparation.encode().unwrap();
        assert_eq!(
            NativePreparationHeader::decode(&encoded).unwrap(),
            preparation
        );
        assert!(matches!(
            NativeVolumeHeader::decode(&encoded),
            Err(NativeFormatError::Invalid(message)) if message.contains("kind")
        ));

        let archive = fixture_header().encode().unwrap();
        assert!(matches!(
            NativePreparationHeader::decode(&archive),
            Err(NativeFormatError::Invalid(message)) if message.contains("kind")
        ));
    }

    #[test]
    fn preparation_header_rejects_nil_identity_and_noncanonical_evidence() {
        let mut preparation = fixture_preparation_header();
        preparation.cartridge_id = Uuid::nil();
        assert!(preparation.validate().is_err());

        let mut preparation = fixture_preparation_header();
        preparation.qualification_sha256 = Some(digest('A'));
        assert!(preparation.validate().is_err());

        let mut preparation = fixture_preparation_header();
        preparation.drive_identity_sha256 = Some("missing".to_owned());
        assert!(preparation.validate().is_err());
    }

    #[test]
    fn payload_corruption_is_detected_before_json_is_trusted() {
        let mut encoded = fixture_header().encode().unwrap();
        let final_byte = encoded.last_mut().unwrap();
        *final_byte ^= 1;
        assert!(matches!(
            NativeVolumeHeader::decode(&encoded),
            Err(NativeFormatError::Integrity(_))
        ));
    }

    #[test]
    fn record_kind_cannot_be_confused() {
        let encoded = fixture_header().encode().unwrap();
        assert!(matches!(
            NativeVolumeIndex::decode(&encoded),
            Err(NativeFormatError::Invalid(message)) if message.contains("kind")
        ));
    }

    #[test]
    fn header_rejects_invalid_binding_block_size_and_digest() {
        let mut header = fixture_header();
        header.archive_volume_id = Uuid::nil();
        assert!(header.validate().is_err());
        header.archive_volume_id = Uuid::parse_str("b9c07652-32f6-4f7b-b8cf-53dbd848d3f2").unwrap();
        header.volume_ordinal = 4;
        assert!(header.validate().is_err());
        header.volume_ordinal = 2;
        header.tape_block_bytes = 96 * 1024;
        assert!(header.validate().is_err());
        header.tape_block_bytes = DEFAULT_TAPE_BLOCK_BYTES;
        header.source_manifest_sha256 = digest('A');
        assert!(header.validate().is_err());
    }

    #[test]
    fn index_roundtrip_requires_order_contiguity_and_exact_totals() {
        let index = fixture_index();
        let encoded = index.encode().unwrap();
        assert_eq!(NativeVolumeIndex::decode(&encoded).unwrap(), index);

        let mut invalid = fixture_index();
        invalid.slices[1].slice_ordinal = invalid.slices[0].slice_ordinal;
        assert!(invalid.validate().is_err());
        let mut invalid = fixture_index();
        invalid.slices[1].archive_offset_bytes += 1;
        assert!(invalid.validate().is_err());
        let mut invalid = fixture_index();
        invalid.total_encoded_bytes += 1;
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn trailer_commits_to_index_and_ordered_slice_hashes() {
        let index = fixture_index();
        let completed = Utc.with_ymd_and_hms(2026, 11, 19, 13, 0, 0).unwrap();
        let trailer = NativeVolumeTrailer::from_index(&index, completed).unwrap();
        trailer.verify_index(&index).unwrap();
        let encoded = trailer.encode().unwrap();
        let decoded = NativeVolumeTrailer::decode(&encoded).unwrap();
        decoded.verify_index(&index).unwrap();

        let mut altered = index;
        altered.slices[1].sha256 = digest('e');
        assert!(matches!(
            decoded.verify_index(&altered),
            Err(NativeFormatError::Integrity(_))
        ));

        let mut wrong_volume = fixture_index();
        wrong_volume.archive_volume_id =
            Uuid::parse_str("921392d7-cc16-48cb-95e7-59f089486606").unwrap();
        assert!(matches!(
            decoded.verify_index(&wrong_volume),
            Err(NativeFormatError::Integrity(_))
        ));
    }

    #[test]
    fn decoder_rejects_trailing_bytes_and_unknown_flags() {
        let mut trailing = fixture_header().encode().unwrap();
        trailing.push(0);
        assert!(NativeVolumeHeader::decode(&trailing).is_err());

        let mut flagged = fixture_header().encode().unwrap();
        flagged[11] = 1;
        assert!(NativeVolumeHeader::decode(&flagged).is_err());
    }

    #[test]
    fn v2_volume_chain_and_pack_ledger_roundtrip_without_changing_v1() {
        let header = fixture_v2_header();
        let header_bytes = header.encode().unwrap();
        assert_eq!(&header_bytes[..8], &NATIVE_ROLLING_RECORD_MAGIC);
        assert_eq!(NativeV2VolumeHeader::decode(&header_bytes).unwrap(), header);
        assert!(NativeVolumeHeader::decode(&header_bytes).is_err());

        let v1_bytes = fixture_header().encode().unwrap();
        assert_eq!(
            NativeVolumeHeader::decode(&v1_bytes).unwrap(),
            fixture_header()
        );
        assert!(NativeV2VolumeHeader::decode(&v1_bytes).is_err());

        let index = fixture_v2_index();
        index.verify_header(&header).unwrap();
        let pack_bytes = index.packs[0].encode().unwrap();
        assert_eq!(
            NativePackDescriptor::decode(&pack_bytes).unwrap(),
            index.packs[0]
        );
        assert!(NativeArchiveSetSeal::decode(&pack_bytes).is_err());
        let index_bytes = index.encode().unwrap();
        assert_eq!(NativeV2VolumeIndex::decode(&index_bytes).unwrap(), index);

        let trailer = NativeV2VolumeTrailer::from_index(
            &index,
            Utc.with_ymd_and_hms(2026, 11, 20, 13, 0, 0).unwrap(),
        )
        .unwrap();
        trailer.verify_index(&index).unwrap();
        assert!(trailer.archive_set_seal_sha256.is_none());
        assert_eq!(
            NativeV2VolumeTrailer::decode(&trailer.encode().unwrap()).unwrap(),
            trailer
        );
    }

    #[test]
    fn v2_open_header_and_chain_validation_fail_closed() {
        let mut open = fixture_v2_header();
        open.declared_pack_count = None;
        open.validate().unwrap();

        let mut first = fixture_v2_header();
        first.volume_ordinal = 1;
        assert!(first.validate().is_err());
        first.previous_archive_volume_id = None;
        first.previous_volume_index_sha256 = None;
        first.validate().unwrap();

        let mut continuation = fixture_v2_header();
        continuation.previous_volume_index_sha256 = None;
        assert!(continuation.validate().is_err());

        let mut invalid_pack = fixture_v2_index();
        invalid_pack.packs[1].first_tape_file_number += 1;
        assert!(invalid_pack.validate().is_err());
        let mut invalid_pack = fixture_v2_index();
        invalid_pack.packs[0].catalog_sha256 = None;
        assert!(invalid_pack.validate().is_err());
    }

    #[test]
    fn v2_final_seal_commits_exact_volume_chain_and_final_trailer() {
        let index = fixture_v2_index();
        let seal = fixture_v2_seal(&index);
        let encoded = seal.encode().unwrap();
        let decoded = NativeArchiveSetSeal::decode(&encoded).unwrap();
        assert_eq!(decoded, seal);

        let trailer = NativeV2VolumeTrailer::from_index_with_archive_set_seal(
            &index,
            &seal,
            Utc.with_ymd_and_hms(2026, 11, 20, 14, 1, 0).unwrap(),
        )
        .unwrap();
        trailer.verify_index(&index).unwrap();
        trailer.verify_archive_set_seal(&seal).unwrap();

        let mut altered = seal.clone();
        altered.volumes[0].index_sha256 = digest('9');
        assert!(altered.validate().is_err());
        let mut altered = seal;
        altered.total_encoded_bytes += 1;
        assert!(altered.validate().is_err());
    }
}
