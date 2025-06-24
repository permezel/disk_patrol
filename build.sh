#!/bin/bash
#
# build for local use

set -e

# Clean previous builds
#cargo clean

# Build static binary
#cross build --target x86_64-unknown-linux-musl --release
cargo build --release

# Copy to dist directory
mkdir -p dist
cp target/release/disk_patrol dist/

echo "Build complete: dist/disk_patrol"
echo "Binary size: $(du -h dist/disk_patrol | cut -f1)"
