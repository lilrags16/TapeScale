-- Immutable path-selective restore scope.
--
-- The operator's browser selection is resolved against the authenticated
-- retained manifest during planning, then copied onto the queued run.  The
-- worker recomputes and compares this value before asking Bacula to read
-- media.  Existing rehearsals remain complete-point restores.

ALTER TABLE restore_rehearsals
    ADD COLUMN restore_scope jsonb NOT NULL DEFAULT '{"kind":"complete_point"}'::jsonb
        CHECK (jsonb_typeof(restore_scope) = 'object'
               AND restore_scope->>'kind' IN ('complete_point', 'selected_paths')
               AND octet_length(restore_scope::text) <= 1048576);

-- A selected-path proof is valuable evidence, but it must never satisfy the
-- whole-recovery-point restore-test gate.
ALTER TABLE verification_records
    DROP CONSTRAINT verification_records_kind_check;

ALTER TABLE verification_records
    ADD CONSTRAINT verification_records_kind_check
        CHECK (kind IN (
            'write_complete', 'full_readback', 'sample', 'restore_test',
            'selective_restore_test'
        ));
