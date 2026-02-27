.PHONY: build test check fmt lint clean run-snapshot run-mcp run-mcp-http install token-compare

build:
	cargo build

release:
	cargo build --release

test:
	cargo test

test-verbose:
	cargo test -- --nocapture

check:
	cargo check

fmt:
	cargo fmt

fmt-check:
	cargo fmt -- --check

lint:
	cargo clippy -- -D warnings

run-snapshot:
	cargo run -- snapshot test.html

run-snapshot-json:
	cargo run -- snapshot test.html -f json

run-mcp:
	cargo run -- mcp --launch

run-mcp-connect:
	cargo run -- mcp --port 9222

run-mcp-http:
	cargo run -- mcp-http --launch

run-mcp-http-connect:
	cargo run -- mcp-http --port 9222

token-compare: release
	python3 scripts/token-compare.py

install:
	cargo install --path .

clean:
	cargo clean
