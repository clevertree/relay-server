#!/bin/bash
set -e

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "Building relay-server (debug)..."
cd "$REPO_ROOT"
cargo build

echo ""
echo "Starting relay-server (debug)..."
echo "  RELAY_REPO_ROOT=$REPO_ROOT"
echo "  RELAY_STATIC_PATHS=$REPO_ROOT/../relay-clients/packages/web/dist"
echo "  RELAY_HTTP_PORT=8080"
echo ""

export RELAY_REPO_ROOT="$REPO_ROOT"
export RELAY_STATIC_PATHS="$REPO_ROOT/../relay-clients/packages/web/dist"
export RELAY_HTTP_PORT=8080

exec "$REPO_ROOT/target/debug/relay-server"
