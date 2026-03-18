#!/usr/bin/env bash
# Deprecated name: use relay-install.sh or the tarball's install.sh (same script).
exec "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/relay-install.sh" "$@"
