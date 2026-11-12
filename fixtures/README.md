# Reproducible fixtures

Fixtures in this directory exercise parsers and control-plane behavior without representing physical acceptance evidence.

- `recovery-bundle/manifest.json` is a non-secret format-v1 recovery-manifest example for `tapescale-recovery inspect-bundle`.
- Safety-domain fixtures are expressed as Rust unit tests so compatibility, retention, overwrite, restore, scheduling, custody, and path behavior remain deterministic.

Physical workload fixtures for ACL round trips, ZFS streams, end-of-media, changer movement, interruption, and clean-host recovery must be captured on the designated qualification system. Their evidence reports belong in the checksum-addressed evidence system described in [the qualification ledger](../docs/QUALIFICATION.md).
