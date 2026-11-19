//! Versioned, self-describing records for the TapeScale native tape format.
//!
//! These records are deliberately independent of a database or backup engine. A
//! replacement-host recovery tool can read the volume header, index, and trailer
//! directly from tape and verify every byte before trusting any media metadata.

use chrono::{DateTime, Utc};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Binary envelope magic for every TapeScale native metadata record.
pub const NATIVE_RECORD_MAGIC: [u8; 8] = *b"TSNV1REC";
/// Current on-tape metadata format revision.
pub const NATIVE_FORMAT_VERSION: u16 = 1;
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

fn encode_record<T: Serialize>(kind: RecordKind, value: &T) -> Result<Vec<u8>, NativeFormatError> {
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
    record.extend_from_slice(&NATIVE_RECORD_MAGIC);
    record.extend_from_slice(&NATIVE_FORMAT_VERSION.to_be_bytes());
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
    if record.len() < RECORD_PREFIX_BYTES {
        return Err(NativeFormatError::Invalid(
            "metadata record is shorter than its fixed prefix".to_owned(),
        ));
    }
    if record[..8] != NATIVE_RECORD_MAGIC {
        return Err(NativeFormatError::Invalid(
            "metadata record magic does not match TapeScale native v1".to_owned(),
        ));
    }
    let version = u16::from_be_bytes([record[8], record[9]]);
    if version != NATIVE_FORMAT_VERSION {
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
}
