#!/usr/bin/env bash
# Relay server: install | update | repair | reconfigure-features
# Works on common Linux distros (apt, dnf/yum, apk, pacman).
# Feature selection (Piper TTS, npm packages) is recorded in state/features.json
# and exposed at runtime via relay-server /api/config.
#
# Non-interactive: RELAY_INSTALL_NONINTERACTIVE=1 RELAY_FEAT_PIPER=1 RELAY_FEAT_NPM_PKGS="songwalker-js"
# Vercel DNS (first step of install unless skipped):
#   RELAY_PUBLIC_FQDN=atlanta1.relaygateway.net VERCEL_API_TOKEN=... sudo -E ./install.sh install
#   Optional: VERCEL_TEAM_ID, RELAY_VERCEL_DOMAIN (zone), RELAY_SKIP_VERCEL_DNS=1
set -euo pipefail

INSTALL="${RELAY_INSTALL_ROOT:-/opt/relay}"
HTTP_PORT="${RELAY_HTTP_PORT:-8080}"
GIT_PORT="${RELAY_GIT_PORT:-9418}"
PIPER_HTTP_PORT="${RELAY_PIPER_HTTP_PORT:-5590}"
STATE_DIR="$INSTALL/state"
FEATURES_JSON="$STATE_DIR/features.json"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VERSION_MARKER="2"

log() { echo "[relay-install] $*" >&2; }
die() { echo "[relay-install] ERROR: $*" >&2; exit 1; }

need_root() { [[ "$(id -u)" -eq 0 ]] || die "run as root"; }

detect_pm() {
  if command -v apt-get >/dev/null 2>&1; then echo apt
  elif command -v dnf >/dev/null 2>&1; then echo dnf
  elif command -v yum >/dev/null 2>&1; then echo yum
  elif command -v apk >/dev/null 2>&1; then echo apk
  elif command -v pacman >/dev/null 2>&1; then echo pacman
  elif command -v zypper >/dev/null 2>&1; then echo zypper
  else die "no supported package manager (apt, dnf, yum, apk, pacman, zypper)"
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
    zypper) zypper install -y "$@" ;;
  esac
}

map_pkg_node() {
  case "$(detect_pm)" in
    apt) echo "nodejs" ;;
    dnf|yum|zypper) echo "nodejs npm" ;;
    apk) echo "nodejs npm" ;;
    pacman) echo "nodejs npm" ;;
  esac
}

# Debian/Ubuntu use package xz-utils; others typically use xz.
map_pkg_xz() {
  case "$(detect_pm)" in
    apt) echo xz-utils ;;
    *) echo xz ;;
  esac
}

ensure_user_relay() {
  id relay &>/dev/null || useradd -r -m -d /var/lib/relay -s /bin/bash relay
}

ensure_base_deps() {
  local pm xz_pkg
  pm="$(detect_pm)"
  xz_pkg="$(map_pkg_xz)"
  local node_pkgs
  node_pkgs="$(map_pkg_node)"
  install_packages "$pm" git curl ca-certificates tar gzip "$xz_pkg" python3 ${node_pkgs}
  command -v jq >/dev/null || install_packages "$pm" jq 2>/dev/null || {
    log "jq missing; installing via pip fallback"
    python3 -m pip install --break-system-packages jq 2>/dev/null || true
    command -v jq >/dev/null || die "install jq manually"
  }
}

refresh_features_inventory() {
  [[ -f "$SCRIPT_DIR/relay-probe-features.py" ]] || return 0
  python3 "$SCRIPT_DIR/relay-probe-features.py" merge "$INSTALL" || log "WARN: feature inventory refresh failed"
}

