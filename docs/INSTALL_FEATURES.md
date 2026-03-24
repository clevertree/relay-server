# Install-time server features (`features.json`)

Optional Relay capabilities are chosen when you run **`install.sh`** (or **`reconfigure-features`**), stored under **`${RELAY_INSTALL_ROOT:-/opt/relay}/state/features.json`**, and exposed to clients as **`GET /api/config` → `installed_features.manifest`** plus a short **`installed_features.summary`**.

Peers and client apps can poll **`/api/config`** on each node to discover what is **enabled** and which **subfeatures** (voices, language pairs, npm packages, etc.) are present on that server.

- **Manifest** — full structure written by **`install.sh`** (ports, paths, flags, inventories).
- **Summary** — compact view assembled by **`relay-server`** (counts, language lists, package names).

Schemas use **`schema_version`** (currently **2**). Older nodes may still serve **`1`** until they run **`reconfigure-features`** or a fresh **`install`**.

---

## Commands (installer)

| Command | Purpose |
|---------|--------|
| `sudo ./install.sh install` | First install; interactive prompts unless `RELAY_INSTALL_NONINTERACTIVE=1`. |
| `sudo ./install.sh reconfigure-features` | Add/remove optional features; reinstalls Piper / Argos venv / npm per answers. |
| `sudo ./install.sh refresh-features` | **Rescan inventories only** (Piper `*.onnx` voices, Argos installed packs) into `features.json` — use after you add Piper models or `argospm install …` without changing enabled flags. |
| `sudo ./install.sh repair` | Reapply binaries, systemd, and features **from existing** `features.json`. |
| `sudo ./install.sh update` | Replace **`relay-server`** / **`relay-hook-handler`** from the tarball directory; refreshes inventories. |

See also **[DEPLOY_LINODE.md](./DEPLOY_LINODE.md)** for DNS, tarball download, and non-interactive examples.

---

## Core (always on)

Recorded under **`features.core`** for discoverability:

| Subfeature | Meaning |
|------------|--------|
| `relay_server` | HTTP API (`RELAY_HTTP_PORT`, default **8080**). |
| `relay_git_daemon` | `git daemon` receive-pack (**`RELAY_GIT_PORT`**, default **9418**). |
| `ports` | `{ "http", "git_daemon" }` |

These are not toggled off by the installer today; they define the baseline node.

---

## Optional: Piper TTS (`features.piper_tts`)

Local speech synthesis via **[Piper](https://github.com/rhasspy/piper/)** and **`relay-tts-piper`** (Python HTTP wrapper).

### Subfeatures (inventory)

| Field | Description |
|-------|-------------|
| **`voices`** | List of objects: `id`, `language` (e.g. `en_US`), `voice`, `quality`, `model_file` — derived from **`*.onnx`** files under **`models_dir`**. |
| **`languages`** | Union of language codes inferred from voice ids (BCP‑47 style where models use `ll_CC`). |
| **`default_model`** | Initial model path used by the systemd unit (installer default: **en_US-lessac-medium**). |
| **`http_port`** | Piper HTTP port (default **5590**). |
| **`health_path`**, **`tts_path`** | HTTP paths implemented by `piper-tts-http.py` (`/health`, `/tts`). |

After adding or removing **`.onnx`** models under **`lib/piper/models`**, run **`sudo ./install.sh refresh-features`** so **`voices`** / **`languages`** stay accurate.

### Non-interactive env

| Variable | Values |
|----------|--------|
| **`RELAY_FEAT_PIPER`** | `1` / `0` |

---

## Optional: npm extensions (`features.npm_extensions`)

Installs packages with npm under **`node_extensions/`** for hook tooling (e.g. **`songwalker-js`**).

### Subfeatures

| Field | Description |
|-------|-------------|
| **`packages`** | JSON array of package names installed on this server. |

### Non-interactive env

| Variable | Values |
|----------|--------|
| **`RELAY_FEAT_NPM_PKGS`** | Space-separated list; if set, feature is treated as enabled. |

---

## Optional: offline text translation (`features.text_translation`)

**Goal:** relay users can translate to/from many languages **on the node** (no cloud API) by standardizing on **[Argos Translate](https://www.argosopentech.com/)** in a dedicated venv.

Implementation today:

- **Backend:** `argos_translate_local`
- **Paths:** `venv_dir` / `venv_python` under **`lib/argos-venv`**
- **Packages:** Open-source **translation models** installed with **`argospm`** (ships in the venv after `pip install argostranslate`)

Relay does **not** ship every language pair by default (that would be very large). You choose packs at install time or later via **`argospm install PACKAGE`**, then refresh the manifest.

### Subfeatures (inventory)

| Field | Description |
|-------|-------------|
| **`install_argos_packages`** | Package ids passed to **`argospm install`** at last **`reconfigure-features`** (e.g. **`translate-en_es`**). |
| **`language_pairs`** | Directed pairs `{ "from", "to", "package" }` detected from Argos after packages are installed. |
| **`from_languages`** | Sorted distinct source codes. |
| **`to_languages`** | Sorted distinct target codes. |

If Argos is enabled but the probe fails (venv missing, import error), **`probe_error`** may appear until **`repair`** / **`reconfigure-features`** fixes the install.

### Operations

1. Enable during **`install`** or **`reconfigure-features`**, optionally listing **`translate-xx_yy`** style ids.
2. To add more pairs later (as **`relay`** or root):

   ```bash
   sudo -u relay /opt/relay/lib/argos-venv/bin/argospm install translate-en_de
   sudo ./install.sh refresh-features
   ```

3. Other nodes learn what is available by reading **`installed_features`** from this host’s **`/api/config`**.

### Non-interactive env

| Variable | Values |
|----------|--------|
| **`RELAY_FEAT_TRANSLATION`** | `1` / `0` |
| **`RELAY_FEAT_TRANSLATION_PKGS`** | Space-separated **`argospm`** package names (optional). |

---

## Federating capability metadata

There is no background mesh sync yet: each peer should call **`GET /api/config`** on the node (with the correct per-repo **`Host`** header if you also need repo context) and merge **`installed_features.summary`** (or the full manifest) into its own client or routing layer.

Recommended client behaviour:

- Prefer nodes that advertise the **`text_translation`** / **`piper_tts`** capability you need.
- When **`voice_count`** or **`language_pair_count`** is zero, treat the feature as “enabled but not provisioned” and fall back to another peer or a local-only path.

---

## Environment quick reference

| Variable | Feature |
|----------|---------|
| `RELAY_FEAT_PIPER=1` | Piper TTS |
| `RELAY_FEAT_NPM_PKGS='pkg …'` | npm extensions |
| `RELAY_FEAT_TRANSLATION=1` | Argos offline translation |
| `RELAY_FEAT_TRANSLATION_PKGS='translate-en_es …'` | Argos packs to install during that run |

Combine with **`RELAY_INSTALL_NONINTERACTIVE=1`** for cloud-init / CI (see **DEPLOY_LINODE.md**).
