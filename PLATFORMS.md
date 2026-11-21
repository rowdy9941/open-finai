# Platform support

Open FinAI runs from **Docker**. That is the only hard requirement, and it is what makes
the same build work on Linux, macOS and Windows without per-OS toolchains.

Everything compiles **inside the containers** — `rust:1-slim` for the core, `rust:1-slim`
plus `node:26-slim` for the frontend, with the WASM script engine built in a dedicated
stage. You do **not** need Rust, Node, `wasm32-unknown-unknown` or `wasm-pack` on the host.

## Support matrix

| | Linux | macOS (Intel / Apple Silicon) | Windows |
|---|---|---|---|
| **Docker Compose stack** | ✅ native | ✅ Docker Desktop | ✅ Docker Desktop (WSL2 backend) |
| Source build (`docker compose build`) | ✅ | ✅ | ✅ |
| Prebuilt images | ✅ | ✅ | ✅ |
| Web UI | ✅ | ✅ | ✅ |
| `install.sh` / `deploy/setup.sh` | ✅ | ✅ | ⚠️ needs WSL2 or Git Bash (bash 4+, `set -euo pipefail`) |
| `deploy/server-setup.sh` | ✅ only | ❌ | ❌ | 
| Whisper voice profile | ✅ | ⚠️ CPU only | ⚠️ CPU only |

### What each column means

**Linux** — the reference platform. Both the Docker stack and the bare-metal
`server-setup.sh` provisioning script (systemd unit, service account, firewall) are
supported. This is the target for a 24/7 deployment.

**macOS** — fully supported through Docker Desktop. The stack is identical; only the
container runtime differs. On Apple Silicon the images build for `linux/arm64`.

**Windows** — supported through Docker Desktop with the **WSL2 backend**. The bash
installers (`install.sh`, `deploy/setup.sh`) assume a POSIX shell, so run them from a WSL2
shell or Git Bash. `deploy/server-setup.sh` does not apply: it provisions a bare Linux host
(systemd, service user, firewall) and has no Windows equivalent.

## Why the installers are bash

`install.sh` and `deploy/setup.sh` perform **host-side** work before any container starts:

- generate `OTW_SECRET_KEY` and the Postgres password
- write `deploy/.env`, `network.env`, `dns.env` (mode `600`)
- detect the host's LAN IPv4 to decide the network mode
- choose ports and the bind address

They are deliberately not containerised, because the values they produce must land on the
host. On Windows, run them inside WSL2 — the files they write are visible to Docker Desktop.

## Minimum versions

| Component | Minimum | Notes |
|---|---|---|
| Docker Engine / Desktop | 24.0 | `docker compose` v2 required |
| Docker Compose | v2.20 | `additional_contexts` used by the frontend build |
| Disk | ~8 GB | images + build cache |
| RAM | 8 GB recommended | core defaults to 4 GB, Postgres to 2 GB (`OTW_CORE_MEM`, `OTW_PG_MEM`) |
| CPU | 2 cores | 4+ for comfortable backtests |

## Verifying your host

```bash
docker --version
docker compose version     # must be v2.x
docker buildx version      # required for the frontend's named build context
```

If `docker buildx` is missing, the frontend image cannot be built from source — it needs
the Rust workspace mounted as a named build context (`--build-context core=core`).

## Ports

| Port | Purpose | Default binding |
|---|---|---|
| 5454 | HTTP (app + API) | `127.0.0.1` |
| 8443 | HTTPS | `127.0.0.1` |

Change them in `deploy/network.env` or Settings → Network.

## Known platform limits

- **MetaTrader 5**: the official `MetaTrader5` Python package is **Windows-only**. A Linux
  or macOS deployment needs Wine, a Windows VM, or a broker REST bridge. This is a
  constraint of MT5 itself, not of Open FinAI.
- **Whisper** (optional `voice` profile) is CPU-only on macOS and Windows unless the
  container runtime exposes a GPU.
- **IBKR** requires TWS or IB Gateway reachable from the core container — normally run on
  the host, not inside Docker.
