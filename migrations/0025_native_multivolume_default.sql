-- Native v2 is a bounded rolling archive and can safely request continuation
-- media at complete pack/filemark boundaries. Older saved backup plans kept
-- the pre-v2 single-tape compatibility default, which turns an ordinary full
-- cartridge into a terminal failure for archives larger than one tape.

UPDATE protection_policies policy
   SET spanning_enabled = true,
       updated_at = now()
  FROM single_tape_data_profiles profile
 WHERE profile.policy_id = policy.id
   AND profile.workflow_kind = 'backup'
   AND profile.engine_name = 'TapeScale Native'
   AND profile.engine_version = '2'
   AND policy.spanning_enabled = false;
