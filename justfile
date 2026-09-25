# carwash development tasks. Usage: just <recipe>

default:
    @just --list

# Install carwash from this checkout
install:
    cargo install --locked --path crates/carwash

build:
    cargo build

build-release:
    cargo build --release

# Open the TUI on a directory
run path=".":
    cargo run -- {{path}}

test:
    cargo test --workspace

# Run tests matching a name, with output
test-name name:
    cargo test --workspace {{name}} -- --nocapture

fmt:
    cargo fmt --all

clippy:
    cargo clippy --workspace --all-targets -- -D warnings

# Advisories, licenses and sources
deny:
    cargo deny check

# Compile with the minimum supported Rust version
msrv:
    cargo +1.88 check --workspace --all-targets

# Everything CI runs
check:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace
    cargo deny check

# Publish the engine first, then the binary
publish:
    cargo publish -p carwash-core
    cargo publish -p carwash
