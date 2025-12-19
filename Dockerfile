# Multi-stage build for relay-server

FROM rust:1.83-slim AS builder
WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev ca-certificates build-essential && rm -rf /var/lib/apt/lists/* \
    && rustup toolchain install nightly

# Cache dependencies
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN RUSTUP_TOOLCHAIN=nightly cargo build --release

FROM debian:bookworm-slim
ARG DEFAULT_REPOS="https://github.com/clevertree/relay-template"
LABEL org.opencontainers.image.source="${DEFAULT_REPOS}"
WORKDIR /srv/relay

# Runtime deps (no Node build tooling; expect static assets to be prebuilt and copied in/out of the image)
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    git \
    curl \
    xz-utils \
    && rm -rf /var/lib/apt/lists/*

# Copy server binary
COPY --from=builder /app/target/release/relay-server /usr/local/bin/relay-server

# Create default dirs
RUN mkdir -p /srv/relay/data /srv/relay/www /srv/relay/prebuilt

# Copy prebuilt web dist (if built via GitHub Actions or local build)
# The .dockerignore or build context determines if dist-web/ exists
COPY --chown=root:root dist-web /srv/relay/prebuilt

# Default envs (can override at runtime)
ENV RELAY_REPO_PATH=/srv/relay/data \
    RELAY_STATIC_DIR=/srv/relay/www \
    RELAY_HTTP_PORT=8080 \
    DEFAULT_REPOS=${DEFAULT_REPOS}

EXPOSE 8080

# Entrypoint script handles clone/build/pull loop then execs server
COPY docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
RUN chmod +x /usr/local/bin/docker-entrypoint.sh

ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]