write_features_json() {
  local piper_en="$1" npm_en="$2" npm_pkgs_json="$3" trans_en="$4" trans_pkgs_json="$5"
  mkdir -p "$STATE_DIR"
  local model="$INSTALL/lib/piper/models/en_US-lessac-medium.onnx"
  RELAY_WFJ_INSTALL="$INSTALL" \
  RELAY_WFJ_HTTP_PORT="$HTTP_PORT" \
  RELAY_WFJ_GIT_PORT="$GIT_PORT" \
  RELAY_WFJ_PIPER_PORT="$PIPER_HTTP_PORT" \
  RELAY_WFJ_VERSION="$VERSION_MARKER" \
  RELAY_WFJ_PIPER_EN="$piper_en" \
  RELAY_WFJ_NPM_EN="$npm_en" \
  RELAY_WFJ_NPM_PKGS_JSON="$npm_pkgs_json" \
  RELAY_WFJ_TRANS_EN="$trans_en" \
  RELAY_WFJ_TRANS_PKGS_JSON="$trans_pkgs_json" \
  RELAY_WFJ_FEATURES_JSON="$FEATURES_JSON" \
  RELAY_WFJ_MODEL="$model" \
    python3 <<'PY'
import json
import os

install = os.environ["RELAY_WFJ_INSTALL"]
features_path = os.environ["RELAY_WFJ_FEATURES_JSON"]
model = os.environ["RELAY_WFJ_MODEL"]
http_port = int(os.environ["RELAY_WFJ_HTTP_PORT"])
git_port = int(os.environ["RELAY_WFJ_GIT_PORT"])
piper_port = int(os.environ["RELAY_WFJ_PIPER_PORT"])
ver = int(os.environ["RELAY_WFJ_VERSION"])

piper = os.environ["RELAY_WFJ_PIPER_EN"] == "1"
npm_on = os.environ["RELAY_WFJ_NPM_EN"] == "1"
trans = os.environ["RELAY_WFJ_TRANS_EN"] == "1"
pkgs = json.loads(os.environ["RELAY_WFJ_NPM_PKGS_JSON"])
trans_pkgs = json.loads(os.environ["RELAY_WFJ_TRANS_PKGS_JSON"])

data = {
    "schema_version": ver,
    "install_root": install,
    "http_port": http_port,
    "git_port": git_port,
    "features": {
        "piper_tts": {
            "enabled": piper,
            "expected": piper,
            "binary": f"{install}/lib/piper/piper" if piper else None,
            "models_dir": f"{install}/lib/piper/models" if piper else None,
            "default_model": model if piper else None,
            "http_port": piper_port if piper else None,
            "health_path": "/health" if piper else None,
            "tts_path": "/tts" if piper else None,
            "service": "relay-tts-piper.service" if piper else None,
            "voices": [],
            "languages": [],
        },
        "npm_extensions": {
            "enabled": npm_on,
            "expected": npm_on,
            "directory": f"{install}/node_extensions" if npm_on else None,
            "packages": pkgs if npm_on else [],
        },
        "text_translation": {
            "enabled": trans,
            "expected": trans,
            "backend": "argos_translate_local" if trans else None,
            "description": "Offline neural translation (no cloud); install language packs via argospm.",
            "venv_dir": f"{install}/lib/argos-venv" if trans else None,
            "venv_python": f"{install}/lib/argos-venv/bin/python3" if trans else None,
            "cli": "argospm" if trans else None,
            "install_argos_packages": trans_pkgs if trans else [],
            "language_pairs": [],
            "from_languages": [],
            "to_languages": [],
        },
        "core": {
            "relay_server": True,
            "relay_git_daemon": True,
            "ports": {"http": http_port, "git_daemon": git_port},
        },
    },
}


def strip_none(d):
    if isinstance(d, dict):
        return {k: strip_none(v) for k, v in d.items() if v is not None}
    return d


with open(features_path, "w", encoding="utf-8") as f:
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

install_translation_artifacts() {
  local pm venv_dir pkg
  pm="$(detect_pm)"
  case "$pm" in
    apt) install_packages apt python3-venv python3-pip ;;
    dnf|yum) install_packages "$pm" python3 python3-pip || true ;;
    apk) install_packages apk python3 py3-pip python3-dev ;;
    pacman) install_packages pacman python python-pip ;;
    zypper) install_packages zypper python311 python311-pip || install_packages zypper python3 python3-pip ;;
  esac
  venv_dir="$INSTALL/lib/argos-venv"
  mkdir -p "$INSTALL/lib"
  if [[ ! -x "$venv_dir/bin/python3" ]]; then
    log "Creating Argos Translate venv at $venv_dir"
    sudo -u relay python3 -m venv "$venv_dir"
  fi
  sudo -u relay "$venv_dir/bin/pip" install --upgrade pip setuptools wheel
  sudo -u relay "$venv_dir/bin/pip" install argostranslate || die "pip install argostranslate failed"
  chown -R relay:relay "$venv_dir"
  for pkg in "$@"; do
    [[ -z "$pkg" ]] && continue
    log "argospm install $pkg"
    sudo -u relay "$venv_dir/bin/argospm" install "$pkg" || log "WARN: argospm install $pkg failed (check package name / network)"
  done
}

