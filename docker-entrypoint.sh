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
exec /usr/local/bin/relay-server serve
