#!/usr/bin/env bash
# Relay server: install | update | repair | reconfigure-features
# Works on common Linux distros (apt, dnf/yum, apk, pacman).
# Feature selection (Piper TTS, npm packages) is recorded in state/features.json
# and exposed at runtime via relay-server /api/config.
#
# Non-interactive: RELAY_INSTALL_NONINTERACTIVE=1 RELAY_FEAT_PIPER=1 RELAY_FEAT_NPM_PKGS="songwalker-js"
set -euo pipefail

INSTALL="${RELAY_INSTALL_ROOT:-/opt/relay}"
HTTP_PORT="${RELAY_HTTP_PORT:-8080}"
GIT_PORT="${RELAY_GIT_PORT:-9418}"
PIPER_HTTP_PORT="${RELAY_PIPER_HTTP_PORT:-5590}"
STATE_DIR="$INSTALL/state"
FEATURES_JSON="$STATE_DIR/features.json"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VERSION_MARKER="1"

log() { echo "[relay-install] $*" >&2; }
die() { echo "[relay-install] ERROR: $*" >&2; exit 1; }

need_root() { [[ "$(id -u)" -eq 0 ]] || die "run as root"; }

detect_pm() {
  if command -v apt-get >/dev/null 2>&1; then echo apt
  elif command -v dnf >/dev/null 2>&1; then echo dnf
  elif command -v yum >/dev/null 2>&1; then echo yum
  elif command -v apk >/dev/null 2>&1; then echo apk
  elif command -v pacman >/dev/null 2>&1; then echo pacman
  else die "no supported package manager (apt, dnf, yum, apk, pacman)"
  fi
}

install_packages() {
  local pm="$1"; shift
  case "$pm" in
    apt) apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends "$@" ;;
    dnf) dnf install -y "$@" ;;
    yum) yum install -y "$@" ;;
    apk) apk add --no-cache "$@" ;;
    pacman) pacman -Sy --noconfirm "$@" ;;
  esac
}

map_pkg_node() {
  case "$(detect_pm)" in
    apt) echo "nodejs" ;;
    dnf|yum) echo "nodejs npm" ;;
    apk) echo "nodejs npm" ;;
    pacman) echo "nodejs npm" ;;
  esac
}

ensure_user_relay() {
  id relay &>/dev/null || useradd -r -m -d /var/lib/relay -s /bin/bash relay
}

ensure_base_deps() {
  local pm
  pm="$(detect_pm)"
  local node_pkgs
  node_pkgs="$(map_pkg_node)"
  install_packages "$pm" git curl ca-certificates tar gzip xz python3 ${node_pkgs}
  command -v jq >/dev/null || install_packages "$pm" jq 2>/dev/null || {
    log "jq missing; installing via pip fallback"
    python3 -m pip install --break-system-packages jq 2>/dev/null || true
    command -v jq >/dev/null || die "install jq manually"
  }
}

write_features_json() {
  local piper_en="$1" npm_en="$2" npm_pkgs_json="$3"
  mkdir -p "$STATE_DIR"
  local model="$INSTALL/lib/piper/models/en_US-lessac-medium.onnx"
  python3 <<PY
import json
piper = "${piper_en}" == "1"
npm_on = "${npm_en}" == "1"
pkgs = json.loads(r'''${npm_pkgs_json}''')
data = {
  "schema_version": int("$VERSION_MARKER"),
  "install_root": "$INSTALL",
  "http_port": $HTTP_PORT,
  "git_port": $GIT_PORT,
  "features": {
    "piper_tts": {
      "enabled": piper,
      "expected": piper,
      "binary": "$INSTALL/lib/piper/piper" if piper else None,
      "models_dir": "$INSTALL/lib/piper/models" if piper else None,
      "default_model": "$model" if piper else None,
      "http_port": $PIPER_HTTP_PORT if piper else None,
      "health_path": "/health" if piper else None,
      "tts_path": "/tts" if piper else None,
      "service": "relay-tts-piper.service" if piper else None,
    },
    "npm_extensions": {
      "enabled": npm_on,
      "expected": npm_on,
      "directory": "$INSTALL/node_extensions" if npm_on else None,
      "packages": pkgs if npm_on else [],
    },
    "core": {
      "relay_server": True,
      "relay_git_daemon": True,
      "ports": {"http": $HTTP_PORT, "git_daemon": $GIT_PORT},
    },
  },
}
# strip None for cleaner JSON
def strip_none(d):
    if isinstance(d, dict):
        return {k: strip_none(v) for k, v in d.items() if v is not None}
    return d
with open("$FEATURES_JSON", "w") as f:
    json.dump(strip_none(data), f, indent=2)
PY
  chown relay:relay "$FEATURES_JSON" 2>/dev/null || true
}