# Copy binary into INSTALL/bin unless it is already that file (repair often uses RELAY_BIN_SOURCE=$INSTALL/bin).
install_binaries() {
  local src="${RELAY_BIN_SOURCE:-$SCRIPT_DIR}"
  [[ -f "$src/relay-server" ]] || src="."
  [[ -f "$src/relay-server" ]] || die "relay-server binary not found (place in $SCRIPT_DIR or cwd)"
  [[ -f "$src/relay-hook-handler" ]] || die "relay-hook-handler binary not found beside relay-server in $src"
  mkdir -p "$INSTALL/bin" "$INSTALL/hooks" "$INSTALL/data" "$INSTALL/logs" "$INSTALL/www"
  _copy_bin_if_needed() {
    local s="$1" d="$2"
    if [[ -f "$d" ]] && [[ "$(stat -c '%d:%i' "$s")" == "$(stat -c '%d:%i' "$d")" ]]; then
      return 0
    fi
    cp -f "$s" "$d"
  }
  _copy_bin_if_needed "$src/relay-server" "$INSTALL/bin/relay-server"
  _copy_bin_if_needed "$src/relay-hook-handler" "$INSTALL/bin/relay-hook-handler"
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
  local piper=0 npm_en=0 npm_pkgs="songwalker-js" trans=0 trans_pkgs=""
  if [[ "${RELAY_INSTALL_NONINTERACTIVE:-}" == "1" ]]; then
    [[ "${RELAY_FEAT_PIPER:-0}" == "1" ]] && piper=1
    if [[ -n "${RELAY_FEAT_NPM_PKGS:-}" ]]; then npm_en=1; npm_pkgs="${RELAY_FEAT_NPM_PKGS}"; fi
    [[ "${RELAY_FEAT_TRANSLATION:-0}" == "1" ]] && trans=1
    trans_pkgs="${RELAY_FEAT_TRANSLATION_PKGS:-}"
    printf '%s\0%s\0%s\0%s\0%s\0' "$piper" "$npm_en" "$npm_pkgs" "$trans" "$trans_pkgs"
    return
  fi
  if [[ ! -t 0 ]]; then
    log "non-TTY: set RELAY_INSTALL_NONINTERACTIVE=1 RELAY_FEAT_PIPER=0/1 RELAY_FEAT_NPM_PKGS='pkg1 pkg2' RELAY_FEAT_TRANSLATION=0/1 RELAY_FEAT_TRANSLATION_PKGS='translate-en_es ...'"
    printf '%s\0%s\0%s\0%s\0%s\0' "0" "0" "" "0" ""
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
  read -r -p "Enable offline text translation (Argos Translate local, no cloud)? [y/N] " c
  [[ "${c,,}" == "y" ]] && trans=1
  if [[ "$trans" == "1" ]]; then
    read -r -p "Argos package ids to install now (space-separated, e.g. translate-en_es), or leave empty: " t
    trans_pkgs="$t"
  fi
  printf '%s\0%s\0%s\0%s\0%s\0' "$piper" "$npm_en" "$npm_pkgs" "$trans" "$trans_pkgs"
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

# If binaries exist but state was never written (manual copy / old install), create minimal features.json.
bootstrap_features_json_if_missing() {
  [[ -f "$FEATURES_JSON" ]] && return 0
  [[ -f "$INSTALL/bin/relay-server" ]] || return 0
  log "No $FEATURES_JSON — bootstrapping minimal state (Piper/npm/translation off). Use reconfigure-features to enable."
  write_features_json 0 0 "[]" 0 "$(pkgs_to_json_array "")"
}

# --- Vercel DNS (install-time; same token env as Docker/K8s: VERCEL_API_TOKEN, optional VERCEL_TEAM_ID) ---
RELAY_CONFIGURED_PUBLIC_FQDN=""

ensure_minimal_network_tools() {
  local pm
  pm="$(detect_pm)"
  command -v curl >/dev/null || install_packages "$pm" curl ca-certificates
  if ! command -v jq >/dev/null; then
    install_packages "$pm" jq 2>/dev/null || {
      command -v python3 >/dev/null || install_packages "$pm" python3
      python3 -m pip install --break-system-packages jq 2>/dev/null || true
    }
    command -v jq >/dev/null || die "jq required for Vercel DNS; install jq then re-run"
  fi
  if ! command -v dig >/dev/null && ! command -v host >/dev/null; then
    case "$pm" in
      apt) install_packages apt dnsutils ;;
      dnf|yum) install_packages "$pm" bind-utils ;;
      zypper) install_packages zypper bind-utils ;;
      apk) install_packages apk bind-tools ;;
      pacman) install_packages pacman bind-tools ;;
      *) ;;
    esac
  fi
}

