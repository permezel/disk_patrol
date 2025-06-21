#!/bin/bash

set -e

echo "Building disk_patrol for x86_64 Linux..."

# Clean previous builds
#cargo clean

# Build static binary
#cross build --target x86_64-unknown-linux-musl --release
cargo build --target x86_64-unknown-linux-musl --release

# Copy to dist directory
mkdir -p dist
cp target/x86_64-unknown-linux-musl/release/disk_patrol dist/disk_patrol-x86_64-linux

echo "Build complete: dist/disk_patrol-x86_64-linux"
echo "Binary size: $(du -h dist/disk_patrol-x86_64-linux | cut -f1)"
