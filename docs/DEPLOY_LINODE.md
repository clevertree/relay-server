# Deploy relay-server to Linode (bare metal — no Docker)

## Recommended plan

| Step | What |
|------|------|
| **1. VM** | Linode **Ubuntu 22.04/24.04 LTS**, open **22, 8080, 9418** (and **5590** if Piper). |
| **2. Binaries** | CI **[`publish-linode-tarball`](../.github/workflows/publish-linode-tarball.yml)**: every **`main`** push uploads an artifact **`relay-linode-deploy-<sha>.tgz`** (Actions → workflow run → Artifacts). **Tag** `v1.2.3` → same workflow attaches **`relay-linode-deploy.tgz`** to the **Release** (stable URL for **`RELAY_DEPLOY_TGZ_URL`**). Or **`./scripts/pack-linode-deploy.sh`** locally. |
| **3. Install** | **`relay-curl.sh`** one-liner with **`RELAY_DEPLOY_TGZ_URL`**, **or** `scp` the tgz and **`sudo ./install.sh install`**. The installer’s **first step** is **Vercel DNS** (unless skipped): detect public IPv4, upsert **A** record for your **FQDN**, wait until DNS resolves, then install packages and binaries. Uses **`VERCEL_API_TOKEN`** (optional **`VERCEL_TEAM_ID`**) like Docker/K8s. See **[Vercel DNS (first step of `install`)](#vercel-dns-first-step-of-install)**. |
| **4. Trust** | **`authorized-repos.yaml`** + **`relay.env`** (`RELAY_SERVER_ID`, `RELAY_AUTHORIZED_REPOS_PATH`). |
| **5. Data** | **`relay-bootstrap.sh`** (catalog or manifest) **or** bare init + **`git push`** from laptop. |
| **6. Updates** | **Binary upgrade:** new tgz → **`install.sh update`** or **`relay-curl.sh`** with new **`RELAY_DEPLOY_TGZ_URL`**. **Script/hardening only:** **`relay-curl.sh … repair`** (pulls **`relay-install.sh`** from GitHub). |

**Release tarball URL shape (after you push tag `v0.3.2`):**

`https://github.com/clevertree/relay-server/releases/download/v0.3.2/relay-linode-deploy.tgz`

**Latest `main` tarball:** open the latest successful **publish-linode-tarball** run on **`main`** → **Artifacts** → download **`relay-linode-deploy-<sha>.tgz`**. Or **Run workflow** manually.

**CLI download** ([GitHub CLI](https://cli.github.com/): `gh auth login`):

```bash
RUN=$(gh run list -R clevertree/relay-server -w publish-linode-tarball -b main -L 1 --json databaseId -q '.[0].databaseId')
gh run download -R clevertree/relay-server "$RUN"
# Artifact folder is named relay-linode-deploy-<sha>/relay-linode-deploy.tgz
```

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

If **`install.sh`** completed Vercel DNS, it may have appended **`RELAY_PUBLIC_HOSTNAME=your.fqdn`** (non-secret). That value is the **node** hostname; each bare repo is served at **`{repo-name}.{RELAY_PUBLIC_HOSTNAME}`** (HTTP **`Host`** header). Add a **wildcard** DNS record **`*.your.fqdn`** → same server IP so every repo subdomain resolves. Trust settings are still required regardless.

---

## Vercel DNS (first step of `install`)

**[`scripts/relay-install.sh`](../scripts/relay-install.sh)** (shipped as **`install.sh`** in the tarball) asks for a **full hostname** (e.g. **`atlanta1.relaygateway.net`**), uses the **Vercel API** to create or update an **A** record to this machine’s **detected public IPv4**, then **polls public DNS** (e.g. `dig @8.8.8.8`) until that name resolves to the same address. If resolution never matches, **install aborts** before package/binary setup.

**Authentication** matches the Docker entrypoint and legacy Rackspace/Kubernetes path: **`VERCEL_API_TOKEN`** (Bearer). Optional **`VERCEL_TEAM_ID`** for team-scoped API calls (same as **`relay-credentials`** / daemonset env in **`terraform/rackspace-spot`**).

| Variable | Purpose |
|----------|---------|
| **`RELAY_PUBLIC_FQDN`** or **`RELAY_DNS_FQDN`** | Full DNS name to publish and verify (e.g. `atlanta1.relaygateway.net`). |
| **`VERCEL_API_TOKEN`** | Required for DNS upsert (unless skipping). **`VERCEL_TOKEN`** is accepted as an alias. |
| **`VERCEL_TEAM_ID`** | Optional; append **`?teamId=`** on Vercel API requests. |
| **`RELAY_VERCEL_DOMAIN`** | Optional; **Vercel zone** (e.g. `relaygateway.net`) if you want to skip auto-detection. The FQDN must end with **`.${RELAY_VERCEL_DOMAIN}`** (or equal the zone for an apex record). |
| **`RELAY_SKIP_VERCEL_DNS=1`** | Skip DNS entirely (CI, air-gapped, or DNS managed elsewhere). |
| **`RELAY_DNS_TTL`** | Record TTL in seconds (default **60**). |
| **`RELAY_DNS_WAIT_ATTEMPTS`** | Max poll attempts (default **36**). |
| **`RELAY_DNS_WAIT_SLEEP`** | Seconds between polls (default **5**). |

**Zone auto-detection:** the script calls **`GET /v5/domains`** and picks the **longest** domain name on the account that matches the suffix of your FQDN (so `atlanta1.relaygateway.net` maps to zone **`relaygateway.net`**, record **`atlanta1`**). The domain must already exist under your Vercel team/account.

**Interactive (SSH TTY):** you are prompted to confirm DNS setup, then for **hostname** and **API token** if the token is not already in the environment.

**Non-interactive / cloud-init:** set **`RELAY_INSTALL_NONINTERACTIVE=1`** and either:

- **With DNS:** export **`RELAY_PUBLIC_FQDN`** and **`VERCEL_API_TOKEN`** (and **`VERCEL_TEAM_ID`** if needed), **or**
- **Without DNS:** set **`RELAY_SKIP_VERCEL_DNS=1`**.

**Non-TTY without env:** if neither FQDN+token nor **`RELAY_SKIP_VERCEL_DNS=1`** is set, the installer **skips** the DNS phase and continues (same pattern as other optional prompts).

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
| `sudo ./install.sh install` | First-time install: **Vercel DNS** first (unless skipped), then deps + binaries; interactive feature prompts for Piper TTS and npm (unless non-interactive). Fails if already installed unless `RELAY_INSTALL_FRESH=1`. |
| `sudo ./install.sh update` | Refresh binaries from tarball dir; restart services. |
| `sudo ./install.sh repair` | Re-apply systemd, hooks, npm extensions, Piper service from **`state/features.json`**. If **`features.json`** is missing but **`/opt/relay/bin/relay-server`** exists, a minimal state file is created (Piper/npm off) so repair/update work on manually seeded hosts. |
| `sudo ./install.sh reconfigure-features` | Re-run feature prompts and rewrite **`features.json`** (only supported way to add/change optional features). |
| `sudo ./install.sh refresh-features` | Rescan Piper models + Argos packs into **`features.json`** (after adding voices or translation packages). |

**Ports:** HTTP **8080**, git daemon **9418**, Piper HTTP **5590** (if enabled). Data **`/opt/relay/data`**.

**State:** **`/opt/relay/state/features.json`** records enabled features, ports, and **inventories** (Piper voices, Argos language pairs, npm package list). **`relay-server`** reads it (`RELAY_FEATURES_STATE_PATH` in the unit file) and exposes it under **`GET /api/config`** → **`installed_features`** (`manifest` + `summary`). Peers and clients use that to see what each node offers.

**Feature catalog (Piper TTS, offline translation, npm extensions, subfeatures):** **[INSTALL_FEATURES.md](./INSTALL_FEATURES.md)**.

**Non-interactive install (CI/cloud-init):**

With **Vercel DNS** (recommended when the hostname should point at this VM):

```bash
sudo RELAY_INSTALL_NONINTERACTIVE=1 \
  RELAY_PUBLIC_FQDN="atlanta1.relaygateway.net" \
  VERCEL_API_TOKEN="your-token" \
  RELAY_FEAT_PIPER=0 \
  RELAY_FEAT_TRANSLATION=0 \
  ./install.sh install
```

Optional: **`VERCEL_TEAM_ID=...`**, **`RELAY_VERCEL_DOMAIN=relaygateway.net`** (if you do not want API-based zone detection).

Skipping DNS (no token on host, or DNS managed manually):

```bash
sudo RELAY_INSTALL_NONINTERACTIVE=1 RELAY_SKIP_VERCEL_DNS=1 RELAY_FEAT_PIPER=1 RELAY_FEAT_NPM_PKGS="songwalker-js" RELAY_FEAT_TRANSLATION=1 RELAY_FEAT_TRANSLATION_PKGS="translate-en_es" ./install.sh install
```

---

## One-liner from GitHub (any Linux)

**[`scripts/relay-curl.sh`](../scripts/relay-curl.sh)** installs `curl`, `tar`, `bash`, etc. if missing, pulls **`relay-install.sh`** + **`piper-tts-http.py`** from **`main`** (override with **`RELAY_REF`** only by editing the URL path), then runs the subcommand.

**Repair or update** (server already has `/opt/relay/bin/relay-server`; pulls latest **`relay-install.sh`** from GitHub, reapplies systemd/hooks; binaries stay in place unless you set **`RELAY_DEPLOY_TGZ_URL`** / **`RELAY_BIN_SOURCE`** for **update**):

```bash
curl -fsSL "https://raw.githubusercontent.com/clevertree/relay-server/main/scripts/relay-curl.sh" | sudo bash -s -- repair
```

Swap **`repair`** for **`update`** after you’ve published new binaries (see below).

**First install** (needs a hosted **`relay-linode-deploy.tgz`** — release asset, Actions artifact on HTTPS, S3, etc.):

```bash
export RELAY_DEPLOY_TGZ_URL="https://YOUR_HOST/relay-linode-deploy.tgz"
curl -fsSL "https://raw.githubusercontent.com/clevertree/relay-server/main/scripts/relay-curl.sh" | sudo -E bash -s -- install
```

**Update from a new tarball** (ensures the URL reaches the script when piping):

```bash
curl -fsSL "https://raw.githubusercontent.com/clevertree/relay-server/main/scripts/relay-curl.sh" | \
  sudo env RELAY_DEPLOY_TGZ_URL="https://YOUR_HOST/relay-linode-deploy.tgz" bash -s -- update
```

Non-interactive: `export RELAY_INSTALL_NONINTERACTIVE=1` before the same **`curl | sudo -E bash`** line. For **`install`**, also set **`RELAY_PUBLIC_FQDN`** + **`VERCEL_API_TOKEN`**, or **`RELAY_SKIP_VERCEL_DNS=1`**, plus any Piper/npm vars (see **[Vercel DNS (first step of `install`)](#vercel-dns-first-step-of-install)**).

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