relay_get_public_ipv4() {
  local ip url
  for url in "https://api.ipify.org" "https://ipv4.icanhazip.com" "https://ifconfig.me/ip"; do
    ip=$(curl -fsS -m 15 "$url" 2>/dev/null | tr -d '\r\n' || true)
    if [[ "$ip" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
      echo "$ip"
      return 0
    fi
  done
  return 1
}

vercel_dns_records_list_url() {
  local domain="$1" name="$2" type="$3"
  local base="https://api.vercel.com/v4/domains/${domain}/records"
  if [[ -z "$name" ]]; then
    if [[ -n "${VERCEL_TEAM_ID:-}" ]]; then
      echo "${base}?teamId=${VERCEL_TEAM_ID}&type=${type}"
    else
      echo "${base}?type=${type}"
    fi
  else
    if [[ -n "${VERCEL_TEAM_ID:-}" ]]; then
      echo "${base}?teamId=${VERCEL_TEAM_ID}&name=${name}&type=${type}"
    else
      echo "${base}?name=${name}&type=${type}"
    fi
  fi
}

# Upsert A record; name may be empty for apex.
vercel_dns_upsert_a() {
  local domain="$1" name="$2" value="$3"
  local ttl="${RELAY_DNS_TTL:-60}"
  local token="${VERCEL_API_TOKEN:-${VERCEL_TOKEN:-}}"
  [[ -n "$token" ]] || die "vercel_dns_upsert_a: missing VERCEL_API_TOKEN"
  local auth_header="Authorization: Bearer ${token}"
  local base="https://api.vercel.com"
  local list_url team_q
  list_url="$(vercel_dns_records_list_url "$domain" "$name" A)"
  local list_raw rec_id
  list_raw=$(curl -sS -H "$auth_header" "$list_url" || true)
  echo "$list_raw" | jq -e . >/dev/null 2>&1 || die "Vercel DNS list failed for ${domain}: ${list_raw:0:400}"
  if [[ -z "$name" ]]; then
    rec_id=$(echo "$list_raw" | jq -r '.records[]? | select((.name==null or .name=="" or .name=="@") and .type=="A") | .id // .uid' | head -n1 || true)
  else
    rec_id=$(echo "$list_raw" | jq -r --arg n "$name" '.records[]? | select(.name==$n and .type=="A") | .id // .uid' | head -n1 || true)
  fi

  if [[ -n "$rec_id" && "$rec_id" != "null" ]]; then
    local patch_url="${base}/v4/domains/${domain}/records/${rec_id}"
    [[ -n "${VERCEL_TEAM_ID:-}" ]] && patch_url="${patch_url}?teamId=${VERCEL_TEAM_ID}"
    local body
    body=$(jq -n --arg v "$value" --argjson t "$ttl" '{ value: $v, ttl: $t }')
    curl -fsS -X PATCH -H "$auth_header" -H 'Content-Type: application/json' -d "$body" "$patch_url" >/dev/null && return 0
  else
    local create_url="${base}/v4/domains/${domain}/records"
    [[ -n "${VERCEL_TEAM_ID:-}" ]] && create_url="${create_url}?teamId=${VERCEL_TEAM_ID}"
    local body
    if [[ -z "$name" ]]; then
      body=$(jq -n --arg v "$value" --arg t "A" --argjson ttl "$ttl" '{ name: "", value: $v, type: $t, ttl: $ttl }')
    else
      body=$(jq -n --arg n "$name" --arg v "$value" --arg t "A" --argjson ttl "$ttl" '{ name: $n, value: $v, type: $t, ttl: $ttl }')
    fi
    curl -fsS -X POST -H "$auth_header" -H 'Content-Type: application/json' -d "$body" "$create_url" >/dev/null && return 0
  fi
  return 1
}

vercel_list_domain_names() {
  local token="$1"
  local url="https://api.vercel.com/v5/domains"
  [[ -n "${VERCEL_TEAM_ID:-}" ]] && url="${url}?teamId=${VERCEL_TEAM_ID}"
  local raw
  raw=$(curl -fsS -H "Authorization: Bearer ${token}" "$url")
  echo "$raw" | jq -r '.domains[]?.name | select(.!=null)'
}

# Sets RESOLVED_VERCEL_DOMAIN and RESOLVED_VERCEL_RECORD_NAME (may be empty for apex).
resolve_vercel_zone_for_fqdn() {
  local fqdn="$1" token="$2"
  RESOLVED_VERCEL_DOMAIN=""
  RESOLVED_VERCEL_RECORD_NAME=""
  if [[ -n "${RELAY_VERCEL_DOMAIN:-}" ]]; then
    local d="${RELAY_VERCEL_DOMAIN}"
    if [[ "$fqdn" == "$d" ]]; then
      RESOLVED_VERCEL_DOMAIN="$d"
      RESOLVED_VERCEL_RECORD_NAME=""
      return 0
    fi
    local suffix=".$d"
    if [[ "$fqdn" == *"$suffix" ]]; then
      RESOLVED_VERCEL_DOMAIN="$d"
      RESOLVED_VERCEL_RECORD_NAME="${fqdn%"$suffix"}"
      return 0
    fi
    die "RELAY_VERCEL_DOMAIN=$d does not match FQDN=$fqdn"
  fi

  local raw best="" best_len=0 d len
  raw=$(curl -fsS -H "Authorization: Bearer ${token}" "https://api.vercel.com/v5/domains$([[ -n "${VERCEL_TEAM_ID:-}" ]] && echo "?teamId=${VERCEL_TEAM_ID}")")
  echo "$raw" | jq -e . >/dev/null 2>&1 || die "Vercel domains list failed: ${raw:0:500}"

  while IFS= read -r d; do
    [[ -z "$d" ]] && continue
    if [[ "$fqdn" == "$d" ]]; then
      RESOLVED_VERCEL_DOMAIN="$d"
      RESOLVED_VERCEL_RECORD_NAME=""
      return 0
    fi
    if [[ "$fqdn" == *".$d" ]]; then
      len=${#d}
      if [[ $len -gt $best_len ]]; then
        best_len=$len
        best="$d"
      fi
    fi
  done < <(echo "$raw" | jq -r '.domains[]?.name | select(.!=null)')

  [[ -n "$best" ]] || die "No Vercel domain matches FQDN=$fqdn. Add the domain in Vercel or set RELAY_VERCEL_DOMAIN."
  RESOLVED_VERCEL_DOMAIN="$best"
  RESOLVED_VERCEL_RECORD_NAME="${fqdn%.$best}"
}

wait_dns_a_record() {
  local fqdn="$1" want_ip="$2"
  local max="${RELAY_DNS_WAIT_ATTEMPTS:-36}"
  local sleep_s="${RELAY_DNS_WAIT_SLEEP:-5}"
  local i=0 got
  while [[ $i -lt "$max" ]]; do
    got=""
    if command -v dig >/dev/null 2>&1; then
      got=$(dig +short A "$fqdn" @8.8.8.8 2>/dev/null | head -n1 | tr -d '\r\n' || true)
    elif command -v host >/dev/null 2>&1; then
      got=$(host -t A "$fqdn" 8.8.8.8 2>/dev/null | awk '/has address/ { print $4; exit }' | tr -d '\r\n' || true)
    else
      got=$(python3 -c "import socket; print(socket.gethostbyname('$fqdn'))" 2>/dev/null | tr -d '\r\n' || true)
    fi
    if [[ "$got" == "$want_ip" ]]; then
      log "DNS OK: $fqdn -> $got"
      return 0
    fi
    log "Waiting for DNS: want A $fqdn -> $want_ip (got '${got:-empty}') [$((i + 1))/$max]"
    sleep "$sleep_s"
    i=$((i + 1))
  done
  die "DNS did not resolve $fqdn to $want_ip after $max attempts (${sleep_s}s apart). Check Vercel dashboard and TTL."
}

configure_vercel_dns_first() {
  RELAY_CONFIGURED_PUBLIC_FQDN=""
  if [[ "${RELAY_SKIP_VERCEL_DNS:-}" == "1" ]]; then
    log "Skipping Vercel DNS (RELAY_SKIP_VERCEL_DNS=1)"
    return 0
  fi

  local fqdn="" token=""
  fqdn="${RELAY_PUBLIC_FQDN:-${RELAY_DNS_FQDN:-}}"
  token="${VERCEL_API_TOKEN:-${VERCEL_TOKEN:-}}"

  if [[ "${RELAY_INSTALL_NONINTERACTIVE:-}" == "1" ]]; then
    [[ -n "$fqdn" && -n "$token" ]] || die "Non-interactive install needs RELAY_PUBLIC_FQDN (or RELAY_DNS_FQDN) and VERCEL_API_TOKEN for DNS, or set RELAY_SKIP_VERCEL_DNS=1"
  else
    if [[ ! -t 0 ]]; then
      if [[ -z "$fqdn" || -z "$token" ]]; then
        log "Non-TTY install: set RELAY_PUBLIC_FQDN + VERCEL_API_TOKEN, or RELAY_SKIP_VERCEL_DNS=1 — skipping DNS"
        return 0
      fi
    else
      read -r -p "Update Vercel DNS to this server's public IPv4? [Y/n] " dns_yn
      if [[ "${dns_yn,,}" == "n" ]]; then
        log "Skipping Vercel DNS (user declined)"
        return 0
      fi
      while [[ -z "$fqdn" ]]; do
        read -r -p "Full hostname (e.g. atlanta1.relaygateway.net): " fqdn
        fqdn="${fqdn// /}"
      done
      if [[ -z "$token" ]]; then
        read -r -s -p "Vercel API token (same as VERCEL_API_TOKEN in Kubernetes/Docker): " token
        echo "" >&2
      fi
      [[ -n "$token" ]] || die "Vercel API token required"
    fi
  fi

  fqdn="${fqdn,,}"
  [[ "$fqdn" =~ ^[a-z0-9.-]+$ ]] || die "Invalid FQDN: $fqdn"

  local pub_ip
  pub_ip="$(relay_get_public_ipv4)" || die "Could not detect public IPv4 (need outbound HTTPS)"
  log "Public IPv4 (detected): $pub_ip"

  export VERCEL_API_TOKEN="$token"
  resolve_vercel_zone_for_fqdn "$fqdn" "$token"
  log "Vercel zone: $RESOLVED_VERCEL_DOMAIN  record name: '${RESOLVED_VERCEL_RECORD_NAME:-@}'"

  vercel_dns_upsert_a "$RESOLVED_VERCEL_DOMAIN" "$RESOLVED_VERCEL_RECORD_NAME" "$pub_ip" || die "Vercel DNS upsert failed (check token, domain on account, and team id)"

  wait_dns_a_record "$fqdn" "$pub_ip"
  RELAY_CONFIGURED_PUBLIC_FQDN="$fqdn"
  log "Vercel DNS configured and verified for $fqdn"
}

# Writes RELAY_PUBLIC_HOSTNAME from Vercel-configured FQDN and/or RELAY_PUBLIC_HOSTNAME env
# (needed when RELAY_SKIP_VERCEL_DNS=1 but Host-based routing still requires the node name).
append_relay_env_public_host() {
  local f="$INSTALL/relay.env"
  local h="${RELAY_CONFIGURED_PUBLIC_FQDN:-}"
  [[ -z "$h" ]] && h="${RELAY_PUBLIC_HOSTNAME:-}"
  [[ -z "$h" ]] && return 0
  grep -q '^RELAY_PUBLIC_HOSTNAME=' "$f" 2>/dev/null && return 0
  echo "RELAY_PUBLIC_HOSTNAME=$h" >>"$f"
}

# Optional unique id for /api/config and ops (e.g. relay-dallas1).
append_relay_env_server_id() {
  local f="$INSTALL/relay.env"
  local id=""
  if [[ "${RELAY_INSTALL_NONINTERACTIVE:-}" == "1" ]]; then
    id="${RELAY_SERVER_ID:-}"
  elif [[ -t 0 ]]; then
    read -r -p "RELAY_SERVER_ID — unique name for this node (e.g. relay-dallas1; empty to skip): " id
    id="${id// /}"
  fi
  [[ -z "$id" ]] && return 0
  grep -q '^RELAY_SERVER_ID=' "$f" 2>/dev/null && return 0
  echo "RELAY_SERVER_ID=$id" >>"$f"
}

# Semicolon-separated peer hostnames for /api/config and future sync (RELAY_MASTER_PEER_LIST).
append_relay_env_peer_list() {
  local f="$INSTALL/relay.env"
  local peers=""
  if [[ "${RELAY_INSTALL_NONINTERACTIVE:-}" == "1" ]]; then
    peers="${RELAY_MASTER_PEER_LIST:-}"
  elif [[ -t 0 ]]; then
    read -r -p "Other relay nodes already running (semicolon-separated FQDNs for sync/discovery; empty if none) [e.g. atlanta1.relaygateway.net]: " peers
  fi
  peers="${peers// /}"
  [[ -z "$peers" ]] && return 0
  grep -q '^RELAY_MASTER_PEER_LIST=' "$f" 2>/dev/null && return 0
  echo "RELAY_MASTER_PEER_LIST=$peers" >>"$f"
}

do_install() {
  need_root
  if [[ -f "$FEATURES_JSON" ]] && [[ "${RELAY_INSTALL_FRESH:-}" != "1" ]]; then
    log "Already installed ($FEATURES_JSON). Commands: update | repair | reconfigure-features"
    exit 0
  fi
  ensure_minimal_network_tools
  configure_vercel_dns_first
  ensure_base_deps
  ensure_user_relay
  { IFS= read -r -d '' piper_en && IFS= read -r -d '' npm_en && IFS= read -r -d '' npm_pkgs && IFS= read -r -d '' trans_en && IFS= read -r -d '' trans_pkgs; } < <(prompt_features)
  local npm_json trans_json
  npm_json="$(pkgs_to_json_array "$npm_pkgs")"
  trans_json="$(pkgs_to_json_array "$trans_pkgs")"
  write_features_json "$piper_en" "$npm_en" "$npm_json" "$trans_en" "$trans_json"
  install_binaries
  write_gitconfig_hooks
  write_systemd_core
  [[ "$piper_en" == "1" ]] && install_piper_artifacts
  [[ "$npm_en" == "1" && -n "$npm_pkgs" ]] && install_npm_extensions "$npm_pkgs"
  if [[ "$trans_en" == "1" ]]; then
    install_translation_artifacts $trans_pkgs
  fi
  refresh_features_inventory

  touch "$INSTALL/relay.env"
  chown relay:relay "$INSTALL/relay.env"
  append_relay_env_public_host
  append_relay_env_server_id
  append_relay_env_peer_list
  systemctl enable relay-server relay-git-daemon
  systemctl restart relay-git-daemon relay-server
  [[ "$piper_en" == "1" ]] && systemctl restart relay-tts-piper 2>/dev/null || true

  maybe_ufw
  log "Install complete. HTTP :$HTTP_PORT  git :$GIT_PORT  config: GET /api/config"
}

do_update() {
  need_root
  bootstrap_features_json_if_missing
  [[ -f "$FEATURES_JSON" ]] || die "run install first (or ensure $INSTALL/bin/relay-server exists)"
  install_binaries
  refresh_features_inventory
  systemctl restart relay-git-daemon relay-server
  systemctl try-restart relay-tts-piper 2>/dev/null || true
  log "Binaries updated"
}

do_repair() {
  need_root
  bootstrap_features_json_if_missing
  [[ -f "$FEATURES_JSON" ]] || die "run install first, or deploy binaries to $INSTALL/bin first"
  ensure_base_deps
  ensure_user_relay
  local piper_en npm_en trans_en
  piper_en="$(jq -r '.features.piper_tts.enabled // false' "$FEATURES_JSON" 2>/dev/null || echo false)"
  npm_en="$(jq -r '.features.npm_extensions.enabled // false' "$FEATURES_JSON" 2>/dev/null || echo false)"
  trans_en="$(jq -r '.features.text_translation.enabled // false' "$FEATURES_JSON" 2>/dev/null || echo false)"
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
  if [[ "$trans_en" == "true" ]]; then
    if [[ ! -x "$INSTALL/lib/argos-venv/bin/python3" ]]; then
      local apks
      apks="$(jq -r '.features.text_translation.install_argos_packages | join(" ")' "$FEATURES_JSON" 2>/dev/null || true)"
      install_translation_artifacts $apks
    fi
  fi
  refresh_features_inventory
  systemctl restart relay-git-daemon relay-server
  maybe_ufw
  log "Repair complete"
}

do_reconfigure_features() {
  need_root
  bootstrap_features_json_if_missing
  [[ -f "$FEATURES_JSON" ]] || die "run install first, or deploy binaries to $INSTALL/bin first"
  log "Reconfigure will replace feature installs. Continue? (features are only changed via this script)"
  if [[ "${RELAY_INSTALL_NONINTERACTIVE:-}" != "1" ]] && [[ -t 0 ]]; then
    read -r -p "[y/N] " c
    [[ "${c,,}" == "y" ]] || exit 0
  fi
  { IFS= read -r -d '' piper_en && IFS= read -r -d '' npm_en && IFS= read -r -d '' npm_pkgs && IFS= read -r -d '' trans_en && IFS= read -r -d '' trans_pkgs; } < <(prompt_features)
  local npm_json trans_json
  npm_json="$(pkgs_to_json_array "$npm_pkgs")"
  trans_json="$(pkgs_to_json_array "$trans_pkgs")"
  systemctl stop relay-tts-piper 2>/dev/null || true
  write_features_json "$piper_en" "$npm_en" "$npm_json" "$trans_en" "$trans_json"
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
  if [[ "$trans_en" == "1" ]]; then
    rm -rf "$INSTALL/lib/argos-venv" 2>/dev/null || true
    install_translation_artifacts $trans_pkgs
  else
    rm -rf "$INSTALL/lib/argos-venv" 2>/dev/null || true
  fi
  refresh_features_inventory
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

do_refresh_features() {
  need_root
  bootstrap_features_json_if_missing
  [[ -f "$FEATURES_JSON" ]] || die "run install first (no $FEATURES_JSON)"
  refresh_features_inventory
  systemctl try-restart relay-server 2>/dev/null || true
  log "Feature inventory refreshed (Piper voices / Argos language pairs → $FEATURES_JSON)"
}

case "${1:-install}" in
  install) shift; do_install "$@" ;;
  update) shift; do_update "$@" ;;
  repair) shift; do_repair "$@" ;;
  reconfigure-features) shift; do_reconfigure_features "$@" ;;
  refresh-features) shift; do_refresh_features "$@" ;;
  -h|--help)
    echo "Usage: $0 {install|update|repair|reconfigure-features|refresh-features}"
    echo "  install   — Vercel DNS first (unless skipped), then base deps; Piper + npm + translation prompts (or RELAY_INSTALL_NONINTERACTIVE=1)"
    echo "  DNS env: RELAY_PUBLIC_FQDN, VERCEL_API_TOKEN, optional VERCEL_TEAM_ID, RELAY_VERCEL_DOMAIN, RELAY_SKIP_VERCEL_DNS=1"
    echo "  Features env: RELAY_FEAT_PIPER, RELAY_FEAT_NPM_PKGS, RELAY_FEAT_TRANSLATION, RELAY_FEAT_TRANSLATION_PKGS (see docs/INSTALL_FEATURES.md)"
    echo "  Node env: RELAY_SERVER_ID, RELAY_PUBLIC_HOSTNAME (if RELAY_SKIP_VERCEL_DNS=1), RELAY_MASTER_PEER_LIST (semicolon-separated peer FQDNs)"
    echo "  update    — refresh relay-server binaries from this directory; rescan feature inventory"
    echo "  repair    — fix perms, reinstall features from state/features.json (bootstraps minimal state if missing)"
    echo "  reconfigure-features — change Piper/npm/translation (only supported way to add/remove optional features)"
    echo "  refresh-features — rescan Piper models + Argos packs into features.json (after adding voices or translation packages)"
    exit 0
    ;;
  *) die "unknown command: ${1:-}; try --help" ;;
esac
