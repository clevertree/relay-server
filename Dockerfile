# Multi-stage build for relay-server using cargo-chef
FROM lukemathwalker/cargo-chef:latest-rust-1.85-slim AS chef
WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev build-essential \
    && rm -rf /var/lib/apt/lists/*

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
# Build dependencies - this is the caching layer
RUN cargo chef cook --release --recipe-path recipe.json

# Build application
COPY . .
RUN cargo build --release --bin relay-server \
    && cargo build --release --bin relay-hook-handler

FROM debian:bookworm-slim
ARG DEFAULT_REPOS="https://github.com/clevertree/relay-template"
LABEL org.opencontainers.image.source="${DEFAULT_REPOS}"

# Create non-root user
RUN groupadd -r relay && useradd -r -g relay -d /srv/relay -s /sbin/nologin relay

WORKDIR /srv/relay

# Runtime deps
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    git \
    curl \
    xz-utils \
    nodejs \
    && rm -rf /var/lib/apt/lists/*

# Copy server binary
COPY --from=builder /app/target/release/relay-server /usr/local/bin/relay-server
COPY --from=builder /app/target/release/relay-hook-handler /usr/local/bin/relay-hook-handler

# Create default dirs and set permissions
RUN mkdir -p /srv/relay/data /srv/relay/www /srv/relay/prebuilt /srv/relay/logs /srv/relay/hooks \
    && chown -R relay:relay /srv/relay

# Symlink universal hooks
RUN ln -s /usr/local/bin/relay-hook-handler /srv/relay/hooks/pre-receive \
    && ln -s /usr/local/bin/relay-hook-handler /srv/relay/hooks/post-receive \
    && ln -s /usr/local/bin/relay-hook-handler /srv/relay/hooks/post-update

# Configure system-wide hooks
RUN git config --system core.hooksPath /srv/relay/hooks

# Copy prebuilt web dist if it exists
COPY --chown=relay:relay dist-web* /srv/relay/prebuilt/

# Default envs
ENV RELAY_REPO_PATH=/srv/relay/data \
    RELAY_STATIC_DIR=/srv/relay/www \
    RELAY_HTTP_PORT=8080 \
    RELAY_GIT_PORT=9418 \
    DEFAULT_REPOS=${DEFAULT_REPOS} \
    RUST_LOG=info

EXPOSE 8080 9418

# Healthcheck
HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:${RELAY_HTTP_PORT}/api/config || exit 1

# Entrypoint script
COPY --chown=relay:relay docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
RUN chmod +x /usr/local/bin/docker-entrypoint.sh

USER relay

ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]

