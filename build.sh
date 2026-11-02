#!/bin/bash
set -e
source "$HOME/.cargo/env" 2>/dev/null || true
export PATH="$HOME/.cargo/bin:$PATH"

echo "Building novus2mqtt (release)..."
cargo build --release 2>&1

VERSION=$(./target/release/novus2mqtt --version 2>&1 || true)
echo "Built: ${VERSION}"
echo "Binary: target/release/novus2mqtt"