install_piper_artifacts() {
  mkdir -p "$INSTALL/lib/piper/models"
  local arch url
  arch="$(uname -m)"
  case "$arch" in
    x86_64) url="https://github.com/rhasspy/piper/releases/download/v1.2.0/piper_linux_x86_64.tar.gz" ;;
    aarch64|arm64) url="https://github.com/rhasspy/piper/releases/download/v1.2.0/piper_linux_aarch64.tar.gz" ;;
    *) die "unsupported arch for Piper: $arch" ;;
  esac
  log "Downloading Piper..."
  curl -fsSL "$url" -o /tmp/piper.tgz
  tar -xzf /tmp/piper.tgz -C /tmp
  # release layout: piper/piper or flat
  if [[ -f /tmp/piper/piper ]]; then
    cp /tmp/piper/piper "$INSTALL/lib/piper/piper"
  elif [[ -f /tmp/piper_linux_x86_64/piper ]]; then
    cp /tmp/piper_linux_x86_64/piper "$INSTALL/lib/piper/piper"
  else
    P=$(find /tmp -name piper -type f -perm -111 2>/dev/null | head -1)
    [[ -n "$P" ]] || die "could not find piper binary in archive"
    cp "$P" "$INSTALL/lib/piper/piper"
  fi
  chmod +x "$INSTALL/lib/piper/piper"
  chown -R relay:relay "$INSTALL/lib/piper"
  rm -rf /tmp/piper.tgz /tmp/piper /tmp/piper_linux_* 2>/dev/null || true

  local mbase="https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/en/en_US/lessac/medium"
  log "Downloading default voice model..."
  curl -fsSL "$mbase/en_US-lessac-medium.onnx" -o "$INSTALL/lib/piper/models/en_US-lessac-medium.onnx"
  curl -fsSL "$mbase/en_US-lessac-medium.onnx.json" -o "$INSTALL/lib/piper/models/en_US-lessac-medium.onnx.json" || true
  chown -R relay:relay "$INSTALL/lib/piper/models"

  if [[ -f "$SCRIPT_DIR/piper-tts-http.py" ]]; then
    cp "$SCRIPT_DIR/piper-tts-http.py" "$INSTALL/bin/piper-tts-http.py"
  elif [[ -f ./piper-tts-http.py ]]; then
    cp ./piper-tts-http.py "$INSTALL/bin/piper-tts-http.py"
  else
    log "WARN: piper-tts-http.py not beside script; TTS HTTP disabled until copied"
    return 0
  fi
  chmod +x "$INSTALL/bin/piper-tts-http.py"

  cat >/etc/systemd/system/relay-tts-piper.service <<EOF
[Unit]
Description=Relay Piper TTS HTTP ($PIPER_HTTP_PORT)
After=network.target

[Service]
Type=simple
User=relay
Group=relay
Environment=PIPER_BIN=$INSTALL/lib/piper/piper
Environment=PIPER_MODEL=$INSTALL/lib/piper/models/en_US-lessac-medium.onnx
Environment=PIPER_HTTP_PORT=$PIPER_HTTP_PORT
ExecStart=/usr/bin/python3 $INSTALL/bin/piper-tts-http.py
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF
  systemctl daemon-reload
  systemctl enable relay-tts-piper
  systemctl restart relay-tts-piper || log "relay-tts-piper start failed (check python3 + model)"
}

install_npm_extensions() {
  local pkgs="$1"
  [[ -z "$pkgs" ]] && return 0
  mkdir -p "$INSTALL/node_extensions"
  chown relay:relay "$INSTALL/node_extensions"
  sudo -u relay bash -c "cd '$INSTALL/node_extensions' && (test -f package.json || npm init -y >/dev/null) && npm install --no-save $pkgs"
}

install_binaries() {
  local src="${RELAY_BIN_SOURCE:-$SCRIPT_DIR}"
  [[ -f "$src/relay-server" ]] || src="."
  [[ -f "$src/relay-server" ]] || die "relay-server binary not found (place in $SCRIPT_DIR or cwd)"
  mkdir -p "$INSTALL/bin" "$INSTALL/hooks" "$INSTALL/data" "$INSTALL/logs" "$INSTALL/www"
  cp -f "$src/relay-server" "$src/relay-hook-handler" "$INSTALL/bin/"
  chmod +x "$INSTALL/bin/relay-server" "$INSTALL/bin/relay-hook-handler"
  chown -R relay:relay "$INSTALL"
  for h in pre-receive post-receive post-update; do
    ln -sf "$INSTALL/bin/relay-hook-handler" "$INSTALL/hooks/$h"
  done
}

