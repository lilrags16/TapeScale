-- Operator-visible job deletion without destroying physical or recovery evidence.
--
-- A dismissed job run is hidden from Backup Center and generic job/action views.
-- Its backup request, transitions, media bindings, artifacts, recovery points,
-- Native volume chain, and audit records remain immutable and recoverable.

ALTER TABLE job_runs
    ADD COLUMN dismissed_at timestamptz,
    ADD COLUMN dismissed_by_label text
        CHECK (dismissed_by_label IS NULL OR
               (dismissed_by_label <> ''
                AND octet_length(dismissed_by_label) <= 256
                AND dismissed_by_label !~ '[[:cntrl:]]')),
    ADD COLUMN dismissal_reason text
        CHECK (dismissal_reason IS NULL OR
               (dismissal_reason <> ''
                AND octet_length(dismissal_reason) <= 1000
                AND dismissal_reason !~ '[[:cntrl:]]')),
    ADD CONSTRAINT job_runs_dismissal_contract CHECK (
        (dismissed_at IS NULL
         AND dismissed_by_label IS NULL
         AND dismissal_reason IS NULL) OR
        (dismissed_at IS NOT NULL
         AND dismissed_by_label IS NOT NULL
         AND dismissal_reason IS NOT NULL)
    );

CREATE INDEX job_runs_visible_recent
    ON job_runs(job_id, created_at DESC, id DESC)
    WHERE dismissed_at IS NULL;

CREATE INDEX backup_requests_profile_recent
    ON backup_requests(profile_id, created_at DESC, id DESC);

ALTER TABLE backup_request_events
    DROP CONSTRAINT backup_request_events_event_type_check;
ALTER TABLE backup_request_events
    ADD CONSTRAINT backup_request_events_event_type_check
    CHECK (event_type IN (
        'requested', 'waiting_for_media', 'assignment_recorded', 'cancelled',
        'deleted'
    ));
