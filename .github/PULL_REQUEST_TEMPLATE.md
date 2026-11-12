## Summary

Describe the problem and the resulting behavior.

## Safety and recovery impact

- [ ] I assessed overwrite, retention, path, privilege, persistence, and recovery effects.
- [ ] Simulator evidence is not presented as physical qualification.
- [ ] No credentials, private paths, customer data, or recovery material are included.
- [ ] Existing migrations were not modified; any schema change uses a new migration.

## Validation

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo test --all-targets --locked`
- [ ] `cargo build --release --bins --locked`
- [ ] Documentation and qualification records were updated where applicable.

List any additional tests, fixtures, screenshots, or retained evidence:
