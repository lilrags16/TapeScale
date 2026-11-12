//! LTO drive/media compatibility.
//!
//! The matrix is explicit because the generational pattern changes at LTO-8
//! and again at LTO-10.  Compatibility is a capability check only; it never
//! grants permission to overwrite a cartridge.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DriveGeneration {
    Lto1,
    Lto2,
    Lto3,
    Lto4,
    Lto5,
    Lto6,
    Lto7,
    Lto8,
    Lto9,
    Lto10,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MediaFormat {
    Lto1,
    Lto2,
    Lto3,
    Lto4,
    Lto5,
    Lto6,
    Lto7,
    /// An LTO-7 cartridge formatted as M8 media by a qualified LTO-8 drive.
    Lto7TypeM,
    Lto8,
    Lto9,
    Lto10,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessMode {
    Read,
    Write,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompatibilityDecision {
    pub compatible: bool,
    pub drive: DriveGeneration,
    pub media: MediaFormat,
    pub access: AccessMode,
}

pub fn can_read(drive: DriveGeneration, media: MediaFormat) -> bool {
    use DriveGeneration as D;
    use MediaFormat as M;

    match drive {
        D::Lto1 => matches!(media, M::Lto1),
        D::Lto2 => matches!(media, M::Lto1 | M::Lto2),
        D::Lto3 => matches!(media, M::Lto1 | M::Lto2 | M::Lto3),
        D::Lto4 => matches!(media, M::Lto2 | M::Lto3 | M::Lto4),
        D::Lto5 => matches!(media, M::Lto3 | M::Lto4 | M::Lto5),
        D::Lto6 => matches!(media, M::Lto4 | M::Lto5 | M::Lto6),
        D::Lto7 => matches!(media, M::Lto5 | M::Lto6 | M::Lto7),
        D::Lto8 => matches!(media, M::Lto7 | M::Lto7TypeM | M::Lto8),
        D::Lto9 => matches!(media, M::Lto8 | M::Lto9),
        D::Lto10 => matches!(media, M::Lto10),
    }
}

pub fn can_write(drive: DriveGeneration, media: MediaFormat) -> bool {
    use DriveGeneration as D;
    use MediaFormat as M;

    match drive {
        D::Lto1 => matches!(media, M::Lto1),
        D::Lto2 => matches!(media, M::Lto1 | M::Lto2),
        D::Lto3 => matches!(media, M::Lto2 | M::Lto3),
        D::Lto4 => matches!(media, M::Lto3 | M::Lto4),
        D::Lto5 => matches!(media, M::Lto4 | M::Lto5),
        D::Lto6 => matches!(media, M::Lto5 | M::Lto6),
        D::Lto7 => matches!(media, M::Lto6 | M::Lto7),
        D::Lto8 => matches!(media, M::Lto7 | M::Lto7TypeM | M::Lto8),
        D::Lto9 => matches!(media, M::Lto8 | M::Lto9),
        D::Lto10 => matches!(media, M::Lto10),
    }
}

pub fn compatibility(
    drive: DriveGeneration,
    media: MediaFormat,
    access: AccessMode,
) -> CompatibilityDecision {
    let compatible = match access {
        AccessMode::Read => can_read(drive, media),
        AccessMode::Write => can_write(drive, media),
    };

    CompatibilityDecision {
        compatible,
        drive,
        media,
        access,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DRIVES: [DriveGeneration; 10] = [
        DriveGeneration::Lto1,
        DriveGeneration::Lto2,
        DriveGeneration::Lto3,
        DriveGeneration::Lto4,
        DriveGeneration::Lto5,
        DriveGeneration::Lto6,
        DriveGeneration::Lto7,
        DriveGeneration::Lto8,
        DriveGeneration::Lto9,
        DriveGeneration::Lto10,
    ];

    const MEDIA: [MediaFormat; 11] = [
        MediaFormat::Lto1,
        MediaFormat::Lto2,
        MediaFormat::Lto3,
        MediaFormat::Lto4,
        MediaFormat::Lto5,
        MediaFormat::Lto6,
        MediaFormat::Lto7,
        MediaFormat::Lto7TypeM,
        MediaFormat::Lto8,
        MediaFormat::Lto9,
        MediaFormat::Lto10,
    ];

    #[test]
    fn exhaustive_read_matrix_matches_the_product_contract() {
        let expected = [
            vec![MediaFormat::Lto1],
            vec![MediaFormat::Lto1, MediaFormat::Lto2],
            vec![MediaFormat::Lto1, MediaFormat::Lto2, MediaFormat::Lto3],
            vec![MediaFormat::Lto2, MediaFormat::Lto3, MediaFormat::Lto4],
            vec![MediaFormat::Lto3, MediaFormat::Lto4, MediaFormat::Lto5],
            vec![MediaFormat::Lto4, MediaFormat::Lto5, MediaFormat::Lto6],
            vec![MediaFormat::Lto5, MediaFormat::Lto6, MediaFormat::Lto7],
            vec![MediaFormat::Lto7, MediaFormat::Lto7TypeM, MediaFormat::Lto8],
            vec![MediaFormat::Lto8, MediaFormat::Lto9],
            vec![MediaFormat::Lto10],
        ];

        for (index, drive) in DRIVES.into_iter().enumerate() {
            for media in MEDIA {
                assert_eq!(
                    can_read(drive, media),
                    expected[index].contains(&media),
                    "unexpected read result for {drive:?} and {media:?}"
                );
            }
        }
    }

    #[test]
    fn exhaustive_write_matrix_matches_the_product_contract() {
        let expected = [
            vec![MediaFormat::Lto1],
            vec![MediaFormat::Lto1, MediaFormat::Lto2],
            vec![MediaFormat::Lto2, MediaFormat::Lto3],
            vec![MediaFormat::Lto3, MediaFormat::Lto4],
            vec![MediaFormat::Lto4, MediaFormat::Lto5],
            vec![MediaFormat::Lto5, MediaFormat::Lto6],
            vec![MediaFormat::Lto6, MediaFormat::Lto7],
            vec![MediaFormat::Lto7, MediaFormat::Lto7TypeM, MediaFormat::Lto8],
            vec![MediaFormat::Lto8, MediaFormat::Lto9],
            vec![MediaFormat::Lto10],
        ];

        for (index, drive) in DRIVES.into_iter().enumerate() {
            for media in MEDIA {
                assert_eq!(
                    can_write(drive, media),
                    expected[index].contains(&media),
                    "unexpected write result for {drive:?} and {media:?}"
                );
            }
        }
    }

    #[test]
    fn generation_discontinuities_are_not_inferred_arithmetically() {
        assert!(!can_read(DriveGeneration::Lto9, MediaFormat::Lto7));
        assert!(!can_read(DriveGeneration::Lto9, MediaFormat::Lto7TypeM));
        assert!(!can_read(DriveGeneration::Lto10, MediaFormat::Lto9));
        assert!(!can_write(DriveGeneration::Lto10, MediaFormat::Lto9));
        assert!(can_write(DriveGeneration::Lto8, MediaFormat::Lto7TypeM));
    }
}
