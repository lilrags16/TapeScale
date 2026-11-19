-- Retain Bacula-era volume rows as immutable audit evidence without allowing
-- media that TapeScale cannot recover to appear protected or unreconciled.
--
-- This is intentionally separate from 0019.  The first native alpha shipped
-- 0019, so changing that migration would break SQLx checksum verification for
-- any installation that applied it directly from the tagged source.

ALTER TABLE volume_records
    ADD COLUMN retired_at timestamptz,
    ADD COLUMN retirement_reason text
        CHECK (retirement_reason IS NULL OR
               (retirement_reason <> '' AND octet_length(retirement_reason) <= 512
                AND retirement_reason !~ '[[:cntrl:]]'));

UPDATE volume_records volume
   SET retired_at = now(),
       retirement_reason =
           'Bacula media declared non-recoverable during TapeScale Native v1 migration'
  FROM recovery_points recovery
 WHERE recovery.id = volume.recovery_point_id
   AND recovery.archive_codec = 'legacy_bacula';

ALTER TABLE volume_records
    ADD CONSTRAINT volume_records_retirement_check
        CHECK ((retired_at IS NULL) = (retirement_reason IS NULL));
