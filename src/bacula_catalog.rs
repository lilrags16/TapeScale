//! Fail-closed evidence read directly from the pinned Bacula PostgreSQL catalog.

use std::collections::{HashMap, HashSet};

use serde::Serialize;
use sqlx::{FromRow, PgPool};

use crate::engine::bacula::{
    BaculaJobId, BaculaPoolName, VolumeLabel, BACKUP_JOB_NAME, DIRECTOR_NAME, MEDIA_TYPE,
    RESTORE_JOB_NAME, STORAGE_NAME, VERIFY_JOB_NAME,
};

// Bacula 15.0.3 persists these one-byte JCR codes in the catalog Job table.
// In particular, L_VERIFY_DATA is 'A'; 'D' is the unrelated differential
// backup level. Restore commands are promoted to L_FULL before cataloging.
const BACULA_BACKUP_JOB_TYPE: &str = "B";
const BACULA_VERIFY_JOB_TYPE: &str = "V";
const BACULA_RESTORE_JOB_TYPE: &str = "R";
const BACULA_FULL_LEVEL: &str = "F";
const BACULA_VERIFY_DATA_LEVEL: &str = "A";

#[derive(Debug, thiserror::Error)]
pub enum BaculaCatalogEvidenceError {
    #[error("Bacula catalog query failed: {0}")]
    Database(#[from] sqlx::Error),
    #[error("Bacula catalog contains no {kind} record for {identifier}")]
    Missing {
        kind: &'static str,
        identifier: String,
    },
    #[error("Bacula catalog contains {actual} {kind} records where exactly one was required")]
    Ambiguous { kind: &'static str, actual: usize },
    #[error("Bacula catalog {field} mismatch: expected {expected}, got {actual}")]
    Mismatch {
        field: &'static str,
        expected: String,
        actual: String,
    },
    #[error("Bacula catalog {field} contains an invalid negative or out-of-range value")]
    InvalidNumber { field: &'static str },
    #[error("Bacula backup JobId {job_id} has no ordered JobMedia segments")]
    MissingJobMedia { job_id: BaculaJobId },
    #[error("at least one authorized Bacula volume is required")]
    EmptyVolumeSet,
    #[error("authorized Bacula volume {0} was listed more than once")]
    DuplicateAuthorizedVolume(String),
    #[error("Bacula job used unauthorized volume {0}")]
    UnauthorizedJobVolume(String),
    #[error("Bacula JobMedia segment {segment_id} has invalid or reversed coordinates")]
    InvalidJobMediaCoordinates { segment_id: u64 },
    #[error(
        "Bacula restore JobId {restore_job_id} does not record source backup JobId {backup_job_id}"
    )]
    MissingRestoreRelationship {
        restore_job_id: BaculaJobId,
        backup_job_id: BaculaJobId,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BaculaCatalogJobEvidence {
    pub job_id: BaculaJobId,
    pub name: String,
    pub job_type: String,
    pub level: String,
    pub status: String,
    pub prior_job_id: u64,
    pub files: u64,
    pub job_bytes: u64,
    pub errors: u64,
    pub pool_name: Option<String>,
    pub read_storage_name: Option<String>,
    pub write_storage_name: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BaculaCatalogVolumeEvidence {
    pub media_id: u64,
    pub volume_name: String,
    pub pool_name: String,
    pub media_type: String,
    pub volume_status: String,
    pub enabled: bool,
    pub recycle: bool,
    pub volume_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BaculaJobMediaSegment {
    pub job_media_id: u64,
    pub media_id: u64,
    pub volume_name: String,
    pub pool_name: String,
    pub volume_index: u64,
    pub first_index: u64,
    pub last_index: u64,
    pub start_file: u64,
    pub start_block: u64,
    pub end_file: u64,
    pub end_block: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BaculaBackupCatalogEvidence {
    pub job: BaculaCatalogJobEvidence,
    /// The first physical volume, retained for compatibility with existing
    /// single-volume evidence consumers.
    pub volume: BaculaCatalogVolumeEvidence,
    /// Every physical volume actually used by the job, in Bacula volume-index
    /// order.  A spanning archive therefore remains explicit and auditable.
    pub volumes: Vec<BaculaCatalogVolumeEvidence>,
    pub segments: Vec<BaculaJobMediaSegment>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BaculaVerifyCatalogEvidence {
    pub job: BaculaCatalogJobEvidence,
    pub source_backup_job_id: BaculaJobId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BaculaRestoreCatalogEvidence {
    pub job: BaculaCatalogJobEvidence,
    pub source_backup_job_id: BaculaJobId,
}

#[derive(FromRow)]
struct CatalogJobRow {
    job_id: i32,
    name: String,
    job_type: String,
    level: String,
    job_status: String,
    prior_job_id: i32,
    job_files: i32,
    job_bytes: i64,
    job_errors: i32,
    pool_name: Option<String>,
    read_storage_name: Option<String>,
    write_storage_name: Option<String>,
}

#[derive(FromRow)]
struct CatalogPoolRow {
    pool_id: i32,
    name: String,
    max_volumes: i32,
    recycle: i16,
    auto_prune: i16,
}

#[derive(FromRow)]
struct CatalogVolumeRow {
    media_id: i32,
    volume_name: String,
    pool_name: String,
    media_type: String,
    volume_status: String,
    enabled: i16,
    recycle: i16,
    volume_bytes: i64,
}

#[derive(FromRow)]
struct CatalogJobMediaRow {
    job_media_id: i32,
    media_id: i32,
    volume_name: String,
    pool_name: String,
    volume_index: i32,
    first_index: i32,
    last_index: i32,
    start_file: i32,
    start_block: i64,
    end_file: i32,
    end_block: i64,
}

/// Prove the label exists in one otherwise-empty, non-recycling per-run pool.
pub async fn validate_labeled_volume(
    database: &PgPool,
    volume: &VolumeLabel,
    pool: &BaculaPoolName,
) -> Result<BaculaCatalogVolumeEvidence, BaculaCatalogEvidenceError> {
    let mut volumes =
        validate_labeled_volumes(database, std::slice::from_ref(volume), pool, 1).await?;
    Ok(volumes.remove(0))
}

/// Prove all labels exist in the isolated, non-recycling per-run pool.
/// `expected_max_volumes = 0` is Bacula's unlimited-spanning representation;
/// the exact authorized labels still bound which physical media may be used.
pub async fn validate_labeled_volumes(
    database: &PgPool,
    volumes: &[VolumeLabel],
    pool: &BaculaPoolName,
    expected_max_volumes: u32,
) -> Result<Vec<BaculaCatalogVolumeEvidence>, BaculaCatalogEvidenceError> {
    validate_authorized_volume_labels(volumes)?;
    let pools = sqlx::query_as::<_, CatalogPoolRow>(
        "SELECT poolid AS pool_id, name, maxvols AS max_volumes,
                recycle, autoprune AS auto_prune
         FROM pool WHERE name = $1",
    )
    .bind(pool.as_str())
    .fetch_all(database)
    .await?;
    require_exactly_one("pool", pools.len())?;
    let pool_row = &pools[0];
    expect("pool.name", pool.as_str(), &pool_row.name)?;
    let expected_max_volumes = i64::from(expected_max_volumes);
    expect_number(
        "pool.max_volumes",
        expected_max_volumes,
        i64::from(pool_row.max_volumes),
    )?;
    expect_number("pool.recycle", 0, i64::from(pool_row.recycle))?;
    expect_number("pool.auto_prune", 0, i64::from(pool_row.auto_prune))?;
    let _pool_id = nonnegative_i32("pool.id", pool_row.pool_id)?;

    let pool_volume_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM media m JOIN pool p ON p.poolid = m.poolid
         WHERE p.name = $1",
    )
    .bind(pool.as_str())
    .fetch_one(database)
    .await?;
    expect_number(
        "pool.volume_count",
        i64::try_from(volumes.len()).map_err(|_| BaculaCatalogEvidenceError::InvalidNumber {
            field: "pool.volume_count",
        })?,
        pool_volume_count,
    )?;

    load_authorized_volumes(database, volumes, pool).await
}

pub async fn validate_backup_job(
    database: &PgPool,
    job_id: BaculaJobId,
    volume: &VolumeLabel,
    pool: &BaculaPoolName,
) -> Result<BaculaBackupCatalogEvidence, BaculaCatalogEvidenceError> {
    validate_backup_job_on_volumes(database, job_id, std::slice::from_ref(volume), pool).await
}

/// Validate a completed backup whose JobMedia chain may span any of the exact
/// TapeScale-authorized labels.  Catalog media outside this set fail closed.
pub async fn validate_backup_job_on_volumes(
    database: &PgPool,
    job_id: BaculaJobId,
    authorized_volumes: &[VolumeLabel],
    pool: &BaculaPoolName,
) -> Result<BaculaBackupCatalogEvidence, BaculaCatalogEvidenceError> {
    validate_authorized_volume_labels(authorized_volumes)?;
    let job = load_job(database, job_id).await?;
    validate_backup_job_record(&job, pool)?;

    let catalog_volumes = load_authorized_volumes(database, authorized_volumes, pool).await?;
    let catalog_by_name = catalog_volumes
        .iter()
        .map(|volume| (volume.volume_name.clone(), volume.clone()))
        .collect::<HashMap<_, _>>();
    let media_by_name = catalog_volumes
        .iter()
        .map(|volume| (volume.volume_name.clone(), volume.media_id))
        .collect::<HashMap<_, _>>();
    let rows = sqlx::query_as::<_, CatalogJobMediaRow>(
        "SELECT jm.jobmediaid AS job_media_id, jm.mediaid AS media_id,
                m.volumename AS volume_name, p.name AS pool_name,
                jm.volindex AS volume_index, jm.firstindex AS first_index,
                jm.lastindex AS last_index, jm.startfile AS start_file,
                jm.startblock AS start_block, jm.endfile AS end_file,
                jm.endblock AS end_block
         FROM jobmedia jm
         JOIN media m ON m.mediaid = jm.mediaid
         JOIN pool p ON p.poolid = m.poolid
         WHERE jm.jobid = $1
         ORDER BY jm.volindex, jm.jobmediaid",
    )
    .bind(job_id_i32(job_id)?)
    .fetch_all(database)
    .await?;
    if rows.is_empty() {
        return Err(BaculaCatalogEvidenceError::MissingJobMedia { job_id });
    }
    let segments = validate_segments_for_volumes(rows, pool, &media_by_name)?;
    let mut seen = HashSet::new();
    let mut used_volumes = Vec::new();
    for segment in &segments {
        if seen.insert(segment.volume_name.clone()) {
            let volume = catalog_by_name.get(&segment.volume_name).ok_or_else(|| {
                BaculaCatalogEvidenceError::UnauthorizedJobVolume(segment.volume_name.clone())
            })?;
            used_volumes.push(volume.clone());
        }
    }
    let first_volume = used_volumes
        .first()
        .cloned()
        .ok_or(BaculaCatalogEvidenceError::MissingJobMedia { job_id })?;
    Ok(BaculaBackupCatalogEvidence {
        job,
        volume: first_volume,
        volumes: used_volumes,
        segments,
    })
}

pub async fn validate_verify_job(
    database: &PgPool,
    verify_job_id: BaculaJobId,
    backup_job_id: BaculaJobId,
) -> Result<BaculaVerifyCatalogEvidence, BaculaCatalogEvidenceError> {
    let job = load_job(database, verify_job_id).await?;
    validate_verify_job_record(&job, backup_job_id)?;
    Ok(BaculaVerifyCatalogEvidence {
        job,
        source_backup_job_id: backup_job_id,
    })
}

fn validate_verify_job_record(
    job: &BaculaCatalogJobEvidence,
    backup_job_id: BaculaJobId,
) -> Result<(), BaculaCatalogEvidenceError> {
    validate_job_common(
        job,
        VERIFY_JOB_NAME,
        BACULA_VERIFY_JOB_TYPE,
        BACULA_VERIFY_DATA_LEVEL,
    )?;
    expect_number(
        "verify.prior_job_id",
        i64::from(job_id_i32(backup_job_id)?),
        i64::try_from(job.prior_job_id).map_err(|_| BaculaCatalogEvidenceError::InvalidNumber {
            field: "verify.prior_job_id",
        })?,
    )?;
    expect_optional(
        "verify.read_storage",
        STORAGE_NAME,
        job.read_storage_name.as_deref(),
    )?;
    Ok(())
}

pub async fn validate_restore_job(
    database: &PgPool,
    restore_job_id: BaculaJobId,
    backup_job_id: BaculaJobId,
) -> Result<BaculaRestoreCatalogEvidence, BaculaCatalogEvidenceError> {
    let job = load_job(database, restore_job_id).await?;
    validate_restore_job_record(&job)?;
    let relationship = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
             SELECT 1 FROM log
             WHERE jobid = $1 AND btrim(logtext, E'\\r\\n') = $2)",
    )
    .bind(job_id_i32(restore_job_id)?)
    .bind(restore_relationship_log(restore_job_id, backup_job_id))
    .fetch_one(database)
    .await?;
    if !relationship {
        return Err(BaculaCatalogEvidenceError::MissingRestoreRelationship {
            restore_job_id,
            backup_job_id,
        });
    }
    Ok(BaculaRestoreCatalogEvidence {
        job,
        source_backup_job_id: backup_job_id,
    })
}

fn validate_backup_job_record(
    job: &BaculaCatalogJobEvidence,
    pool: &BaculaPoolName,
) -> Result<(), BaculaCatalogEvidenceError> {
    validate_job_common(
        job,
        BACKUP_JOB_NAME,
        BACULA_BACKUP_JOB_TYPE,
        BACULA_FULL_LEVEL,
    )?;
    expect_optional("backup.pool", pool.as_str(), job.pool_name.as_deref())?;
    expect_optional(
        "backup.write_storage",
        STORAGE_NAME,
        job.write_storage_name.as_deref(),
    )
}

fn validate_restore_job_record(
    job: &BaculaCatalogJobEvidence,
) -> Result<(), BaculaCatalogEvidenceError> {
    validate_job_common(
        job,
        RESTORE_JOB_NAME,
        BACULA_RESTORE_JOB_TYPE,
        BACULA_FULL_LEVEL,
    )?;
    expect_optional(
        "restore.read_storage",
        STORAGE_NAME,
        job.read_storage_name.as_deref(),
    )
}

fn restore_relationship_log(restore_job_id: BaculaJobId, backup_job_id: BaculaJobId) -> String {
    format!(
        "{DIRECTOR_NAME} JobId {}: Restoring files from JobId(s) {}",
        restore_job_id.get(),
        backup_job_id.get()
    )
}

fn validate_authorized_volume_labels(
    volumes: &[VolumeLabel],
) -> Result<(), BaculaCatalogEvidenceError> {
    if volumes.is_empty() {
        return Err(BaculaCatalogEvidenceError::EmptyVolumeSet);
    }
    let mut seen = HashSet::new();
    for volume in volumes {
        if !seen.insert(volume.as_str()) {
            return Err(BaculaCatalogEvidenceError::DuplicateAuthorizedVolume(
                volume.as_str().to_owned(),
            ));
        }
    }
    Ok(())
}

async fn load_authorized_volumes(
    database: &PgPool,
    volumes: &[VolumeLabel],
    pool: &BaculaPoolName,
) -> Result<Vec<BaculaCatalogVolumeEvidence>, BaculaCatalogEvidenceError> {
    validate_authorized_volume_labels(volumes)?;
    let names = volumes
        .iter()
        .map(|volume| volume.as_str().to_owned())
        .collect::<Vec<_>>();
    let rows = sqlx::query_as::<_, CatalogVolumeRow>(
        "SELECT m.mediaid AS media_id, m.volumename AS volume_name,
                p.name AS pool_name, m.mediatype AS media_type,
                m.volstatus AS volume_status, m.enabled, m.recycle,
                m.volbytes AS volume_bytes
         FROM media m JOIN pool p ON p.poolid = m.poolid
         WHERE m.volumename = ANY($1::text[])",
    )
    .bind(&names)
    .fetch_all(database)
    .await?;
    if rows.len() != volumes.len() {
        return Err(BaculaCatalogEvidenceError::Mismatch {
            field: "authorized_volume.count",
            expected: volumes.len().to_string(),
            actual: rows.len().to_string(),
        });
    }
    let mut rows_by_name = rows
        .into_iter()
        .map(|row| (row.volume_name.clone(), row))
        .collect::<HashMap<_, _>>();
    let mut evidence = Vec::with_capacity(volumes.len());
    for volume in volumes {
        let row = rows_by_name.remove(volume.as_str()).ok_or_else(|| {
            BaculaCatalogEvidenceError::Missing {
                kind: "volume",
                identifier: volume.as_str().to_owned(),
            }
        })?;
        evidence.push(validated_volume(&row, volume, pool)?);
    }
    Ok(evidence)
}

async fn load_job(
    database: &PgPool,
    job_id: BaculaJobId,
) -> Result<BaculaCatalogJobEvidence, BaculaCatalogEvidenceError> {
    let row = sqlx::query_as::<_, CatalogJobRow>(
        "SELECT j.jobid AS job_id, j.name, j.type::text AS job_type,
                j.level::text AS level, j.jobstatus::text AS job_status,
                j.priorjobid AS prior_job_id, j.jobfiles AS job_files,
                j.jobbytes AS job_bytes, j.joberrors AS job_errors,
                p.name AS pool_name, rs.name AS read_storage_name,
                ws.name AS write_storage_name
         FROM job j
         LEFT JOIN pool p ON p.poolid = j.poolid
         LEFT JOIN storage rs ON rs.storageid = j.lastreadstorageid
         LEFT JOIN storage ws ON ws.storageid = j.writestorageid
         WHERE j.jobid = $1",
    )
    .bind(job_id_i32(job_id)?)
    .fetch_optional(database)
    .await?
    .ok_or_else(|| BaculaCatalogEvidenceError::Missing {
        kind: "job",
        identifier: job_id.to_string(),
    })?;
    Ok(BaculaCatalogJobEvidence {
        job_id: BaculaJobId::new(nonnegative_i32("job.id", row.job_id)?)
            .map_err(|_| BaculaCatalogEvidenceError::InvalidNumber { field: "job.id" })?,
        name: row.name,
        job_type: row.job_type,
        level: row.level,
        status: row.job_status,
        prior_job_id: nonnegative_i32("job.prior_job_id", row.prior_job_id)?,
        files: nonnegative_i32("job.files", row.job_files)?,
        job_bytes: nonnegative_i64("job.bytes", row.job_bytes)?,
        errors: nonnegative_i32("job.errors", row.job_errors)?,
        pool_name: row.pool_name,
        read_storage_name: row.read_storage_name,
        write_storage_name: row.write_storage_name,
    })
}

fn validate_job_common(
    job: &BaculaCatalogJobEvidence,
    name: &str,
    job_type: &str,
    level: &str,
) -> Result<(), BaculaCatalogEvidenceError> {
    expect("job.name", name, &job.name)?;
    expect("job.type", job_type, &job.job_type)?;
    expect("job.level", level, &job.level)?;
    expect("job.status", "T", &job.status)?;
    expect_number(
        "job.errors",
        0,
        i64::try_from(job.errors).map_err(|_| BaculaCatalogEvidenceError::InvalidNumber {
            field: "job.errors",
        })?,
    )
}

fn validated_volume(
    row: &CatalogVolumeRow,
    volume: &VolumeLabel,
    pool: &BaculaPoolName,
) -> Result<BaculaCatalogVolumeEvidence, BaculaCatalogEvidenceError> {
    expect("volume.name", volume.as_str(), &row.volume_name)?;
    expect("volume.pool", pool.as_str(), &row.pool_name)?;
    expect("volume.media_type", MEDIA_TYPE, &row.media_type)?;
    expect("volume.enabled", "1", &row.enabled.to_string())?;
    expect("volume.recycle", "0", &row.recycle.to_string())?;
    if !matches!(row.volume_status.as_str(), "Append" | "Used" | "Full") {
        return Err(BaculaCatalogEvidenceError::Mismatch {
            field: "volume.status",
            expected: "Append, Used, or Full".to_owned(),
            actual: row.volume_status.clone(),
        });
    }
    Ok(BaculaCatalogVolumeEvidence {
        media_id: nonnegative_i32("volume.media_id", row.media_id)?,
        volume_name: row.volume_name.clone(),
        pool_name: row.pool_name.clone(),
        media_type: row.media_type.clone(),
        volume_status: row.volume_status.clone(),
        enabled: row.enabled == 1,
        recycle: row.recycle != 0,
        volume_bytes: nonnegative_i64("volume.bytes", row.volume_bytes)?,
    })
}

#[cfg(test)]
fn validate_segments(
    rows: Vec<CatalogJobMediaRow>,
    volume: &VolumeLabel,
    pool: &BaculaPoolName,
    media_id: u64,
) -> Result<Vec<BaculaJobMediaSegment>, BaculaCatalogEvidenceError> {
    validate_segments_for_volumes(
        rows,
        pool,
        &HashMap::from([(volume.as_str().to_owned(), media_id)]),
    )
}

fn validate_segments_for_volumes(
    rows: Vec<CatalogJobMediaRow>,
    pool: &BaculaPoolName,
    authorized_media: &HashMap<String, u64>,
) -> Result<Vec<BaculaJobMediaSegment>, BaculaCatalogEvidenceError> {
    let mut previous_order = None;
    rows.into_iter()
        .map(|row| {
            let expected_media_id = authorized_media.get(&row.volume_name).ok_or_else(|| {
                BaculaCatalogEvidenceError::UnauthorizedJobVolume(row.volume_name.clone())
            })?;
            expect("job_media.pool", pool.as_str(), &row.pool_name)?;
            let segment = BaculaJobMediaSegment {
                job_media_id: nonnegative_i32("job_media.id", row.job_media_id)?,
                media_id: nonnegative_i32("job_media.media_id", row.media_id)?,
                volume_name: row.volume_name,
                pool_name: row.pool_name,
                volume_index: nonnegative_i32("job_media.volume_index", row.volume_index)?,
                first_index: nonnegative_i32("job_media.first_index", row.first_index)?,
                last_index: nonnegative_i32("job_media.last_index", row.last_index)?,
                start_file: nonnegative_i32("job_media.start_file", row.start_file)?,
                start_block: nonnegative_i64("job_media.start_block", row.start_block)?,
                end_file: nonnegative_i32("job_media.end_file", row.end_file)?,
                end_block: nonnegative_i64("job_media.end_block", row.end_block)?,
            };
            if segment.media_id != *expected_media_id
                || segment.volume_index == 0
                || segment.first_index > segment.last_index
                || (segment.start_file, segment.start_block) > (segment.end_file, segment.end_block)
            {
                return Err(BaculaCatalogEvidenceError::InvalidJobMediaCoordinates {
                    segment_id: segment.job_media_id,
                });
            }
            let order = (segment.volume_index, segment.job_media_id);
            if previous_order.is_some_and(|previous| previous > order) {
                return Err(BaculaCatalogEvidenceError::InvalidJobMediaCoordinates {
                    segment_id: segment.job_media_id,
                });
            }
            previous_order = Some(order);
            Ok(segment)
        })
        .collect()
}

fn require_exactly_one(kind: &'static str, count: usize) -> Result<(), BaculaCatalogEvidenceError> {
    match count {
        1 => Ok(()),
        0 => Err(BaculaCatalogEvidenceError::Missing {
            kind,
            identifier: "expected exact record".to_owned(),
        }),
        actual => Err(BaculaCatalogEvidenceError::Ambiguous { kind, actual }),
    }
}

fn expect(
    field: &'static str,
    expected: &str,
    actual: &str,
) -> Result<(), BaculaCatalogEvidenceError> {
    if actual == expected {
        Ok(())
    } else {
        Err(BaculaCatalogEvidenceError::Mismatch {
            field,
            expected: expected.to_owned(),
            actual: actual.to_owned(),
        })
    }
}

fn expect_optional(
    field: &'static str,
    expected: &str,
    actual: Option<&str>,
) -> Result<(), BaculaCatalogEvidenceError> {
    expect(field, expected, actual.unwrap_or("<missing>"))
}

fn expect_number(
    field: &'static str,
    expected: i64,
    actual: i64,
) -> Result<(), BaculaCatalogEvidenceError> {
    expect(field, &expected.to_string(), &actual.to_string())
}

fn nonnegative_i32(field: &'static str, value: i32) -> Result<u64, BaculaCatalogEvidenceError> {
    u64::try_from(value).map_err(|_| BaculaCatalogEvidenceError::InvalidNumber { field })
}

fn nonnegative_i64(field: &'static str, value: i64) -> Result<u64, BaculaCatalogEvidenceError> {
    u64::try_from(value).map_err(|_| BaculaCatalogEvidenceError::InvalidNumber { field })
}

fn job_id_i32(job_id: BaculaJobId) -> Result<i32, BaculaCatalogEvidenceError> {
    i32::try_from(job_id.get())
        .map_err(|_| BaculaCatalogEvidenceError::InvalidNumber { field: "job.id" })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool() -> BaculaPoolName {
        BaculaPoolName::for_run(
            uuid::Uuid::parse_str("018f7cac-2e2a-7dc5-b70c-78a46a6f6f01").expect("uuid"),
        )
    }

    #[test]
    fn validates_one_volume_ordered_tape_coordinates() {
        let volume = VolumeLabel::parse("SCRATCH-0001").expect("volume");
        let rows = vec![CatalogJobMediaRow {
            job_media_id: 8,
            media_id: 7,
            volume_name: volume.as_str().to_owned(),
            pool_name: pool().as_str().to_owned(),
            volume_index: 1,
            first_index: 1,
            last_index: 42,
            start_file: 0,
            start_block: 0,
            end_file: 1,
            end_block: 2048,
        }];
        let segments = validate_segments(rows, &volume, &pool(), 7).expect("segments");
        assert_eq!(segments[0].end_block, 2048);
    }

    #[test]
    fn validates_ordered_segments_across_authorized_volumes() {
        let first = VolumeLabel::parse("ARCHIVE-0001").expect("volume");
        let second = VolumeLabel::parse("ARCHIVE-0002").expect("volume");
        let rows = vec![
            CatalogJobMediaRow {
                job_media_id: 8,
                media_id: 7,
                volume_name: first.as_str().to_owned(),
                pool_name: pool().as_str().to_owned(),
                volume_index: 1,
                first_index: 1,
                last_index: 42,
                start_file: 0,
                start_block: 0,
                end_file: 1,
                end_block: 2048,
            },
            CatalogJobMediaRow {
                job_media_id: 9,
                media_id: 11,
                volume_name: second.as_str().to_owned(),
                pool_name: pool().as_str().to_owned(),
                volume_index: 2,
                first_index: 43,
                last_index: 84,
                start_file: 0,
                start_block: 0,
                end_file: 1,
                end_block: 4096,
            },
        ];
        let segments = validate_segments_for_volumes(
            rows,
            &pool(),
            &HashMap::from([
                (first.as_str().to_owned(), 7),
                (second.as_str().to_owned(), 11),
            ]),
        )
        .expect("segments");
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].volume_index, 1);
        assert_eq!(segments[1].volume_index, 2);
    }

    #[test]
    fn rejects_duplicate_or_unauthorized_archive_volumes() {
        let volume = VolumeLabel::parse("ARCHIVE-0001").expect("volume");
        assert!(matches!(
            validate_authorized_volume_labels(&[volume.clone(), volume.clone()]),
            Err(BaculaCatalogEvidenceError::DuplicateAuthorizedVolume(_))
        ));
        let row = CatalogJobMediaRow {
            job_media_id: 8,
            media_id: 7,
            volume_name: "FOREIGN-0001".to_owned(),
            pool_name: pool().as_str().to_owned(),
            volume_index: 1,
            first_index: 1,
            last_index: 42,
            start_file: 0,
            start_block: 0,
            end_file: 1,
            end_block: 2048,
        };
        assert!(matches!(
            validate_segments_for_volumes(
                vec![row],
                &pool(),
                &HashMap::from([(volume.as_str().to_owned(), 7)])
            ),
            Err(BaculaCatalogEvidenceError::UnauthorizedJobVolume(_))
        ));
    }

    #[test]
    fn rejects_wrong_media_and_reversed_coordinates() {
        let volume = VolumeLabel::parse("SCRATCH-0001").expect("volume");
        let row = CatalogJobMediaRow {
            job_media_id: 8,
            media_id: 9,
            volume_name: volume.as_str().to_owned(),
            pool_name: pool().as_str().to_owned(),
            volume_index: 1,
            first_index: 42,
            last_index: 1,
            start_file: 2,
            start_block: 0,
            end_file: 1,
            end_block: 0,
        };
        assert!(matches!(
            validate_segments(vec![row], &volume, &pool(), 7),
            Err(BaculaCatalogEvidenceError::InvalidJobMediaCoordinates { .. })
        ));
    }

    #[test]
    fn backup_validation_requires_terminal_job_and_exact_pool() {
        let mut job = BaculaCatalogJobEvidence {
            job_id: BaculaJobId::new(10).expect("id"),
            name: BACKUP_JOB_NAME.to_owned(),
            job_type: BACULA_BACKUP_JOB_TYPE.to_owned(),
            level: BACULA_FULL_LEVEL.to_owned(),
            status: "R".to_owned(),
            prior_job_id: 0,
            files: 1,
            job_bytes: 10,
            errors: 0,
            pool_name: Some("wrong-pool".to_owned()),
            read_storage_name: None,
            write_storage_name: Some(STORAGE_NAME.to_owned()),
        };
        assert!(validate_backup_job_record(&job, &pool()).is_err());

        job.status = "T".to_owned();
        assert!(matches!(
            validate_backup_job_record(&job, &pool()),
            Err(BaculaCatalogEvidenceError::Mismatch {
                field: "backup.pool",
                ..
            })
        ));
    }

    #[test]
    fn accepts_bacula_data_verify_catalog_code_and_rejects_differential_code() {
        let backup_job_id = BaculaJobId::new(41).expect("backup id");
        let mut job = BaculaCatalogJobEvidence {
            job_id: BaculaJobId::new(42).expect("verify id"),
            name: VERIFY_JOB_NAME.to_owned(),
            job_type: BACULA_VERIFY_JOB_TYPE.to_owned(),
            level: BACULA_VERIFY_DATA_LEVEL.to_owned(),
            status: "T".to_owned(),
            prior_job_id: backup_job_id.get(),
            files: 746,
            job_bytes: 1_064_353_177,
            errors: 0,
            // Bacula does not assign PoolId to Verify catalog records.
            pool_name: None,
            read_storage_name: Some(STORAGE_NAME.to_owned()),
            write_storage_name: None,
        };

        validate_verify_job_record(&job, backup_job_id)
            .expect("Bacula Level=Data catalogs Job.Level as A");

        job.level = "D".to_owned();
        assert!(matches!(
            validate_verify_job_record(&job, backup_job_id),
            Err(BaculaCatalogEvidenceError::Mismatch {
                field: "job.level",
                expected,
                actual,
            }) if expected == "A" && actual == "D"
        ));

        job.level = BACULA_VERIFY_DATA_LEVEL.to_owned();
        job.prior_job_id = 99;
        assert!(matches!(
            validate_verify_job_record(&job, backup_job_id),
            Err(BaculaCatalogEvidenceError::Mismatch {
                field: "verify.prior_job_id",
                ..
            })
        ));

        job.prior_job_id = backup_job_id.get();
        job.read_storage_name = None;
        assert!(matches!(
            validate_verify_job_record(&job, backup_job_id),
            Err(BaculaCatalogEvidenceError::Mismatch {
                field: "verify.read_storage",
                ..
            })
        ));
    }

    #[test]
    fn accepts_completed_restore_without_a_catalog_pool() {
        let mut job = BaculaCatalogJobEvidence {
            job_id: BaculaJobId::new(43).expect("restore id"),
            name: RESTORE_JOB_NAME.to_owned(),
            job_type: BACULA_RESTORE_JOB_TYPE.to_owned(),
            level: BACULA_FULL_LEVEL.to_owned(),
            status: "T".to_owned(),
            prior_job_id: 0,
            files: 746,
            job_bytes: 1_064_353_177,
            errors: 0,
            // Bacula does not assign PoolId to Restore catalog records.
            pool_name: None,
            read_storage_name: Some(STORAGE_NAME.to_owned()),
            write_storage_name: None,
        };

        validate_restore_job_record(&job)
            .expect("a successful Bacula restore legitimately has no PoolId");

        job.read_storage_name = None;
        assert!(matches!(
            validate_restore_job_record(&job),
            Err(BaculaCatalogEvidenceError::Mismatch {
                field: "restore.read_storage",
                ..
            })
        ));

        assert_eq!(
            restore_relationship_log(
                BaculaJobId::new(43).expect("restore id"),
                BaculaJobId::new(41).expect("backup id")
            ),
            "tapescale-dir JobId 43: Restoring files from JobId(s) 41"
        );
    }

    #[test]
    fn volume_validation_requires_the_pinned_lto_media_type() {
        let volume = VolumeLabel::parse("SCRATCH-0001").expect("volume");
        let mut row = CatalogVolumeRow {
            media_id: 7,
            volume_name: volume.as_str().to_owned(),
            pool_name: pool().as_str().to_owned(),
            media_type: MEDIA_TYPE.to_owned(),
            volume_status: "Append".to_owned(),
            enabled: 1,
            recycle: 0,
            volume_bytes: 4096,
        };
        validated_volume(&row, &volume, &pool()).expect("pinned media type");

        row.media_type = "File".to_owned();
        assert!(matches!(
            validated_volume(&row, &volume, &pool()),
            Err(BaculaCatalogEvidenceError::Mismatch {
                field: "volume.media_type",
                ..
            })
        ));
    }
}
