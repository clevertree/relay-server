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
- `src/main.rs` or `src/lib.rs` - Entry point
- `Dockerfile` - Container configuration
- `run-debug.sh` - Local development script
- `run-release.sh` - Production runner

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
