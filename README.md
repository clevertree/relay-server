# Relay Server (Rust)

Implements the Relay API over bare Git repositories with static directory serving and detailed error reporting.

## Features

- Serves files from multiple bare Git repositories (`.git` suffix)
- Static file serving from configurable directories
- Automatic hourly git pulls for repository updates
- Detailed 404 error messages showing searched paths, repos, and branches
- JSX/TSX transpilation via hook-transpiler WASM
- Optional HTTPS support with automatic certificate loading
- CORS-enabled with custom headers (X-Relay-Repo, X-Relay-Branch)

## Repository Policy

- Never import or reference files directly from `apps/` other than code in this crate; prefer shared assets and data in
  shared crates.

Endpoints

- OPTIONS / and OPTIONS /* — discovery endpoint returning capabilities, branches, repos, current selections, and branch
  HEAD commits (branchHeads). If `X-Relay-Branch` or `X-Relay-Repo` headers (or `?branch=`/`?repo=` query) are sent, the
  response is filtered accordingly.
- GET /{path} — read file at branch/repo (branch via header X-Relay-Branch or query `?branch=...`, repo via header
  X-Relay-Repo or query `?repo=...`).
    - If the path resolves to a directory, returns a markdown listing with breadcrumbs and links.
    - If the file/dir is missing, returns 404 with `text/html` body. If `/site/404.md` exists on that branch, it is
      rendered; otherwise a default page plus a parent directory listing is returned. Global and per-directory CSS are
      auto-linked when present.
- PUT /{path} — write file and commit to selected branch/repo (branch via header or query `?branch=...`; repo via header
  or query `?repo=...`).
    - Commits are validated by any `.relay/pre-commit.mjs` script present in the repository. Rejected commits return
      400/500
      with error text.
- DELETE /{path} — delete file and commit to selected branch/repo (branch via header or query `?branch=...`).
- QUERY * — Custom method for YAML-driven query using the local PoloDB index built by hooks (no POST alias).
    - Pagination defaults: pageSize=25, page=0; can override via request body
    - Header X-Relay-Branch may be a branch name or `all` to query across branches
    - Request body (generic): `{ filter?: object, page?: number, pageSize?: number, sort?: [{ field, dir }] }`
    - Response: `{ total, page, pageSize, items }`

## Environment Variables

### Server Configuration
- `RELAY_REPO_PATH`: Root directory containing bare repos (default: `./data`)
- `RELAY_STATIC_DIR`: Comma-separated static directories to serve (default: none)
- `RELAY_HTTP_PORT`: HTTP port (default: 80)
- `RELAY_HTTPS_PORT`: HTTPS port (default: 443)
- `RELAY_BIND`: Override bind address (format: `host:port`)

### Repository Management
- `RELAY_MASTER_REPO_LIST`: Semicolon-separated list of repos to clone on startup (e.g., `https://github.com/clevertree/relay-template`)
- `DEFAULT_REPOS`: Alternative to RELAY_MASTER_REPO_LIST for Docker entrypoint

### TLS/HTTPS Configuration
- `RELAY_TLS_CERT`: Path to TLS certificate file
- `RELAY_TLS_KEY`: Path to TLS private key file

### ACME Challenge Support
- `RELAY_ACME_DIR`: Directory for ACME HTTP-01 challenges (default: `/var/www/certbot`)

## Headers

- `X-Relay-Repo`: Select repository by name (without `.git` suffix)
- `X-Relay-Branch`: Select branch (default: `main`)

## Env

- Repository rules and validation are defined in `.relay/pre-commit.mjs` and `.relay/pre-receive.mjs` scripts within
  each repository.
- `relay_index.json` is maintained by pre-commit/pre-receive scripts to track metadata changes and build queryable
  indexes.
- Rules are enforced for new commits by the respective hook scripts. Repositories can customize validation by
  implementing these scripts.

Repository Scripts

- `.relay/pre-commit.mjs` — executed by the server during PUT operations to validate file changes before committing.
- `.relay/pre-receive.mjs` — executed during git push operations (if using git-based workflows) to validate commits.
- `.relay/validation.mjs` — optional custom validation logic that can be invoked by pre-commit/pre-receive scripts in a
  sandboxed environment.
- `.relay/lib/*.mjs` — shared utility modules for common validation and index management tasks.

Testing policy

- Tests may clone from the canonical template repository: `https://github.com/clevertree/relay-template/`.
- The template repository includes example `.relay/pre-commit.mjs` and `.relay/pre-receive.mjs` scripts for validation.

## CLI & Run

### 1. Build
```bash
cargo build --release
```

### 2. Run Server
```bash
cargo run --release -- serve [OPTIONS]
```

**Options:**
- `--repo <PATH>`: Bare Git repository root directory (default: from `RELAY_REPO_PATH` env or `./data`)
- `--static <DIR>`: Additional static directory to serve files from (repeatable)
- `--bind <HOST:PORT>`: Bind address (default: from `RELAY_BIND` env or `0.0.0.0:80`)

**Example:**
```bash
RELAY_REPO_PATH=/srv/relay/data \
RELAY_STATIC_DIR=/srv/relay/www \
RELAY_HTTP_PORT=8080 \
cargo run --release -- serve
```

### 3. Docker

**Build:**
```bash
docker build -t relay-server .
```

**Run:**
```bash
docker run -d \
  -p 8080:8080 \
  -e DEFAULT_REPOS="https://github.com/clevertree/relay-template" \
  -e RELAY_HTTP_PORT=8080 \
  relay-server
```

The entrypoint script will:
1. Clone repositories from `DEFAULT_REPOS` (bare) to `/srv/relay/data/<name>.git`
2. Copy prebuilt static assets from `/srv/relay/prebuilt` to `/srv/relay/www`
3. Start hourly git fetch loop in background
4. Start relay-server on specified port

## CLI & Run

1) Build everything
   cargo build --workspace

2) Run server (ensure .relay/pre-commit.mjs is available in your template repo)
   cargo run --manifest-path apps/server/Cargo.toml -- serve

   Options:
   --repo <PATH>        Path to the bare Git repository (default from RELAY_REPO_PATH or ./data/repo.git)
   --static <DIR>       Additional static directory to serve files from (can repeat)
   --bind <HOST:PORT>   Bind address (default from RELAY_BIND or 0.0.0.0:8088)

3) Create an IPFS hash for a built UI directory and seed it locally
   # This runs `ipfs add -r <dir>` and prints the root CID
   cargo run --manifest-path apps/server/Cargo.toml -- ipfs-add template-ui/dist

4) Try requests
    - Root listing with breadcrumbs:
      curl -i "http://localhost:8080/?branch=main"
    - Put a file (validates via .relay/pre-commit.mjs if present):
      curl -i -X PUT "http://localhost:8080/README.md?branch=main" \
      -H "Content-Type: application/octet-stream" \
      --data-binary @-
      Hello Relay!
      [Ctrl+D]
    - Fetch it:
      curl -i "http://localhost:8080/README.md?branch=main"
    - Query (QUERY method):
      curl -i -X QUERY "http://localhost:8080" -H "X-Relay-Branch: main" -H "Content-Type: application/json"
      --data '{"filter":{"title":"Inception"}}'

Serving order (GET requests)

1. Static directories: if `--static` paths are provided, the server will first try to serve the file from these
   directories.
2. Git repository: the server reads from the selected bare repo/branch.

Notes:

- The server never serves `.html`, `.htm`, or `.js` files from the Git repository. HTML/JS must come from static
  directories during development.

Logs

- Structured logs are written to stdout and to rolling daily files under `./logs/server.log*`.
- HTTP request/response spans are included (method, path, status, latency).

## Detailed Error Messages

When a file is not found, the server returns a detailed 404 error showing:

```
Not Found

Path: hooks/client/get-client.jsx
Branch: main
Repo: relay-template

File not found in git repository. No hooks/get.mjs found to handle dynamic routing.
```

This helps developers debug:
- Which repository was searched
- Which branch was checked
- Which path was requested
- What static directories were checked (if no repo selected)

## Container Deployment

The server is designed to run in containers with automatic SSL termination via nginx proxy. The entrypoint script
handles:

1. **Repository Initialization**: Clones template repository if none exists
2. **SSL Certificate Provisioning**: Uses Let's Encrypt when `RELAY_CERTBOT_EMAIL` is set
3. **DNS Registration**: Registers with Vercel DNS when `VERCEL_API_TOKEN` is provided
4. **Nginx Proxy Configuration**: Automatically configures nginx to proxy HTTPS to the relay-server

Example container run with SSL:

```bash
docker run -p 80:80 -p 443:443 \
  -e RELAY_CERTBOT_EMAIL="admin@example.com" \
  -e RELAY_DNS_SUBDOMAIN="node1" \
  -e VERCEL_API_TOKEN="your-token" \
  relay-server
```

The server will be accessible at `https://node-dfw1.relaynet.online` with automatic HTTP to HTTPS redirection.

Developing and deploying a UI for repositories

A. Build/export all HTML/JS/asset files of your UI (e.g., into `template-ui/dist`).
B. Test locally: run the server and add your UI folder as an additional static directory, then exercise the app.

- Example:
  cargo run --manifest-path apps/server/Cargo.toml -- serve --repo ./data/repo.git --static ./template-ui/dist
