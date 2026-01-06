import vm from 'node:vm';
import fs from 'node:fs';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { execSync } from 'node:child_process';

/**
 * RelayHost.mjs
 * 
 * Target environment for sandboxed Relay server hooks.
 * This script is responsible for:
 * 1. Reading the piped context from Rust.
 * 2. Setting up the restricted 'Relay' global.
 * 3. Executing the repository hook in a VM sandbox.
 */
async function main() {
    const scriptPath = process.argv[2];
    if (!scriptPath) {
        console.error('Usage: node RelayHost.mjs <script_path>');
        process.exit(1);
    }

    // 1. Read context from stdin
    let context;
    try {
        const stdinBuffer = fs.readFileSync(0);
        context = JSON.parse(stdinBuffer.toString('utf8'));
    } catch (e) {
        console.error('[RelayHost] Failed to read context from stdin:', e.message);
        process.exit(1);
    }

    const {
        old_commit,
        new_commit,
        refname,
        branch,
        files: contextFiles,
        repo_path,
        is_verified // Passed from Rust
    } = context;

    const gitDir = process.env.GIT_DIR || repo_path;
    const relayDataDir = path.join(gitDir, '.relay_data');
    const branchHash = Buffer.from(branch || 'main').toString('hex').slice(0, 12);
    const branchDir = path.join(relayDataDir, 'branches', branchHash);
    const repoBlobsDir = path.join(relayDataDir, 'blobs');

    // Ensure basic directories exist
    [relayDataDir, path.join(relayDataDir, 'branches'), branchDir, repoBlobsDir].forEach(d => {
        if (!fs.existsSync(d)) fs.mkdirSync(d, { recursive: true });
    });

    const dbPath = path.join(branchDir, 'index.db.json');
    const loadDb = () => {
        if (!fs.existsSync(dbPath)) return { collections: {} };
        try { return JSON.parse(fs.readFileSync(dbPath, 'utf8')); }
        catch { return { collections: {} }; }
    };
    const saveDb = (db) => fs.writeFileSync(dbPath, JSON.stringify(db, null, 2));

    // Initialize metadata
    const db = loadDb();
    if (!db.metadata) db.metadata = { indexed_head: null };
    saveDb(db);

    // 2. Define the Relay global
    const Relay = {
        db: {
            collection: (name) => {
                const triggerWatcher = (doc) => {
                    const ipfsBuf = Relay.git.readFile('ipfs.yaml');
                    if (!ipfsBuf) return;
                    const ipfsConfig = Relay.utils.parseYaml(ipfsBuf);
                    if (!ipfsConfig || !ipfsConfig.collections) return;
                    const fieldConfigs = ipfsConfig.collections[name];
                    if (!fieldConfigs) return;
                    fieldConfigs.forEach(cfg => {
                        const cid = doc[cfg.field];
                        if (cid && typeof cid === 'string' && (cid.startsWith('Qm') || cid.startsWith('ba'))) {
                            try { execSync(`ipfs pin add -q ${cid}`, { stdio: 'ignore' }); } catch { }
                        }
                    });
                };
                return {
                    insert: (doc) => {
                        const db = loadDb();
                        if (!db.collections[name]) db.collections[name] = [];
                        const newDoc = { ...doc, _id: Date.now() + Math.random() };
                        db.collections[name].push(newDoc);
                        saveDb(db);
                        triggerWatcher(newDoc);
                    },
                    update: (query, update) => {
                        const db = loadDb();
                        const items = db.collections[name] || [];
                        let count = 0;
                        items.forEach(item => {
                            let match = true;
                            for (let k in query) if (item[k] !== query[k]) { match = false; break; }
                            if (match) {
                                Object.assign(item, update);
                                triggerWatcher(item);
                                count++;
                            }
                        });
                        if (count > 0) saveDb(db);
                        return count;
                    },
                    remove: (query) => {
                        const db = loadDb();
                        const items = db.collections[name] || [];
                        const initialLen = items.length;
                        db.collections[name] = items.filter(item => {
                            for (let k in query) if (item[k] !== query[k]) return true;
                            return false;
                        });
                        if (db.collections[name].length !== initialLen) saveDb(db);
                        return initialLen - db.collections[name].length;
                    },
                    find: (query) => {
                        const db = loadDb();
                        const items = db.collections[name] || [];
                        if (!query) return items;
                        return items.filter(item => {
                            for (let k in query) if (item[k] !== query[k]) return false;
                            return true;
                        });
                    }
                };
            }
        },
        config: {
            get: (key) => context[key]
        },
        fs: {
            branch: {
                read: (p) => fs.readFileSync(path.join(branchDir, p)),
                write: (p, data) => {
                    const full = path.join(branchDir, p);
                    fs.mkdirSync(path.dirname(full), { recursive: true });
                    fs.writeFileSync(full, data);
                },
                exists: (p) => fs.existsSync(path.join(branchDir, p)),
                unlink: (p) => fs.unlinkSync(path.join(branchDir, p)),
            },
            repo: {
                read: (p) => fs.readFileSync(path.join(repoBlobsDir, p)),
                write: (p, data) => {
                    const full = path.join(repoBlobsDir, p);
                    fs.mkdirSync(path.dirname(full), { recursive: true });
                    fs.writeFileSync(full, data);
                },
                exists: (p) => fs.existsSync(path.join(repoBlobsDir, p)),
            },
            global: {
                get: (hash) => {
                    const p = path.join(relayDataDir, '..', 'global_blobs', hash);
                    return fs.existsSync(p) ? fs.readFileSync(p) : null;
                },
                put: (data) => {
                    const hash = createHash('sha256').update(data).digest('hex');
                    const globalDir = path.join(relayDataDir, '..', 'global_blobs');
                    if (!fs.existsSync(globalDir)) fs.mkdirSync(globalDir, { recursive: true });
                    fs.writeFileSync(path.join(globalDir, hash), data);
                    try { execSync(`ipfs add -q ${path.join(globalDir, hash)}`, { stdio: 'ignore' }); } catch { }
                    return hash;
                }
            }
        },
        git: {
            readFile: (p) => {
                if (contextFiles && contextFiles[p]) {
                    return Buffer.from(contextFiles[p], 'base64');
                }
                // Fallback to git show for files not in context (e.g. during JIT re-indexing)
                try {
                    return execSync(`git -C "${gitDir}" show "${new_commit}:${p}"`, { stdio: ['ignore', 'pipe', 'ignore'] });
                } catch {
                    return null;
                }
            },
            listChanges: () => {
                try {
                    if (old_commit && old_commit !== '0000000000000000000000000000000000000000') {
                        const out = execSync(`git -C "${gitDir}" diff --name-status "${old_commit}" "${new_commit}"`, { encoding: 'utf8' });
                        return out.trim().split('\n').filter(Boolean).map(line => {
                            const [status, path] = line.split(/\s+/);
                            return { status: status[0], path };
                        });
                    } else {
                        // For full re-indexing, return all files in the tree
                        const out = execSync(`git -C "${gitDir}" ls-tree -r --name-only "${new_commit}"`, { encoding: 'utf8' });
                        return out.trim().split('\n').filter(Boolean).map(p => ({
                            status: 'A',
                            path: p
                        }));
                    }
                } catch (e) {
                    // Fallback to context if git command fails (might happen in shared/bare environments)
                    return Object.keys(contextFiles || {}).map(p => ({
                        path: p,
                        status: 'M'
                    }));
                }
            },
            verifySignature: () => !!is_verified
        },
        utils: {
            env: (name, def) => {
                const configVal = Relay.config.get(name.toLowerCase());
                if (configVal !== undefined) return configVal;
                const v = process.env[name];
                if (v == null) return def;
                return v;
            },
            listChanged: () => Relay.git.listChanges(),
            readFromTree: (p) => Relay.git.readFile(p),
            parseYaml: (buf) => {
                if (!buf) return null;
                const text = buf.toString('utf8');
                const obj = {};
                text.split('\n').forEach(line => {
                    const parts = line.split(':');
                    if (parts.length >= 2) {
                        const key = parts[0].trim();
                        const val = parts.slice(1).join(':').trim();
                        if (val.startsWith('"') && val.endsWith('"')) obj[key] = val.slice(1, -1);
                        else if (val === 'true') obj[key] = true;
                        else if (val === 'false') obj[key] = false;
                        else if (!isNaN(val) && val !== '') obj[key] = Number(val);
                        else obj[key] = val;
                    }
                });
                return obj;
            },
            upsertIndex: (changes, readFileFn, branch = 'main') => {
                const collection = Relay.db.collection('index');
                for (const ch of changes) {
                    if (ch.path.endsWith('meta.yaml') || ch.path.endsWith('meta.yml')) {
                        const metaDir = ch.path.split('/').slice(0, -1).join('/') || '.';
                        collection.remove({ _meta_dir: metaDir });
                        if (ch.status === 'D') continue;
                        const buf = readFileFn(ch.path);
                        if (!buf) continue;
                        const json = Relay.utils.parseYaml(buf);
                        if (!json) continue;
                        const doc = {
                            ...json,
                            _branch: branch,
                            _meta_dir: metaDir,
                            _updated_at: new Date().toISOString()
                        };
                        collection.insert(doc);
                    }
                }
            },
            matchPath: (pattern, p) => {
                const regex = new RegExp('^' + pattern
                    .replace(/\./g, '\\.')
                    .replace(/\*\*\//g, '(.+/)?')
                    .replace(/\*\*/g, '.*')
                    .replace(/\*/g, '[^/]*') + '$');
                return regex.test(p);
            },
            runValidation: (code, changes) => {
                const api = {
                    listStaged: () => changes,
                    readFile: (p) => Relay.git.readFile(p)
                };
                const innerSandbox = { api, console, Buffer };
                const innerContext = vm.createContext(innerSandbox);
                const script = new vm.Script(`${code}\nvalidate(api);`, { filename: 'validation.mjs' });
                return script.runInContext(innerContext);
            }
        }
    };

    // 4. Setup VM
    const sandbox = {
        Relay,
        // Inject utilities directly for convenience and ESM stripping support
        env: Relay.utils.env,
        listChanged: Relay.utils.listChanged,
        readFromTree: Relay.git.readFile,
        upsertIndex: Relay.utils.upsertIndex,
        yamlToJson: Relay.utils.parseYaml,
        verifyCommit: Relay.git.verifySignature,

        Buffer,
        console,
        process: {
            exit: (code) => process.exit(code),
            env: {
                OLD_COMMIT: old_commit,
                NEW_COMMIT: new_commit,
                REFNAME: refname,
                BRANCH: branch,
                GIT_DIR: gitDir
            }
        },
        setTimeout,
        clearTimeout
    };
    const contextObj = vm.createContext(sandbox);

    try {
        let code = fs.readFileSync(scriptPath, 'utf8');
        // Strip shebang
        if (code.startsWith('#!')) {
            code = code.split('\n').slice(1).join('\n');
        }

        // Strip ESM imports/exports for simple vm.Script compatibility
        code = code.replace(/^import\s+.*?\s+from\s+['"].*?['"];?\s*$/gm, '// Stripped import');
        code = code.replace(/^export\s+default\s+/gm, '');
        code = code.replace(/^export\s+const\s+/gm, 'const ');
        code = code.replace(/^export\s+function\s+/gm, 'function ');

        const script = new vm.Script(`(async () => { 
             try {
                ${code}
             } catch (e) {
                console.error('[RelayHost] Error during script execution:', e.message);
                console.error(e.stack);
                process.exit(1);
             }
         })()`, { filename: scriptPath });

        await script.runInContext(contextObj);

        // Update indexed_head after successful hook execution (if it was an indexing hook)
        // Note: For pre-receive, we don't necessarily want to update indexed_head yet, 
        // as the query might still see stale data until the push is finalized.
        // But for JIT indexing (where new_commit is the actual HEAD), we MUST update it.
        const finalDb = loadDb();
        finalDb.metadata.indexed_head = new_commit;
        saveDb(finalDb);

    } catch (e) {
        console.error(`[RelayHost] Compilation/Setup Error in ${scriptPath}:`, e.message);
        if (e.stack) console.error(e.stack);
        process.exit(1);
    }
}

main();
