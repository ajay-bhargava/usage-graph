set dotenv-load := false

fmt:
	cargo fmt

clippy:
	cargo clippy --all-targets -- -D warnings

test:
	cargo test

check:
	cargo fmt --check
	cargo clippy --all-targets -- -D warnings
	cargo test
