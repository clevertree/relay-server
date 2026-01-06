# relay-server Copilot Instructions

## Project Overview
Rust server backend for Relay system. Handles hook execution, plugin coordination, and API requests.

### Tech Stack
- Rust (async)
- Likely: Actix-web or Tokio runtime
- CORS handling for hook requests
- REST/GraphQL API

## Server Responsibilities

### Core Functions
1. **Hook Serving** - Deliver hook source code to clients
2. **Plugin Coordination** - Manage plugin API calls
3. **CORS Management** - Handle cross-origin hook requests
4. **Caching** - Cache transpiled hooks/plugin responses

### Architecture
- Request routing to hook endpoints
- Integration with plugin system (TMDB, YTS, etc.)
- Error handling and logging

## Key Directories
- `src/` - Rust source code
- `src/main.rs` or `src/lib.rs` - API server entry point
- `src/bin/relay-hook-handler.rs` - Native Git hook dispatcher
- `Dockerfile` - Container configuration
- `run-debug.sh` - Local development script
- `run-release.sh` - Production runner

## Git & Hook Infrastructure

### Unified Configuration (`.relay.yaml`)
Repositories are governed by `.relay.yaml` at the root.
- **Client**: JSX/TSX hook mapping for the mobile/web clients.
- **Server**: Node.js validation scripts (`pre-commit`, `pre-receive`).
- **Git**: Infrastructure rules (Branch protection, auto-push, GitHub webhooks).

### Native Hook Dispatcher (`relay-hook-handler`)
A dedicated Rust binary that acts as the entry point for Git hooks.
1. **Config Resolution**: Fetches `.relay.yaml` directly from the Git object database (works during `pre-receive`).
2. **Native Enforcement**: Checks branch rules (e.g., SSH commit signatures) before calling any scripts.
3. **JS Sandbox Integration**: Pipes commit metadata and file blobs as JSON over `stdin` to Node.js hooks.
4. **Resiliency**: If Node.js is missing or scripts fail to parse, infrastructure rules (like signature checks) still apply.

### Peer Synchronization (Auto-Push)
- Triggered by `post-receive` via `git.autoPush` config.
- Automatically propagates commits to a list of peers.
- Uses `RELAY_SYNC_IN_PROGRESS=1` environment variable to prevent infinite sync loops between peers.

## Database & Indexing Architecture (Planned)

### Per-Branch Databases
- Databases are isolated per-branch in `.relay_data/branches/[branch]/`.
- Avoids large monolithic indices and concurrency issues.
- Allows "Just-in-Time" indexing: the `QUERY` method triggers an update if the branch HEAD has moved since the last index.

### Trusted Runtime
- Server hooks are denied direct OS access (`fs`, `child_process`, `path`).
- They interact with the host via an injected `Relay` global providing restricted file and DB access.
- Enables safe execution of repository-owned code.

## Development

### Local Dev Server
```bash
cd /home/ari/dev/relay-server
bash run-debug.sh
```

### Building
```bash
cargo build --release
```

### Testing
```bash
cargo test
```

## Plugin Integration

### Plugin API Flow
1. Client requests hook from server
2. Hook calls plugin endpoint (TMDB, YTS, etc.)
3. Server proxies/coordinates plugin API
4. Response cached and returned to client

### Plugin Environment
Plugins need access to API keys/configs:
- Store in `.env` or environment
- Pass to plugin via server context
- Cache plugin responses if possible

## CORS Configuration
Hooks execute cross-origin - ensure:
- `Access-Control-Allow-Origin` headers set correctly
- Preflight requests handled
- Credentials/cookies if needed

### Test CORS
```bash
bash test_cors.sh  # if available
```

## Deployment

### Docker
```dockerfile
# Typical Rust deployment
FROM rust:latest
COPY . /app
WORKDIR /app
RUN cargo build --release
CMD ["./target/release/relay-server"]
```

### Rackspace Deploy
Script provided: `deploy-to-rackspace.sh`

```bash
bash deploy-to-rackspace.sh
```

## Debugging

### Check Server Health
- Verify Rust compilation: `cargo build`
- Check error logs in debug output
- Test endpoints with `curl`

### Common Issues

**CORS errors on client:**
- Check `Access-Control-Allow-Origin` headers
- Verify preflight response (OPTIONS method)

**Plugin API failures:**
- Check API key configuration in environment
- Verify plugin endpoint URL correct
- Check rate limiting (TMDB, YTS have limits)

**Hook execution timeout:**
- May need to cache plugin responses
- Check network latency to plugin services

## Key Files
- `src/main.rs` or `src/lib.rs` - Server logic
- `Cargo.toml` - Dependencies
- `test_cors.sh` - CORS testing
- `run-debug.sh` - Development runner
- `run-release.sh` - Production runner

## Integration with relay-clients

Server provides:
- HTTP endpoint for hook source (`/hooks/client/get-client.jsx`)
- Plugin proxy endpoints (`/api/plugin/tmdb/*`, etc.)
- Environment variables for API keys

relay-clients expects:
- Hook source as plain text (transpiled by client)
- Plugin responses as JSON

## Environment Variables
Likely needed:
- `RUST_LOG` - Log level
- `TMDB_API_KEY` - TMDB integration
- `PORT` - Server port (default 3000 or 5000)
- `ALLOWED_ORIGINS` - CORS whitelist
