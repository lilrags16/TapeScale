-- Protect streaming tape drives from the segmented fill/drain behavior of
-- Bacula's bounded same-job spool.
--
-- TapeScale now treats disk_spool as a capacity-gated whole-archive staging
-- contract: the worker requires a conservative archive estimate to fit before
-- dispatch, then fails closed near the hard limit or when Bacula reports an
-- early drain rather than knowingly continuing cyclic partial batches.
-- Existing saved direct policies are moved to the safe default once; an
-- operator can deliberately select direct delivery again after qualifying
-- sustained source throughput. Already assigned runs retain their immutable
-- delivery decision.

ALTER TABLE protection_policies
    ALTER COLUMN data_delivery SET DEFAULT 'disk_spool';

UPDATE protection_policies
SET data_delivery = 'disk_spool'
WHERE data_delivery = 'direct';
