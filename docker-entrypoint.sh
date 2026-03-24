#!/usr/bin/env bash
set -euxo pipefail

log() {
  echo "[entrypoint] $*" >&2
}

# Parse repo list (comma/semicolon/whitespace separated)
# Support both DEFAULT_REPOS and RELAY_MASTER_REPO_LIST
REPOS_STR="${RELAY_MASTER_REPO_LIST:-${DEFAULT_REPOS:-https://github.com/clevertree/relay-template}}"
REPOS=$(echo "$REPOS_STR" | tr ',;' ' ')

DATA_ROOT="${RELAY_REPO_PATH:-/srv/relay/data}"
WWW_DIR="${RELAY_STATIC_DIR:-/srv/relay/www}"
mkdir -p "$DATA_ROOT" "$WWW_DIR"

clone_or_update_repo() {
  local url="$1"
  local name
  name=$(basename -s .git "${url}")
  local dir="$DATA_ROOT/${name}.git"

  if [[ -d "$dir" ]]; then
    log "Repo exists, skipping clone: $name"
  else
    log "Cloning $url -> $dir (bare)"
    git clone --bare "$url" "$dir"
  fi
}

copy_static_if_present() {
  local repo_dir="$1"
  local www_dir="$WWW_DIR"
  local prebuilt_dir="/srv/relay/prebuilt"

  # 1. Try to extract from bare repo (HEAD:dist or HEAD:packages/web/dist)
  local paths=("dist" "packages/web/dist" "web/dist")
  for p in "${paths[@]}"; do
    if git --git-dir="$repo_dir" rev-parse --verify "HEAD:$p" >/dev/null 2>&1; then
      log "Extracting static assets from $repo_dir ($p) to $www_dir"
      rm -rf "$www_dir"/*
      git --git-dir="$repo_dir" archive HEAD:"$p" | tar -x -C "$www_dir"
      return 0
    fi
  done

  # 2. Fallback to prebuilt assets in the image if www is empty
  if [[ -d "$prebuilt_dir" ]] && [[ -n "$(ls -A "$prebuilt_dir" 2>/dev/null)" ]]; then
    if [[ ! -n "$(ls -A "$www_dir" 2>/dev/null)" ]]; then
      log "Using prebuilt static assets from $prebuilt_dir"
      cp -r "$prebuilt_dir"/. "$www_dir"/
      return 0
    fi
  fi

  log "No static assets found in $repo_dir or $prebuilt_dir"
  return 1
}

# Initial clone + build for all repos
for repo in $REPOS; do
  clone_or_update_repo "$repo"
  name=$(basename -s .git "${repo}")
  copy_static_if_present "$DATA_ROOT/${name}.git" || true
done

# Background hourly pull + rebuild if present
(
  while true; do
    for repo in $REPOS; do
      name=$(basename -s .git "${repo}")
      dir="$DATA_ROOT/${name}.git"
      if [[ -d "$dir" ]]; then
        log "Pulling latest for $name (bare repo)"
        git -C "$dir" fetch origin +refs/heads/*:refs/heads/* || log "WARN: git fetch failed for $name"
        copy_static_if_present "$dir" || true
      fi
    done
    sleep 3600
  done
) &

# Start git daemon
log "Starting git daemon on port ${RELAY_GIT_PORT:-9418}"
git daemon --reuseaddr --base-path="$DATA_ROOT" --export-all --enable=receive-pack --port="${RELAY_GIT_PORT:-9418}" "$DATA_ROOT" &

# Exec the server (inherits RELAY_* env)
# Start the relay server in background if we're doing DNS/SSL logic
# --- Vercel DNS registration (requires VERCEL_API_TOKEN) ---
VERCEL_API_TOKEN_ENV=${VERCEL_API_TOKEN:-}
VERCEL_DOMAIN=${RELAY_DNS_DOMAIN:-relaynet.online}
VERCEL_SUBDOMAIN=${RELAY_DNS_SUBDOMAIN:-node1}
FQDN="${VERCEL_SUBDOMAIN}.${VERCEL_DOMAIN}"

get_public_ip() {
  for url in "https://api.ipify.org" "https://ipv4.icanhazip.com" "https://ifconfig.me/ip"; do
    ip=$(curl -fsS "$url" | tr -d '\r' | tr -d '\n' || true)
    if [[ "$ip" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
      echo "$ip"
      return 0
    fi
  done
  return 1
}

vercel_dns_upsert() {
  local domain="$1" name="$2" value="$3" type="${4:-A}" ttl="${5:-60}"
  local auth_header="Authorization: Bearer ${VERCEL_API_TOKEN_ENV}"
  local base="https://api.vercel.com"
  local team_q=""
  [[ -n "${VERCEL_TEAM_ID:-}" ]] && team_q="?teamId=${VERCEL_TEAM_ID}"

  # List existing records (first query param must be ?teamId= or ?name=)
  local list_url
  if [[ -n "${VERCEL_TEAM_ID:-}" ]]; then
    list_url="${base}/v4/domains/${domain}/records?teamId=${VERCEL_TEAM_ID}&name=${name}&type=${type}"
  else
    list_url="${base}/v4/domains/${domain}/records?name=${name}&type=${type}"
  fi
  local list_raw=$(curl -sS -H "$auth_header" "$list_url" || true)
  local rec_id=$(echo "$list_raw" | jq -r '.records[] | select(.name=="'"$name"'" and .type=="'"$type"'" ) | .id // .uid' | head -n1 || true)

  if [[ -n "$rec_id" && "$rec_id" != "null" ]]; then
    local patch_url="${base}/v4/domains/${domain}/records/${rec_id}${team_q}"
    local body=$(jq -n --arg v "$value" --argjson t $ttl '{ value: $v, ttl: $t }')
    curl -sS -X PATCH -H "$auth_header" -H 'Content-Type: application/json' -d "$body" "$patch_url" >/dev/null && return 0
  else
    local create_url="${base}/v4/domains/${domain}/records${team_q}"
    local body=$(jq -n --arg n "$name" --arg v "$value" --arg t "$type" --argjson ttl $ttl '{ name: $n, value: $v, type: $t, ttl: $ttl }')
    curl -sS -X POST -H "$auth_header" -H 'Content-Type: application/json' -d "$body" "$create_url" >/dev/null && return 0
  fi
  return 1
}

if [[ -n "$VERCEL_API_TOKEN_ENV" ]]; then
  log "Attempting DNS upsert for ${FQDN}"
  PUB_IP=$(get_public_ip || true)
  if [[ -n "$PUB_IP" ]]; then
    if vercel_dns_upsert "$VERCEL_DOMAIN" "$VERCEL_SUBDOMAIN" "$PUB_IP"; then
      log "DNS upsert successful: ${FQDN} -> ${PUB_IP}"
    else
      log "WARN: DNS upsert failed"
    fi
  fi
fi

# Start the server (foreground if no SSL, background if we need certbot)
SSL_MODE=${RELAY_SSL_MODE:-auto}
if [[ "$SSL_MODE" != "none" && -n "${RELAY_CERTBOT_EMAIL:-}" && -n "${FQDN:-}" ]]; then
  log "Starting certbot flow for ${FQDN}"
  # Start server in background first so it can serve ACME challenges
  /usr/local/bin/relay-server serve &
  RELAY_PID=$!
  sleep 5
  
  if certbot certonly --webroot -w "${RELAY_ACME_DIR:-/var/www/certbot}" -d "${FQDN}" -m "${RELAY_CERTBOT_EMAIL}" --agree-tos --non-interactive; then
    log "Certbot successful"
    export RELAY_TLS_CERT="/etc/letsencrypt/live/${FQDN}/fullchain.pem"
    export RELAY_TLS_KEY="/etc/letsencrypt/live/${FQDN}/privkey.pem"
    kill $RELAY_PID
    sleep 2
    exec /usr/local/bin/relay-server serve
  else
    log "Certbot failed, falling back to HTTP"
    wait $RELAY_PID
  fi
else
  exec /usr/local/bin/relay-server serve
fi
