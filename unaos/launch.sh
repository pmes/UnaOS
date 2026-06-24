#!/bin/bash
set -e

echo "🔹 Phase 1: Building Kernel (Isolated)..."
cd crates/kernel

# This is the EXACT command that worked for you manually
cargo +nightly build \
  --release \
  --target ../../x86_64-unaos.json \
  -Z build-std=core,compiler_builtins,alloc \
  -Z build-std-features=compiler-builtins-mem \
  -Z json-target-spec

echo "✅ Kernel Built."
cd ../..

echo "🔹 Phase 2: Packaging & Launching..."
cd builder
cargo +nightly run --release
