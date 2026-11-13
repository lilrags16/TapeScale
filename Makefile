.PHONY: check test web-test build run secrets

check:
	cargo fmt --all -- --check
	cargo clippy --all-targets -- -D warnings

test:
	cargo test --all-targets --locked

web-test:
	node --check web/app.js
	node --test scripts/test-web-backup-ux.mjs

build:
	cargo build --release --bins --locked

run:
	TAPESCALE_DEMO_MODE=true cargo run --bin tapescale-api

secrets:
	./scripts/init-secrets.sh
