# Deploy rendezvous server (`wisp-server`)

The rendezvous server is the HTTP middleman that lets two devices pair via
a 6-character code over the public internet. It does **not** see file
content — it only swaps iroh tickets between sender and receiver.

Code lives in `crates/server/`. Routes:

| Method + path | Purpose |
|---|---|
| `GET /healthz` | Plain-text liveness probe (`OK`) |
| `POST /v1/pairs` | Receiver registers, server returns 6-char code |
| `GET /v1/pairs/{code}/status` | Receiver polls to detect claim |
| `POST /v1/pairs/{code}/claim` | Sender swaps code → ticket |

Defaults: listens on `127.0.0.1:8787`. Dockerfile binds `0.0.0.0:8787`.

Rate-limited per IP (10 register/min, 60 read/min). State held in memory —
no DB needed. Pair sessions TTL 5 minutes. Restart = active pair codes lost
(receivers will re-register on the next health-check loop, so practically
fine).

## Chosen deploy: Oracle Cloud free tier (ARM64) + `rendezvous.wisp.mooo.com`

Step-by-step for the committed setup. Sections "Option A / Option B" below
stay as generic references; use them only if you swap host or registrar.

### 0. One-time account setup (~30 min)

- **Oracle Cloud free tier**: sign up at <https://www.oracle.com/cloud/free/>.
  Pick a home region with Ampere A1 capacity (Singapore / Tokyo / Frankfurt
  usually have stock — try a few if "out of capacity").
- **mooo.com (FreeDNS)**: sign up at <https://freedns.afraid.org>. The
  `mooo.com` zone is one of the free public domains. After signup → "Add a
  Subdomain" → Type `A`, Subdomain `rendezvous.wisp`, Domain `mooo.com`,
  Destination = (filled later, after VM exists).

### 1. Provision the ARM VM

In Oracle Cloud Console → **Compute** → **Instances** → **Create Instance**:

| Field | Value |
|---|---|
| Image | Canonical Ubuntu 22.04 (or 24.04) |
| Shape | `VM.Standard.A1.Flex` — 1 OCPU, 6 GB RAM (free-tier slice) |
| Network | new VCN, public subnet, **Assign public IPv4** |
| SSH | paste your public key |

After launch, note the **public IPv4**. Update FreeDNS:
mooo.com control panel → subdomain `rendezvous.wisp` → set Destination to
that IP → Save.

Verify (might take 1–5 min for DNS propagation):

```bash
dig +short rendezvous.wisp.mooo.com
# expect: <your-vm-ip>
```

### 2. Open ports — two places (Oracle gotcha)

Oracle Cloud blocks inbound by default, in **two** layers. You must open
both:

**a) VCN security list** (cloud-level firewall):

Console → **Networking** → **Virtual Cloud Networks** → your VCN →
**Security Lists** → **Default Security List** → **Add Ingress Rules**:

| Source CIDR | Protocol | Dest Port |
|---|---|---|
| `0.0.0.0/0` | TCP | 80 |
| `0.0.0.0/0` | TCP | 443 |

**b) Instance-level iptables** (Ubuntu image preloads strict rules):

```bash
ssh ubuntu@rendezvous.wisp.mooo.com
sudo iptables -I INPUT 6 -m state --state NEW -p tcp --dport 80 -j ACCEPT
sudo iptables -I INPUT 6 -m state --state NEW -p tcp --dport 443 -j ACCEPT
sudo netfilter-persistent save
```

(If `netfilter-persistent` not found: `sudo apt install iptables-persistent`.)

Sanity check from another machine:

```bash
nc -zv rendezvous.wisp.mooo.com 80
nc -zv rendezvous.wisp.mooo.com 443
# both should say "succeeded"
```

If either still hangs → recheck which layer is blocking (cloud security
list usually).

### 3. Build the Docker image on the VM (native ARM64)

Docker pulls the `arm64` variants of `rust:1.94-slim` and `debian:bookworm-slim`
automatically when the host is ARM64, so the repo's `Dockerfile` works
unchanged. Build on the VM (avoids cross-arch headaches from pushing an
x86 image to an ARM host):

```bash
ssh ubuntu@rendezvous.wisp.mooo.com

sudo apt update
sudo apt install -y git curl ca-certificates

git clone https://github.com/vigov5/wisp.git wisp-server
cd wisp-server
docker build -t wisp-rendezvous:latest .
# ~10 min on Ampere A1 (1 OCPU) — Cargo's the long pole, not Docker
```

### 4. Run via docker compose (rendezvous + Caddy)

`~/wisp-server/docker-compose.yml`:

```yaml
services:
  rendezvous:
    image: wisp-rendezvous:latest
    restart: unless-stopped
    expose:
      - "8787"
    networks: [web]

  caddy:
    image: caddy:2
    restart: unless-stopped
    ports:
      - "80:80"
      - "443:443"
    volumes:
      - ./Caddyfile:/etc/caddy/Caddyfile:ro
      - caddy_data:/data
      - caddy_config:/config
    networks: [web]

networks:
  web: {}

volumes:
  caddy_data: {}
  caddy_config: {}
```

`~/wisp-server/Caddyfile`:

```
rendezvous.wisp.mooo.com {
    reverse_proxy rendezvous:8787
}
```

Start:

```bash
cd ~/wisp-server
docker compose up -d
docker compose logs -f caddy
# wait for "certificate obtained successfully"
# Ctrl-C to stop following — services keep running
```

> **Rootless-Docker gotcha**: if `docker compose up` errors out with
> `cannot expose privileged port 443 ... rootlesskit binary`, Docker is
> running rootless and can't bind to ports < 1024 by default. Fix by
> lowering the unprivileged port floor:
>
> ```bash
> sudo sh -c 'echo "net.ipv4.ip_unprivileged_port_start=80" > /etc/sysctl.d/99-unprivileged-ports.conf'
> sudo sysctl --system
> docker compose down && docker compose up -d
> ```
>
> Persists across reboots. Alternative: `sudo setcap cap_net_bind_service=+ep "$(which rootlesskit)"` if you'd rather scope the capability to Docker only.

Sanity check from the VM:

```bash
docker compose ps              # both services Up
curl -s http://localhost/healthz   # OK (HTTP, no cert)
```

### 5. Verify end-to-end

From your laptop:

```bash
curl -s https://rendezvous.wisp.mooo.com/healthz
# expect: OK

curl -isS https://rendezvous.wisp.mooo.com/healthz | head -3
# expect HTTP/2 200 and a valid LetsEncrypt cert (no warnings)
```

### 6. Switch the app

Two paths — pick one:

- **Quick test first**: open Wisp on a real phone → Settings →
  Discovery Server → paste `https://rendezvous.wisp.mooo.com` → save.
  Try a code-based pair end-to-end with another device using the same
  override. If that works, flip the default in code (next bullet).
- **Ship-the-default**: edit `crates/core/src/rendezvous.rs`:
  ```rust
  pub const DEFAULT_RENDEZVOUS_URL: &str = "https://rendezvous.wisp.mooo.com";
  ```
  Rebuild + release. Existing users keep their per-install override (if
  any); new installs use the new default.

### Operational notes specific to this setup

- **mooo.com TTL**: FreeDNS subdomain TTL is short (5 min). Re-pointing
  on a VM swap propagates fast.
- **Oracle reclaim policy**: free-tier ARM VMs get reclaimed if idle >7
  days CPU. Health-check ping from UptimeRobot (next section) keeps the
  VM "active" enough to dodge that.
- **Ampere A1 quirks**: cargo builds occasionally OOM on the 1 OCPU /
  6 GB shape during link of heavy crates. If a build dies with `killed`,
  `cargo build -j 1 --release -p wisp-server` serialises and gets
  through.
- **No registrar billing**: mooo.com is free but ad-supported; the
  control panel is plain HTML, no API. If you outgrow it, swap to a real
  registrar (`wisp.<your-domain>`) — only the Caddyfile host line + the
  app's `DEFAULT_RENDEZVOUS_URL` change.

---

## Prerequisites

- A VPS / always-on host with public IPv4 + SSH access. Cheap tier is enough
  (1 vCPU, 512 MB RAM, <1 GB disk). Tested fine on a Hetzner CX11, Oracle
  Cloud free tier, or a Raspberry Pi with port-forward.
- A domain you control (e.g. `<your-domain>`). The app pins
  `DEFAULT_RENDEZVOUS_URL` to an `https://` URL, so HTTPS is required —
  not a plain IP.
- Ports `80` + `443` reachable from the public internet (for TLS issuance
  + serving). `8787` does **not** need to be public — Caddy proxies it.

## Option A — Docker + Caddy (recommended)

Caddy auto-provisions a Let's Encrypt cert; you only have to write a 4-line
config.

### 1. Build the server image

On your dev machine, from the repo root:

```bash
docker build -t wisp-rendezvous:latest .
```

The repo's `Dockerfile` produces a ~25 MB image based on `debian:bookworm-slim`.

Push to a registry your VPS can pull from (Docker Hub, GHCR, or
`docker save | ssh vps docker load` if no registry).

### 2. Point DNS

Create an `A` record:

```
wisp.<your-domain>   →   <vps-public-ip>
```