write_gitconfig_hooks() {
  git config --file /etc/gitconfig core.hooksPath "$INSTALL/hooks" 2>/dev/null || \
    printf '[core]\n\thooksPath = %s\n' "$INSTALL/hooks" >>/etc/gitconfig
}

write_systemd_core() {
  cat >/etc/systemd/system/relay-server.service <<EOF
[Unit]
Description=Relay HTTP server
After=network.target

[Service]
Type=simple
User=relay
Group=relay
WorkingDirectory=$INSTALL
Environment=RELAY_REPO_PATH=$INSTALL/data
Environment=RELAY_STATIC_DIR=$INSTALL/www
Environment=RELAY_HTTP_PORT=$HTTP_PORT
Environment=RELAY_BIND=0.0.0.0:$HTTP_PORT
Environment=RELAY_FEATURES_STATE_PATH=$FEATURES_JSON
Environment=RUST_LOG=info
EnvironmentFile=-$INSTALL/relay.env
ExecStart=$INSTALL/bin/relay-server serve
Restart=always
RestartSec=3

[Install]
WantedBy=multi-user.target
EOF

  cat >/etc/systemd/system/relay-git-daemon.service <<EOF
[Unit]
Description=Relay git daemon
After=network.target

[Service]
Type=simple
User=relay
Group=relay
ExecStart=/usr/bin/git daemon --reuseaddr --base-path=$INSTALL/data --export-all --enable=receive-pack --port=$GIT_PORT $INSTALL/data
Restart=always
RestartSec=3

[Install]
WantedBy=multi-user.target
EOF
  systemctl daemon-reload
}

prompt_features() {
  local piper=0 npm_en=0 npm_pkgs="songwalker-js"
  if [[ "${RELAY_INSTALL_NONINTERACTIVE:-}" == "1" ]]; then
    [[ "${RELAY_FEAT_PIPER:-0}" == "1" ]] && piper=1
    if [[ -n "${RELAY_FEAT_NPM_PKGS:-}" ]]; then npm_en=1; npm_pkgs="${RELAY_FEAT_NPM_PKGS}"; fi
    printf '%s\0%s\0%s\0' "$piper" "$npm_en" "$npm_pkgs"
    return
  fi
  if [[ ! -t 0 ]]; then
    log "non-TTY: set RELAY_INSTALL_NONINTERACTIVE=1 RELAY_FEAT_PIPER=0/1 RELAY_FEAT_NPM_PKGS='pkg1 pkg2'"
    printf '%s\0%s\0%s\0' "0" "0" ""
    return
  fi
  read -r -p "Enable Piper TTS (HTTP on port $PIPER_HTTP_PORT)? [y/N] " a
  [[ "${a,,}" == "y" ]] && piper=1
  read -r -p "Install npm packages into node_extensions (e.g. songwalker-js)? [y/N] " b
  if [[ "${b,,}" == "y" ]]; then
    npm_en=1
    read -r -p "Package names (space-separated) [songwalker-js]: " p
    [[ -n "$p" ]] && npm_pkgs="$p"
  fi
  printf '%s\0%s\0%s\0' "$piper" "$npm_en" "$npm_pkgs"
}

pkgs_to_json_array() {
  local s="$1"
  [[ -z "$s" ]] && echo '[]' && return
  local a=() i
  read -ra i <<<"$s"
  printf '['
  local first=1
  for x in "${i[@]}"; do
    [[ -z "$x" ]] && continue
    [[ $first -eq 1 ]] || printf ','
    first=0
    printf '"%s"' "${x//\"/}"
  done
  printf ']'
}

do_install() {
  need_root
  if [[ -f "$FEATURES_JSON" ]] && [[ "${RELAY_INSTALL_FRESH:-}" != "1" ]]; then
    log "Already installed ($FEATURES_JSON). Commands: update | repair | reconfigure-features"
    exit 0
  fi
  ensure_base_deps
  ensure_user_relay
  IFS= read -r -d '' piper_en; IFS= read -r -d '' npm_en; IFS= read -r -d '' npm_pkgs < <(prompt_features)
  local npm_json
  npm_json="$(pkgs_to_json_array "$npm_pkgs")"
  write_features_json "$piper_en" "$npm_en" "$npm_json"
  install_binaries
  write_gitconfig_hooks
  write_systemd_core
  [[ "$piper_en" == "1" ]] && install_piper_artifacts
  [[ "$npm_en" == "1" && -n "$npm_pkgs" ]] && install_npm_extensions "$npm_pkgs"

  touch "$INSTALL/relay.env"
  chown relay:relay "$INSTALL/relay.env"
  systemctl enable relay-server relay-git-daemon
  systemctl restart relay-git-daemon relay-server
  [[ "$piper_en" == "1" ]] && systemctl restart relay-tts-piper 2>/dev/null || true

  maybe_ufw
  log "Install complete. HTTP :$HTTP_PORT  git :$GIT_PORT  config: GET /api/config"
}

