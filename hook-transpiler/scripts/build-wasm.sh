#!/bin/bash
set -e

echo "Building hook-transpiler WASM..."

# Ensure wasm32 target is installed
rustup target add wasm32-unknown-unknown

# Build WASM
cargo build --release --target wasm32-unknown-unknown --features wasm

# Create wasm directory
mkdir -p ./wasm

# Generate JS bindings
wasm-bindgen \
  --target web \
  --out-dir ./wasm \
  --out-name hook_transpiler \
  target/wasm32-unknown-unknown/release/relay_hook_transpiler.wasm

# Optimize WASM if wasm-opt is available
if command -v wasm-opt &> /dev/null; then
  echo "Optimizing WASM with wasm-opt..."
  wasm-opt -Oz -o ./wasm/hook_transpiler_bg.wasm ./wasm/hook_transpiler_bg.wasm
fi

echo "✓ WASM build complete: ./wasm/"
ls -lh ./wasm/
