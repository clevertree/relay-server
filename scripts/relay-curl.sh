#!/usr/bin/env bash
# Run from any Linux host (as root or pipe through sudo):
#   curl -fsSL "https://raw.githubusercontent.com/clevertree/relay-server/main/scripts/relay-curl.sh" | sudo bash -s -- repair
#
# Env:
#   RELAY_REPO=clevertree/relay-server   RELAY_REF=main
#   RELAY_DEPLOY_TGZ_URL=…             full deploy .tgz for install / update (update: prefer sudo env URL when piping:
#                                      curl … | sudo env RELAY_DEPLOY_TGZ_URL=https://…/relay-linode-deploy.tgz bash -s -- update)
#   RELAY_BIN_SOURCE=/path             dir containing relay-server + relay-hook-handler (overrides tarball)
set -euo pipefail

RELAY_REPO="${RELAY_REPO:-clevertree/relay-server}"
RELAY_REF="${RELAY_REF:-main}"
BASE_RAW="https://raw.githubusercontent.com/${RELAY_REPO}/${RELAY_REF}"

log() { echo "[relay-curl] $*" >&2; }
die() { echo "[relay-curl] ERROR: $*" >&2; exit 1; }

[[ "${EUID:-$(id -u)}" -eq 0 ]] || die "run as root, e.g. curl ... | sudo bash -s -- install"

detect_pm() {
  if command -v apt-get >/dev/null 2>&1; then echo apt
  elif command -v dnf >/dev/null 2>&1; then echo dnf
  elif command -v yum >/dev/null 2>&1; then echo yum
  elif command -v apk >/dev/null 2>&1; then echo apk
  elif command -v pacman >/dev/null 2>&1; then echo pacman
  elif command -v zypper >/dev/null 2>&1; then echo zypper
  else die "no supported package manager"
  fi
}

install_pkgs() {
  local pm="$1"; shift
  case "$pm" in
    apt) apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends "$@" ;;
    dnf) dnf install -y "$@" ;;
    yum) yum install -y "$@" ;;
    apk) apk add --no-cache "$@" ;;
    pacman) pacman -Sy --noconfirm "$@" ;;
    zypper) zypper install -y "$@" ;;
  esac
}

ensure_tools() {
  command -v curl >/dev/null 2>&1 && command -v tar >/dev/null 2>&1 && command -v bash >/dev/null 2>&1 && return 0
  local pm
  pm="$(detect_pm)"
  log "Installing curl, tar, gzip, ca-certificates, bash…"
  case "$pm" in
    apt) install_pkgs apt curl ca-certificates tar gzip bash ;;
    dnf|yum) install_pkgs "$pm" curl tar gzip ca-certificates bash ;;
    apk) install_pkgs apk curl tar gzip ca-certificates bash ;;
    pacman) install_pkgs pacman curl tar gzip ca-certificates bash ;;
    zypper) install_pkgs zypper curl tar gzip ca-certificates bash ;;
  esac
}

find_bin_dir() {
  local dir="$1"
  [[ -f "$dir/relay-server" && -f "$dir/relay-hook-handler" ]] && echo "$dir" && return
  local f
  f="$(find "$dir" -maxdepth 3 -name relay-server -type f 2>/dev/null | head -1)"
  [[ -n "$f" ]] && echo "$(dirname "$f")" && return
  return 1
}

WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT

ensure_tools

log "Fetching install scripts ($RELAY_REF)…"
curl -fsSL "$BASE_RAW/scripts/relay-install.sh" -o "$WORKDIR/relay-install.sh"
curl -fsSL "$BASE_RAW/scripts/piper-tts-http.py" -o "$WORKDIR/piper-tts-http.py"
curl -fsSL "$BASE_RAW/scripts/relay-probe-features.py" -o "$WORKDIR/relay-probe-features.py"
chmod +x "$WORKDIR/relay-install.sh" "$WORKDIR/relay-probe-features.py"

INSTALL="${RELAY_INSTALL_ROOT:-/opt/relay}"
CMD="${1:-install}"
BIN_DIR=""

if [[ -n "${RELAY_BIN_SOURCE:-}" ]] && [[ -f "$RELAY_BIN_SOURCE/relay-server" ]]; then
  BIN_DIR="$(cd "$RELAY_BIN_SOURCE" && pwd)"
  log "Using RELAY_BIN_SOURCE=$BIN_DIR"
elif [[ -n "${RELAY_DEPLOY_TGZ_URL:-}" ]]; then
  log "Downloading RELAY_DEPLOY_TGZ_URL…"
  curl -fsSL "$RELAY_DEPLOY_TGZ_URL" -o "$WORKDIR/dist.tgz"
  tar -xzf "$WORKDIR/dist.tgz" -C "$WORKDIR"
  BIN_DIR="$(find_bin_dir "$WORKDIR")" || die "tarball missing relay-server binary"
  log "Binaries from tarball: $BIN_DIR"
elif [[ "$CMD" != "install" ]] || [[ "${RELAY_INSTALL_FRESH:-}" == "1" ]]; then
  if [[ -f "$INSTALL/bin/relay-server" ]]; then
    BIN_DIR="$INSTALL/bin"
    log "Using existing binaries in $BIN_DIR"
  fi
fi

if [[ -z "$BIN_DIR" ]] && [[ "$CMD" == "install" ]]; then
  die "First install needs binaries. Set RELAY_DEPLOY_TGZ_URL to your relay-linode-deploy.tgz URL, or RELAY_BIN_SOURCE=/path/to/dir with relay-server + relay-hook-handler (e.g. after tar xzf)."
fi

if [[ -z "$BIN_DIR" ]]; then
  die "No relay-server binary found. Set RELAY_DEPLOY_TGZ_URL or RELAY_BIN_SOURCE, or run from a machine that already has $INSTALL/bin/relay-server"
fi

export RELAY_BIN_SOURCE="$BIN_DIR"
cd "$WORKDIR"
exec bash "$WORKDIR/relay-install.sh" "$@"
