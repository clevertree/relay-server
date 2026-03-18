#!/usr/bin/env bash
# Provision bare repos + optional npm extensions from a manifest URL or RELAY_SEED_REPOS.
# Intended for new Relay nodes identified by RELAY_SERVER_ID only.
set -euo pipefail

: "${RELAY_SERVER_ID:?Set RELAY_SERVER_ID (e.g. relay-atlanta2)}"
DATA_ROOT="${RELAY_REPO_PATH:-/opt/relay/data}"
EXT_DIR="${RELAY_NODE_EXTENSIONS_DIR:-/opt/relay/node_extensions}"
ENV_OUT="${RELAY_ENV_FILE:-/opt/relay/relay.env}"
RUN_USER="${RELAY_RUN_USER:-relay}"

mkdir -p "$DATA_ROOT" "$EXT_DIR"
id "$RUN_USER" &>/dev/null || true

log() { echo "[relay-bootstrap] $*" >&2; }

clone_bare() {
  local name="$1" git_url="$2"
  local dest="$DATA_ROOT/${name}.git"
  if [[ -d "$dest" ]]; then
    log "Updating $name"
    sudo -u "$RUN_USER" git -C "$dest" fetch origin '+refs/heads/*:refs/heads/*' 2>/dev/null || \
    sudo -u "$RUN_USER" git -C "$dest" remote add origin "$git_url" 2>/dev/null || true
    sudo -u "$RUN_USER" git -C "$dest" fetch origin '+refs/heads/*:refs/heads/*' || true
  else
    log "Cloning bare $name <- $git_url"
    sudo -u "$RUN_USER" git clone --bare "$git_url" "$dest"
  fi
}

if [[ -n "${RELAY_BOOTSTRAP_MANIFEST_URL:-}" ]]; then
  log "Fetching manifest from RELAY_BOOTSTRAP_MANIFEST_URL"
  tmp="$(mktemp)"
  trap 'rm -f "$tmp"' EXIT
  hdr=()
  [[ -n "${RELAY_BOOTSTRAP_TOKEN:-}" ]] && hdr=(-H "Authorization: Bearer ${RELAY_BOOTSTRAP_TOKEN}")
  curl -fsSL "${hdr[@]}" "$RELAY_BOOTSTRAP_MANIFEST_URL" -o "$tmp"
  command -v jq >/dev/null || { log "install jq"; exit 1; }
  mid="$(jq -r '.relay_server_id // empty' "$tmp")"
  if [[ -n "$mid" && "$mid" != "null" && "$mid" != "$RELAY_SERVER_ID" ]]; then
    log "manifest relay_server_id ($mid) != RELAY_SERVER_ID ($RELAY_SERVER_ID)"
    exit 1
  fi
  n="$(jq '.bareRepos | length' "$tmp")"
  if [[ "$n" -gt 0 ]]; then
    for ((i = 0; i < n; i++)); do
      name="$(jq -r ".bareRepos[$i].name" "$tmp")"
      git_url="$(jq -r ".bareRepos[$i].git" "$tmp")"
      [[ "$name" != "null" && "$git_url" != "null" ]] && clone_bare "$name" "$git_url"
      anchor="$(jq -r ".bareRepos[$i].anchorCommit // empty" "$tmp")"
      br="$(jq -r ".bareRepos[$i].branch // \"main\"" "$tmp")"
      if [[ -n "$anchor" && "$anchor" != "null" && "$name" != "null" ]]; then
        dest="$DATA_ROOT/${name}.git"
        if ! sudo -u "$RUN_USER" git -C "$dest" merge-base --is-ancestor "$anchor" "refs/heads/$br" 2>/dev/null; then
          log "anchor $anchor not ancestor of $name:$br — clone rejected"
          exit 1
        fi
        log "anchor OK $name"
      fi
    done
  fi
  ne="$(jq '.npmExtensions | length' "$tmp")"
  if [[ "${ne:-0}" -gt 0 ]]; then
    log "npm install in $EXT_DIR (requires npm)"
    pkgs="$(jq -r '.npmExtensions | join(" ")' "$tmp")"
    sudo -u "$RUN_USER" bash -c "cd '$EXT_DIR' && (test -f package.json || npm init -y >/dev/null 2>&1) && npm install --no-save $pkgs"
  fi
  jq -r '.relayEnv | to_entries[] | "\(.key)=\(.value)"' "$tmp" 2>/dev/null | tee -a "$ENV_OUT" >/dev/null || true
elif [[ -n "${RELAY_SEED_REPOS:-}" ]]; then
  log "Using RELAY_SEED_REPOS"
  IFS=';'
  for pair in $RELAY_SEED_REPOS; do
    name="${pair%%=*}"
    url="${pair#*=}"
    [[ -n "$name" && -n "$url" ]] && clone_bare "$name" "$url"
  done
else
  echo "Set RELAY_BOOTSTRAP_MANIFEST_URL or RELAY_SEED_REPOS" >&2
  exit 1
fi

grep -q "^RELAY_SERVER_ID=" "$ENV_OUT" 2>/dev/null || echo "RELAY_SERVER_ID=${RELAY_SERVER_ID}" >>"$ENV_OUT"
chown "$RUN_USER:$RUN_USER" "$ENV_OUT" 2>/dev/null || true

log "Done. Restart: systemctl restart relay-server relay-git-daemon"
