set positional-arguments

rust_manifest := "rust/Cargo.toml"

build:
    cargo build --manifest-path {{rust_manifest}} --workspace
    uv sync --project python --frozen

test:
    cargo test --manifest-path {{rust_manifest}} --workspace --all-targets
    uv run --project python --frozen python -m pytest python/tests

lint:
    cargo fmt --manifest-path {{rust_manifest}} --all -- --check
    cargo clippy --manifest-path {{rust_manifest}} --workspace --all-targets --all-features -- -D warnings
    uv run --project python --frozen ruff check python scripts
    uv run --project python --frozen ruff format --check python scripts

contracts:
    uv run --project python --frozen python scripts/validate_contracts.py

check: build lint test contracts
