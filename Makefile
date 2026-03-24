.PHONY: validate build fmt

validate:
	cargo fmt --check
	cargo clippy --all-targets -- -D warnings
	cargo test

build:
	cargo build --release

fmt:
	cargo fmt