Wait for DNS to propagate: `dig wisp.<your-domain> +short` should return
your VPS IP.

### 3. SSH into the VPS + write the compose file

```bash
ssh <user>@<vps>
mkdir -p ~/wisp && cd ~/wisp
```

`docker-compose.yml`:

```yaml
services:
  rendezvous:
    image: wisp-rendezvous:latest
    # Or: ghcr.io/<you>/wisp-rendezvous:latest
    restart: unless-stopped
    expose:
      - "8787"
    networks: [web]

  caddy:
    image: caddy:2
    restart: unless-stopped
    ports:
      - "80:80"
      - "443:443"
    volumes:
      - ./Caddyfile:/etc/caddy/Caddyfile:ro
      - caddy_data:/data
      - caddy_config:/config
    networks: [web]

networks:
  web: {}

volumes:
  caddy_data: {}
  caddy_config: {}
```

`Caddyfile`:

```
wisp.<your-domain> {
    reverse_proxy rendezvous:8787
}
```

That's the whole config. Caddy handles cert issue + renewal automatically.

### 4. Start

```bash
docker compose up -d
docker compose logs -f
```

Wait for Caddy to log `certificate obtained successfully`.

### 5. Verify

From any machine with internet:

```bash
curl -s https://wisp.<your-domain>/healthz
# expect: OK

# Try a register call (won't actually pair anything but should return JSON):
curl -s -X POST https://wisp.<your-domain>/v1/pairs \
  -H 'Content-Type: application/json' \
  -d '{"ticket":"AAAA"}' | head
```

The register call rate-limits per IP — don't loop it.

## Option B — bare metal + systemd (no Docker)

If the VPS can't or shouldn't run Docker.

### 1. Build the binary

On the VPS (or cross-compile + scp):

```bash
git clone https://github.com/vigov5/wisp.git wisp-server
cd wisp-server
cargo build --release -p wisp-server
sudo install -o root -g root -m 0755 \
  target/release/wisp-server /usr/local/bin/wisp-server
```

### 2. systemd unit

`/etc/systemd/system/wisp-rendezvous.service`:

```ini
[Unit]
Description=Wisp rendezvous server
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=/usr/local/bin/wisp-server serve --listen 127.0.0.1:8787
Restart=on-failure
RestartSec=5s
User=nobody
Group=nogroup
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
LimitNOFILE=8192

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now wisp-rendezvous
sudo systemctl status wisp-rendezvous
```

### 3. Caddy in front

Install Caddy: <https://caddyserver.com/docs/install>

`/etc/caddy/Caddyfile`:

```
wisp.<your-domain> {
    reverse_proxy 127.0.0.1:8787
}
```

```bash
sudo systemctl restart caddy
```

Verify same as Option A.

## Wire the app to the new server

Two ways — pick one:

### A. Default in code (preferred for shipping Wisp)

```rust
// crates/core/src/rendezvous.rs
pub const DEFAULT_RENDEZVOUS_URL: &str = "https://wisp.<your-domain>";
```

Rebuild + reship. Any user who hasn't overridden the URL will use your
server.

### B. Per-install override

Open the app → **Settings** → **Discovery Server** → paste
`https://wisp.<your-domain>` → Save. Useful for testing before flipping
the default in code.

## Health monitoring

Set up an uptime check (e.g. UptimeRobot free tier, Better Stack, Healthchecks.io)
hitting `https://wisp.<your-domain>/healthz` every 5 minutes. If it goes
red, code-based pairing breaks for all users — QR / LAN still works as a
fallback so it's not fatal, just degraded.

## Capacity sizing

Per pair session: ~200 bytes RAM + 5 min TTL + 1 register + ≤300 status
polls + 1 claim. The rate limiter is the practical cap: 10 register/min
per source IP. A single tiny VPS handles thousands of concurrent users.

Bandwidth: tickets are ≤4 KB. A successful pair = ~10 KB round-trip total.
A noisy 10k-pairs-per-day install pushes <100 MB/day.

## Restart / redeploy

```bash
# Docker
docker compose pull && docker compose up -d

# Systemd
sudo systemctl restart wisp-rendezvous
```

Existing receivers that registered before the restart will see their next
status poll fail, then re-register on the next loop (a few seconds). No
manual user action required.

## Decisions (locked)

| | Choice |
|---|---|
| Host | Oracle Cloud free tier — Ampere A1 ARM64 |
| Domain | `rendezvous.wisp.mooo.com` (FreeDNS) |
| Runtime | Docker compose (rendezvous + Caddy) on the VM |
| Default URL switch | Parallel test via per-install override first; flip `DEFAULT_RENDEZVOUS_URL` after a successful end-to-end pair |
