-- Distinguish the first Native v2 producer activation from a durable resume.
-- Existing producer_resumed history remains valid; new runs append the exact
-- lifecycle edge that occurred.

ALTER TABLE single_tape_data_events
    DROP CONSTRAINT single_tape_data_events_event_type_check;
ALTER TABLE single_tape_data_events
    ADD CONSTRAINT single_tape_data_events_event_type_check CHECK (event_type IN (
        'queued', 'started', 'preparing', 'inventorying', 'writing', 'written', 'verifying',
        'restoring', 'comparing', 'archive_staged', 'pack_staged', 'pack_assigned',
        'pack_written', 'spool_reclaimed', 'producer_paused', 'producer_started',
        'producer_resumed', 'producer_complete', 'archive_sealed', 'media_write_started',
        'media_prepared', 'media_required', 'media_authorized', 'media_loaded', 'media_full',
        'media_identified', 'media_read', 'media_verified', 'engine_dispatched', 'fenced_out',
        'cancel_requested',
        'passed', 'failed', 'blocked', 'interrupted', 'cancelled'
    ));
