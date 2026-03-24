# Server-Side Hooks and Repository Configuration

Relay repositories are governed by a unified configuration file `.relay.yaml` located at the root of the repository. This file defines how the repository behaves on the server, including hook dispatching, branch protection rules, and P2P synchronization.

## Trust, authorized servers, and bootstrap (normative)

**Minimum server launch:** **`RELAY_SERVER_ID`** + **`RELAY_AUTHORIZED_REPOS_PATH`** (repo list + **`anchor_commit`** per repo). See **[`RELAY_TRUST_AND_BOOTSTRAP.md`](./RELAY_TRUST_AND_BOOTSTRAP.md)**.

| Concern | Mechanism |
|---------|-----------|
| **Pull validation** | Only repos listed in **`authorized-repos.yaml`**; after fetch, **`anchor_commit`** must be ancestor of branch tip. |
| Who may push **content** | Signed commits + **`allowedKeys`** / **`allowedKeyFingerprints`**; server hooks validate every change. |
| Which **Relay nodes** may participate | **`git.relayTrust.authorizedServerIds`**; **`autoPush.originList`** aligned with that list. |
| New nodes | **`RELAY_SERVER_ID`** + **`relay-bootstrap.sh`** + same **authorized-repos** policy. |

Until **`allowedKeys` / fingerprint matching** is fully enforced in Rust, combine **`requireSigned: true`** with strict firewall rules on **9418**.

## Configuration Schema (`.relay.yaml`)

```yaml
name: "My Repository"
version: "1.0.0"
description: "A description of the repository"

# Client-side JSX hook mapping
client:
  hooks:
    get:
      path: hooks/client/get-client.jsx
    query:
      path: hooks/client/query-client.jsx

# Server-side Node.js hook mapping
server:
  hooks:
    pre-commit:
      path: hooks/server/pre-commit.mjs
    pre-receive:
      path: hooks/server/pre-receive.mjs

# Git-level infrastructure settings (Rust-enforced)
git:
  # P2P Synchronization between peers
  autoPush:
    branches: [ "main" ] # Branches to sync on successful push
    originList:
      - "peer1.relay.online"
      - "peer2.relay.online"
    debounceSeconds: 2
  
  # Native Branch Protection Rules
  branchRules:
    default:
      requireSigned: true
      allowedKeys: [ ".ssh/*" ]
    branches:
      - name: main
        rule:
          requireSigned: true
          allowedKeys: [ ".ssh/admin.pub" ]
      - name: public
        rule:
          allowUnsigned: true

  # GitHub Integration
  github:
    enabled: true
    path: "/hooks/github/{repo}" # Documentary; server route is POST `/hooks/github/:repo`
    events: [ "push" ]
```

## Hook Flow

### 1. Pre-Commit Hook (Server `PUT`)
When a file is uploaded via the Relay `PUT` API:
1.  The server checks `.relay.yaml` for `server.hooks.pre-commit`.
2.  If found, it executes the specified Node.js script.
3.  The script receives a JSON context via `stdin` containing the proposed changes.
4.  If the script exits with non-zero, the commit is rejected.

### 2. Pre-Receive Hook (`git push`)
When a commit is pushed via the Git protocol:
1.  The native `relay-hook-handler` binary is triggered.
2.  It reads `.relay.yaml` from the **new** commit being pushed.
3.  **Native Rules**: It enforces `branchRules` (e.g., signature verification) natively in Rust.
4.  **Legacy Dispatch**: It then executes the Node.js `pre-receive` script if configured.
5.  If any step fails, the push is rejected.

### 3. Post-Receive Hook (Synchronization)
After a successful push:
1.  The `relay-hook-handler` checks for `git.autoPush`.
2.  It automatically pushes the updated branch to all listed peers.
3.  Circular sync is avoided using the `RELAY_SYNC_IN_PROGRESS` environment variable.

## Validation Sandbox

Relay provides a shared validation sandbox (`.relay/validation.mjs`) that can be used by both `pre-commit` and `pre-receive` to enforce consistent repository rules.

```javascript
// Example validation.mjs
function validate(api) {
  const staged = api.listStaged();
  // ... check paths, contents, schemas ...
  return { ok: true };
}
validate; // Script must return the function
```

## GitHub Webhooks

The server provides a native endpoint for GitHub push webhooks. When enabled in `.relay.yaml`:
1.  Configure GitHub to POST to **`https://{your-server}/hooks/github/{repo}`** where **`{repo}`** is the bare directory name without `.git` (same name as the first label of your per-repo HTTP host if you use **`{repo}.{RELAY_PUBLIC_HOSTNAME}`**).
2.  Signature verification is not wired in this path yet; treat the URL as secret or restrict by network policy.

## Testing Hooks

You can test your server-side hooks without performing a real Git push using the provided test runner in the `relay-template` repository.

### Node.js Integration Test
Run the test simulator:
```bash
cd /home/ari/dev/relay-template
node tests/test_hooks.mjs
```

This script:
1.  Simulates the JSON context piped by Rust.
2.  Loads your `pre-receive.mjs` or `pre-commit.mjs`.
3.  Executes the validation logic in the sandbox.
4.  Reports success or failure with detailed logs.

### Docker Peer Verification
To test the full "Push -> Protect -> Sync" flow:
1.  Ensure you have at least two peer containers running.
2.  Configure `.relay.yaml` with the peer's hostname in `originList`.
3.  Push a commit and check the logs of both peers:
    ```bash
    docker logs relay-peer-1
    docker exec relay-peer-2 git rev-parse HEAD
    ```
3.  The server automatically performs a `git fetch` and update for the target repository.
