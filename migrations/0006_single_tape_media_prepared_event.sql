-- Retain the irreversible G0.2 scratch-label invalidation as its own event.
ALTER TABLE single_tape_data_events
    DROP CONSTRAINT single_tape_data_events_event_type_check;

ALTER TABLE single_tape_data_events
    ADD CONSTRAINT single_tape_data_events_event_type_check CHECK (event_type IN (
        'queued', 'started', 'preparing', 'writing', 'written', 'verifying',
        'restoring', 'comparing',
        'media_write_started', 'media_prepared', 'engine_dispatched', 'fenced_out',
        'passed', 'failed', 'blocked', 'interrupted', 'cancelled'
    ));
