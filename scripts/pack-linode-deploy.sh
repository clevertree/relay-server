#!/usr/bin/env bash
# Build release binaries and pack for Linode bare-metal install.
# Local deps: stable Rust (Cargo.lock v4+), build-essential, pkg-config, libssl-dev (or OpenSSL dev kit).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
cargo build --release --bin relay-server --bin relay-hook-handler
DIST="$ROOT/target/linode-deploy"
rm -rf "$DIST"
mkdir -p "$DIST"
cp target/release/relay-server target/release/relay-hook-handler "$DIST/"
cp "$ROOT/scripts/relay-install.sh" "$DIST/install.sh"
cp "$ROOT/scripts/piper-tts-http.py" "$DIST/piper-tts-http.py"
cp "$ROOT/scripts/relay-probe-features.py" "$DIST/relay-probe-features.py"
cp "$ROOT/scripts/relay-bootstrap.sh" "$DIST/relay-bootstrap.sh"
chmod +x "$DIST/install.sh" "$DIST/relay-bootstrap.sh" "$DIST/relay-probe-features.py"
tar -czvf "$ROOT/target/relay-linode-deploy.tgz" -C "$DIST" .
echo "Created $ROOT/target/relay-linode-deploy.tgz"
echo "Deploy: scp target/relay-linode-deploy.tgz root@YOUR_IP:/root/ && ssh root@YOUR_IP 'cd /root && tar xzf relay-linode-deploy.tgz && sudo ./install.sh install'"