do_update() {
  need_root
  [[ -f "$FEATURES_JSON" ]] || die "run install first"
  install_binaries
  systemctl restart relay-git-daemon relay-server
  systemctl try-restart relay-tts-piper 2>/dev/null || true
  log "Binaries updated"
}

do_repair() {
  need_root
  [[ -f "$FEATURES_JSON" ]] || die "run install first"
  ensure_base_deps
  ensure_user_relay
  local piper_en npm_en
  piper_en="$(jq -r '.features.piper_tts.enabled // false' "$FEATURES_JSON" 2>/dev/null || echo false)"
  npm_en="$(jq -r '.features.npm_extensions.enabled // false' "$FEATURES_JSON" 2>/dev/null || echo false)"
  chown -R relay:relay "$INSTALL"
  install_binaries
  write_gitconfig_hooks
  write_systemd_core
  if [[ "$piper_en" == "true" ]]; then
    [[ -x "$INSTALL/lib/piper/piper" ]] || install_piper_artifacts
    systemctl enable relay-tts-piper 2>/dev/null || true
    systemctl restart relay-tts-piper 2>/dev/null || true
  fi
  if [[ "$npm_en" == "true" ]]; then
    local pkgs
    pkgs="$(jq -r '.features.npm_extensions.packages | join(" ")' "$FEATURES_JSON")"
    [[ -n "$pkgs" ]] && install_npm_extensions "$pkgs"
  fi
  systemctl restart relay-git-daemon relay-server
  maybe_ufw
  log "Repair complete"
}

do_reconfigure_features() {
  need_root
  [[ -f "$FEATURES_JSON" ]] || die "run install first"
  log "Reconfigure will replace feature installs. Continue? (features are only changed via this script)"
  if [[ "${RELAY_INSTALL_NONINTERACTIVE:-}" != "1" ]] && [[ -t 0 ]]; then
    read -r -p "[y/N] " c
    [[ "${c,,}" == "y" ]] || exit 0
  fi
  IFS= read -r -d '' piper_en; IFS= read -r -d '' npm_en; IFS= read -r -d '' npm_pkgs < <(prompt_features)
  local npm_json
  npm_json="$(pkgs_to_json_array "$npm_pkgs")"
  systemctl stop relay-tts-piper 2>/dev/null || true
  write_features_json "$piper_en" "$npm_en" "$npm_json"
  if [[ "$piper_en" == "1" ]]; then
    rm -rf "$INSTALL/lib/piper" 2>/dev/null || true
    install_piper_artifacts
    systemctl enable relay-tts-piper
    systemctl start relay-tts-piper
  else
    systemctl disable relay-tts-piper 2>/dev/null || true
    systemctl stop relay-tts-piper 2>/dev/null || true
  fi
  if [[ "$npm_en" == "1" && -n "$npm_pkgs" ]]; then
    rm -rf "$INSTALL/node_extensions/node_modules" 2>/dev/null || true
    install_npm_extensions "$npm_pkgs"
  else
    rm -rf "$INSTALL/node_extensions" 2>/dev/null || true
  fi
  systemctl restart relay-server
  log "Features reconfigured"
}

maybe_ufw() {
  command -v ufw >/dev/null || return 0
  ufw allow 22/tcp 2>/dev/null || true
  ufw allow "$HTTP_PORT/tcp" 2>/dev/null || true
  ufw allow "$GIT_PORT/tcp" 2>/dev/null || true
  local piper_en
  piper_en="$(jq -r '.features.piper_tts.enabled // false' "$FEATURES_JSON" 2>/dev/null || echo false)"
  [[ "$piper_en" == "true" ]] && ufw allow "$PIPER_HTTP_PORT/tcp" 2>/dev/null || true
  ufw --force enable 2>/dev/null || true
}

case "${1:-install}" in
  install) shift; do_install "$@" ;;
  update) shift; do_update "$@" ;;
  repair) shift; do_repair "$@" ;;
  reconfigure-features) shift; do_reconfigure_features "$@" ;;
  -h|--help)
    echo "Usage: $0 {install|update|repair|reconfigure-features}"
    echo "  install   — first-time; prompts for Piper + npm features (or set RELAY_INSTALL_NONINTERACTIVE=1)"
    echo "  update    — refresh relay-server binaries from this directory"
    echo "  repair    — fix perms, reinstall features from state/features.json"
    echo "  reconfigure-features — change Piper/npm (only supported way to add/remove features)"
    exit 0
    ;;
  *) die "unknown command: ${1:-}; try --help" ;;
esac
