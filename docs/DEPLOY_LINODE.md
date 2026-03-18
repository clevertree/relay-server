# Deploy relay-server to Linode (bare metal — no Docker)

## Recommended plan

| Step | What |
|------|------|
| **1. VM** | Linode **Ubuntu 22.04/24.04 LTS**, open **22, 8080, 9418** (and **5590** if Piper). |
| **2. Binaries** | Use a **versioned tarball**, not ad-hoc laptop builds in prod. **Tag** `v1.2.3` → GitHub Actions **[`publish-linode-tarball`](../.github/workflows/publish-linode-tarball.yml)** attaches **`relay-linode-deploy.tgz`** to that **Release**. Or run **`./scripts/pack-linode-deploy.sh`** locally and host the file (S3, etc.). |
| **3. Install** | **`relay-curl.sh`** one-liner with **`RELAY_DEPLOY_TGZ_URL`** pointing at the release asset URL, **or** `scp` the tgz and **`sudo ./install.sh install`**. |
| **4. Trust** | **`authorized-repos.yaml`** + **`relay.env`** (`RELAY_SERVER_ID`, `RELAY_AUTHORIZED_REPOS_PATH`). |
| **5. Data** | **`relay-bootstrap.sh`** (catalog or manifest) **or** bare init + **`git push`** from laptop. |
| **6. Updates** | **Binary upgrade:** new tgz → **`install.sh update`** or **`relay-curl.sh`** with new **`RELAY_DEPLOY_TGZ_URL`**. **Script/hardening only:** **`relay-curl.sh … repair`** (pulls **`relay-install.sh`** from GitHub). |

**Release tarball URL shape (after you push tag `v0.3.1`):**

`https://github.com/clevertree/relay-server/releases/download/v0.3.1/relay-linode-deploy.tgz`

**Manual CI build (no tag):** Actions → **publish-linode-tarball** → **Run workflow** → download **relay-linode-deploy** artifact.

---

## Minimum launch (required)

1. **`RELAY_SERVER_ID`** — unique name (e.g. `relay-atlanta1`).
2. **`RELAY_AUTHORIZED_REPOS_PATH`** — YAML listing allowed repo names + **`anchor_commit`** per repo. See **[`examples/authorized-repos.yaml`](./examples/authorized-repos.yaml)** and **[`RELAY_TRUST_AND_BOOTSTRAP.md`](./RELAY_TRUST_AND_BOOTSTRAP.md)**.

```bash
sudo cp docs/examples/authorized-repos.yaml /opt/relay/authorized-repos.yaml
# Edit anchor_commit to match each repo’s trusted main tip: git -C repo.git rev-parse refs/heads/main
sudo chown relay:relay /opt/relay/authorized-repos.yaml
```

Add to **`/opt/relay/relay.env`**:

```
RELAY_SERVER_ID=relay-atlanta1
RELAY_AUTHORIZED_REPOS_PATH=/opt/relay/authorized-repos.yaml
```

---

## Binary install (`install.sh` = install / update / repair)

The tarball includes **`install.sh`** (`relay-install.sh`), **`piper-tts-http.py`**, binaries, and **`relay-bootstrap.sh`**.

```bash
./scripts/pack-linode-deploy.sh
scp target/relay-linode-deploy.tgz root@IP:/root/
ssh root@IP 'cd /root && tar xzf relay-linode-deploy.tgz && sudo ./install.sh install'
```

| Command | Purpose |
|--------|---------|
| `sudo ./install.sh install` | First-time install (interactive feature prompts: Piper TTS, npm packages). Fails if already installed unless `RELAY_INSTALL_FRESH=1`. |
| `sudo ./install.sh update` | Refresh binaries from tarball dir; restart services. |
| `sudo ./install.sh repair` | Re-apply systemd, hooks, npm extensions, Piper service from **`state/features.json`**. If **`features.json`** is missing but **`/opt/relay/bin/relay-server`** exists, a minimal state file is created (Piper/npm off) so repair/update work on manually seeded hosts. |
| `sudo ./install.sh reconfigure-features` | Re-run feature prompts and rewrite **`features.json`** (only supported way to add/change optional features). |

**Ports:** HTTP **8080**, git daemon **9418**, Piper HTTP **5590** (if enabled). Data **`/opt/relay/data`**.

**State:** **`/opt/relay/state/features.json`** records enabled features and ports. **`relay-server`** reads it (`RELAY_FEATURES_STATE_PATH` in the unit file) and exposes it under **`GET /api/config`** → **`installed_features`** (`manifest` + `summary`).

