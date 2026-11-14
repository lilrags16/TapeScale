-- Make continuous source-to-tape delivery the normal operational path.
--
-- Bacula data spooling is a disk-backed fill/drain stage, not a continuously
-- consumed ring buffer.  It doubles local I/O and pauses the File Daemon while
-- a completed segment is drained to tape.  Existing in-flight data runs retain
-- disk-spool semantics; policies (including existing ones) use direct delivery
-- for their next media assignment unless an operator explicitly selects the
-- compatibility fallback.

ALTER TABLE protection_policies
    ADD COLUMN data_delivery text NOT NULL DEFAULT 'direct'
        CHECK (data_delivery IN ('direct', 'disk_spool'));

ALTER TABLE single_tape_data_runs
    ADD COLUMN data_delivery text NOT NULL DEFAULT 'disk_spool'
        CHECK (data_delivery IN ('direct', 'disk_spool'));
