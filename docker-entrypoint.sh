#!/usr/bin/env bash
set -euo pipefail

log() {
  echo "[entrypoint] $*" >&2
}

# Parse repo list (comma/semicolon/whitespace separated) from DEFAULT_REPOS only
DEFAULT_REPOS="${DEFAULT_REPOS:-https://github.com/clevertree/relay-template}"
REPOS=$(echo "$DEFAULT_REPOS" | tr ',;' ' ')

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
  local candidates=(
    "$repo_dir/packages/web/dist"
    "$repo_dir/web/dist"
    "$repo_dir/dist"
    "/srv/relay/prebuilt"
  )

  for cand in "${candidates[@]}"; do
    if [[ -d "$cand" ]]; then
      rm -rf "$WWW_DIR"/*
      cp -r "$cand"/. "$WWW_DIR"/
      log "Copied static assets from $cand to $WWW_DIR"
      return
    fi
  done

  log "No static assets found (looked in ${candidates[*]}). Provide prebuilt dist via volume or image."
}

# Initial clone + build for all repos
for repo in $REPOS; do
  clone_or_update_repo "$repo"
  name=$(basename -s .git "${repo}")
  copy_static_if_present "$DATA_ROOT/${name}.git"
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
        copy_static_if_present "$dir"
      fi
    done
    sleep 3600
  done
) &

# Exec the server (inherits RELAY_* env)
exec /usr/local/bin/relay-server serve
