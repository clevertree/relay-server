# Relay Per-Repo Database & Sandbox Plan

This document outlines the strategy for migrating the current loose repository indexing into a structured, sandboxed, and per-branch isolated system.

## 1. Current State Analysis

### Finished
- **Unified Config**: `.relay.yaml` controls how hooks are dispatched.
- **Rust Hook Handler**: Native binary intercepts Git pushes and pipes context to Node.
- **Basic Validation Sandbox**: `validation-util.mjs` uses Node's `vm` module to restrict validation logic.
- **Initial Indexing**: `utils.mjs` has `upsertIndex` logic for `meta.yaml` to JSON.

### Missing / Issues
- **OS Exposure**: `utils.mjs` and the main hook scripts (`pre-receive.mjs`, etc.) have full access to `fs`, `path`, and `child_process`.
- **Concurrency & Scaling**: `relay_index.json` is a single file in the repo root. It will struggle with multi-branch scenarios and large datasets.
- **Branch Collision**: Storing all branch data in one file makes partial queries difficult and clutters the index with stale branch data.
- **Untracked Files**: No formal system for hooks to store auxiliary files (IPFS, torrents, logs) that shouldn't be in Git history.

## 2. Proposed Architecture: Per-Branch Databases

### Storage Layout
Each bare repository will maintain a non-Git directory `.relay_data/` at its root.
```text
repository.git/
├── .relay_data/
│   ├── blobs/               # Repository-global content (IPFS, high-res posters)
│   │   └── [sha256_hash]/   # Content-addressed storage
│   └── branches/
│       └── [branch_hash]/   # Use branch name hash to avoid filesystem issues
│           ├── index.db     # Branch-specific PoloDB/SQLite
│           └── ephemeral/   # Branch-local cache/temporary data
```

### Storage Tiers
1. **Branch Tier**: Isolated to the push/ref. Primary storage for indexing metadata.
2. **Repo Tier**: Shared across all branches in a single repository. (Repository Quota enforced here).
3. **Global Tier**: Server-wide storage (e.g., `.relay_data/global_blobs/`). Specifically for IPFS-style content where the same hash should only exist once on the entire server regardless of repo.

## 3. Trusted Runtime (Sandboxed API)

We will remove all Node.js built-in module access. No `import fs`, `path`, or `child_process`.

### The `Relay` Global object
```javascript
const Relay = {
  // Config access
  config: {
    get: (key) => any,               // Access .relay.yaml settings
    db: () => object,                // Pre-parsed db.yaml
    files: () => object,             // Pre-parsed files.yaml
  },
  // Multi-tier storage
  fs: {
    branch: { /* read, write, exists, list */ }, 
    repo: { /* read, write, exists, list */ },
    global: {                        // Dedicated to content-addressed blobs
      get: (hash) => Buffer,
      put: (data) => string,         // Returns hash, handles physical pathing
      exists: (hash) => boolean,
    }
  },
  // Database Access
  db: {
    collection: (name) => ({
      insert: (doc) => void,
      find: (query) => Promise,
      // ... CRUD
    })
  },
  // Git Access (Context-aware)
  git: {
    readFile: (path) => Buffer,      // Reads from current commit
    listChanges: () => Array,        // Replaces listChanged utility
    verifySignature: () => boolean,  // Offloads to Rust verify-commit
  },
  // Utilities
  utils: {
    matchPath: (pattern, path) => boolean, // For files.yaml validation
    parseYaml: (buf) => object,
  }
};
```

## 4. Quota and Accounting
- **Repository Quota**: Configured in `.relay.yaml`.
- **Double-Counting**: If a file is shared/referenced by multiple repositories in the Global Tier, its full size is counted against the quota of *each* repository referencing it.
- **Enforcement**: `Relay.fs.global.put()` will throw a `QuotaExceeded` error if the new blob exceeds the limit.

## 5. Just-in-Time (JIT) Incremental Indexing
To ensure the database is always accurate without requiring expensive massive re-indexes on every push:
1. **State Tracking**: The `index.db` stores the `HEAD_COMMIT_ID` it was last synchronized with.
2. **Incremental Update**: When `QUERY` is called, if `HEAD_COMMIT_ID != current_git_head`:
   - The server identifies the delta: `git rev-list HEAD_COMMIT_ID..current_git_head`.
   - Each missing commit is processed in chronological order.
   - The indexing hook performs `upsert` operations on the database.
3. **Blocking**: The `QUERY` method blocks until the delta is fully processed to ensure data consistency.

## 6. Blob Lifecycle & IPFS Integration

### IPFS Tracking (`ipfs.yaml`)
A repo can include `ipfs.yaml` to define which database fields contain IPFS CIDs.
```yaml
# ipfs.yaml
collections:
  movies:
    - field: "torrent_cid"
      type: "cid-v1"
```

### State Watcher Hook
When a database row is inserted, updated, or deleted, an internal "Blob Watcher" is triggered:
1. **Change Detection**: Compares old and new values of tracked IPFS fields.
2. **Add/Pin**: New CIDs are automatically passed to the server's IPFS daemon.
3. **Removal/Unpin**: CIDs no longer referenced in any branch index are unpinned.
4. **Resiliency**: If the IPFS daemon is unreachable (e.g., local docker), the operations fail silently with a log warning.
5. **Rebuild Sync**: If the database is completely rebuilt, the watcher performs a full diff of "current set of CIDs" vs "daemon pinned set" to sync state.

## 6. Execution Plan

1. **Step 1**: Create a `RelayHost.mjs` wrapper that sets up the full `vm` environment with the `Relay` object.
2. **Step 2**: Update `RelayConfig` to include a dedicated `index` hook type.
3. **Step 3**: Rewrite `utils.mjs` to be a pure consumer of the `Relay` global.
4. **Step 4**: Implement the "stale check" logic in `relay-server` Rust code for the `QUERY` endpoint.
