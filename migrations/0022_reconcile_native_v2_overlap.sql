-- Reconcile operational rows that a retiring Native v1 process could have
-- inserted after migration 0021 had already completed during a container
-- replacement. Profiles are mutable configuration, while any run that has
-- started, finished, touched media, or produced archive segments remains
-- immutable Native v1 evidence.

UPDATE single_tape_data_profiles
   SET engine_name = 'TapeScale Native',
       engine_version = '2',
       adapter_version = 'native-v2',
       format_version = 'tapescale-native-v2',
       archive_codec = 'dar',
       updated_at = now()
 WHERE workflow_kind = 'backup'
   AND (engine_name, engine_version, adapter_version, format_version, archive_codec)
       IS DISTINCT FROM
       ('TapeScale Native', '2', 'native-v2', 'tapescale-native-v2', 'dar');

UPDATE single_tape_data_runs data_run
   SET engine_name = 'TapeScale Native',
       engine_version = '2',
       adapter_version = 'native-v2',
       format_version = 'tapescale-native-v2',
       archive_codec = 'dar',
       native_layout_version = 2
 WHERE data_run.workflow_kind = 'backup'
   AND data_run.state = 'queued'
   AND data_run.started_at IS NULL
   AND data_run.finished_at IS NULL
   AND data_run.media_write_started = false
   AND data_run.media_write_started_at IS NULL
   AND NOT EXISTS (
       SELECT 1
         FROM single_tape_data_run_volumes volume
        WHERE volume.single_tape_data_run_id = data_run.id
          AND (volume.state <> 'authorized'
               OR volume.media_write_started_at IS NOT NULL
               OR volume.written_at IS NOT NULL
               OR volume.verified_at IS NOT NULL)
   )
   AND NOT EXISTS (
       SELECT 1
         FROM single_tape_data_run_segments segment
        WHERE segment.single_tape_data_run_id = data_run.id
   );

UPDATE single_tape_data_run_volumes volume
   SET native_layout_version = 2
  FROM single_tape_data_runs data_run
 WHERE volume.single_tape_data_run_id = data_run.id
   AND data_run.workflow_kind = 'backup'
   AND data_run.native_layout_version = 2
   AND data_run.state = 'queued'
   AND data_run.started_at IS NULL
   AND data_run.finished_at IS NULL
   AND data_run.media_write_started = false
   AND data_run.media_write_started_at IS NULL
   AND volume.state = 'authorized'
   AND volume.media_write_started_at IS NULL
   AND volume.written_at IS NULL
   AND volume.verified_at IS NULL
   AND NOT EXISTS (
       SELECT 1
         FROM single_tape_data_run_segments segment
        WHERE segment.single_tape_data_run_id = data_run.id
   );
