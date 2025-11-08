# CarWash Development Tasks
# Usage: just <recipe>

# Default recipe - show available commands
default:
    @just --list

# Install carwash locally from source
install:
    cargo install --path .

# Build debug version
build:
    cargo build

# Build optimized release version
build-release:
    cargo build --release

# Run the application (scan current directory)
run:
    cargo run

# Run with a specific directory
run-path path:
    cargo run -- {{path}}

# Run all tests
test:
    cargo test --lib

# Run tests with output
test-verbose:
    cargo test --lib -- --nocapture

# Run a specific test
test-name name:
    cargo test {{name}} -- --nocapture

# Format code
fmt:
    cargo fmt

# Check code formatting without modifying
fmt-check:
    cargo fmt -- --check

# Run clippy lints
clippy:
    cargo clippy

# Run clippy with warnings as errors
clippy-strict:
    cargo clippy -- -D warnings

# Run all checks (fmt, clippy, test)
check: fmt clippy test

# Package for publishing (creates tarball)
package:
    cargo package

# Publish to crates.io
publish:
    cargo publish

# Clean build artifacts
clean:
    cargo clean

# Update dependencies
update-deps:
    cargo update

# Update and rebuild
update: update-deps build
