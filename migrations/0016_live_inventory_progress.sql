-- Make the long source-manifest pass an explicit durable phase. Live scan counters remain in the
-- bounded run result JSON, while this transition is retained in the append-only event history.

ALTER TABLE job_runs DROP CONSTRAINT job_runs_state_check;
ALTER TABLE job_runs ADD CONSTRAINT job_runs_state_check CHECK (state IN (
    'queued', 'preparing', 'inventorying', 'waiting_for_media', 'writing', 'written',
    'verifying', 'restoring', 'comparing', 'verified', 'blocked', 'interrupted',
    'failed', 'cancelled', 'completed'
));

ALTER TABLE single_tape_data_events
    DROP CONSTRAINT single_tape_data_events_event_type_check;
ALTER TABLE single_tape_data_events
    ADD CONSTRAINT single_tape_data_events_event_type_check CHECK (event_type IN (
        'queued', 'started', 'preparing', 'inventorying', 'writing', 'written', 'verifying',
        'restoring', 'comparing', 'media_write_started', 'media_prepared',
        'media_required', 'media_authorized', 'media_loaded', 'media_full',
        'media_read', 'media_verified', 'engine_dispatched', 'fenced_out', 'cancel_requested',
        'passed', 'failed', 'blocked', 'interrupted', 'cancelled'
    ));