**Non-interactive install (CI/cloud-init):**

```bash
sudo RELAY_INSTALL_NONINTERACTIVE=1 RELAY_FEAT_PIPER=1 RELAY_FEAT_NPM_PKGS="songwalker-js" ./install.sh install
```

---

## One-liner from GitHub (any Linux)

**[`scripts/relay-curl.sh`](../scripts/relay-curl.sh)** installs `curl`, `tar`, `bash`, etc. if missing, pulls **`relay-install.sh`** + **`piper-tts-http.py`** from **`main`** (override with **`RELAY_REF`** only by editing the URL path), then runs the subcommand.

**Repair or update** (server already has `/opt/relay/bin/relay-server`; pulls latest **`relay-install.sh`** from GitHub, reapplies systemd/hooks; binaries stay in place unless you set **`RELAY_DEPLOY_TGZ_URL`** / **`RELAY_BIN_SOURCE`** for **update**):

```bash
curl -fsSL "https://raw.githubusercontent.com/clevertree/relay-server/main/scripts/relay-curl.sh" | sudo bash -s -- repair
```

Swap **`repair`** for **`update`** after you’ve published new binaries (see below).

**First install** (needs a hosted **`relay-linode-deploy.tgz`** — e.g. S3, release asset, or your CDN):

```bash
export RELAY_DEPLOY_TGZ_URL="https://YOUR_HOST/relay-linode-deploy.tgz"
curl -fsSL "https://raw.githubusercontent.com/clevertree/relay-server/main/scripts/relay-curl.sh" | sudo -E bash -s -- install
```

Non-interactive: `export RELAY_INSTALL_NONINTERACTIVE=1` (and Piper/npm vars) before the same **`curl | sudo -E bash`** line.

**Another branch** — change **`main`** in the URL to your branch name.

**Binaries on disk instead of URL:**

```bash
export RELAY_BIN_SOURCE=/root/deploy   # relay-server + relay-hook-handler here
curl -fsSL "https://raw.githubusercontent.com/clevertree/relay-server/main/scripts/relay-curl.sh" | sudo -E bash -s -- install
```

---

## Seed repos (push from laptop; no GitHub on server)

```bash
# On server
sudo -u relay git init --bare /opt/relay/data/snesology-presets.git
```

```bash
# Laptop
git remote add relay git://IP:9418/snesology-presets.git
git push -u relay main
```

Record **`git rev-parse main`** → put that SHA as **`anchor_commit`** for that repo in **`authorized-repos.yaml`**, then **`systemctl restart relay-server`**.

---

## Extra nodes (`RELAY_SERVER_ID` + repos)

**Interactive (TTY):** run **`relay-bootstrap.sh`** with no manifest — you get a checkbox-style list (**`[x]`** = clone). Defaults: **relay-template**, **relay-server**, **songwalker-library** (GitHub `clevertree/*`). Toggle with **`1` `2` `3`**, **`a`** = all, **`n`** = none, **Enter** = proceed.

```bash
export RELAY_SERVER_ID=relay-atlanta2
sudo -u relay bash /opt/relay/.../relay-bootstrap.sh   # or from deploy tarball directory
```

**Non-interactive / cloud-init:** clones **all** catalog repos unless excluded:

```bash
RELAY_BOOTSTRAP_NONINTERACTIVE=1 RELAY_CATALOG_EXCLUDE=relay-server ./relay-bootstrap.sh
```

**Manifest URL** (anchors + optional npm) still supported:

```bash
export RELAY_SERVER_ID=relay-atlanta2
export RELAY_BOOTSTRAP_MANIFEST_URL=https://…/bootstrap.json
./relay-bootstrap.sh
```

Manifest **`relay_server_id`** must match. Copy **`authorized-repos.yaml`** to the new node (same anchors) before starting **`relay-server`**. To add more default catalog repos, edit **`BOOTSTRAP_CATALOG_NAMES`** / **`BOOTSTRAP_CATALOG_URLS`** in **`scripts/relay-bootstrap.sh`**.

---

## Verify

```bash
curl -s http://IP:8080/api/config | jq .
# Optional features (if configured): jq .installed_features
```

---

## Docs

- **[`RELAY_TRUST_AND_BOOTSTRAP.md`](./RELAY_TRUST_AND_BOOTSTRAP.md)** — full trust model.
