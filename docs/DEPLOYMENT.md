# Public deployment (Traefik + Let's Encrypt)

## What gets exposed — and what must not

This project is two halves with very different security properties:

| Half | Port | Auth | Safe to expose? |
|---|---|---|---|
| **Remote server** — projects, kanban issues, orgs, invitations | 8081 | JWT session + login | **Yes** — designed to be internet-facing |
| **Local workspaces app** — agent execution, terminals, dev servers | 3001 / 13333 | **none** | **No — never** |

The local app requires no login (`RootRedirectPage` falls through to
`/workspaces/create` when signed out) and it launches coding agents as processes on the
host. Publishing it means unauthenticated remote code execution on this machine.

**Therefore the public hostname fronts only the remote server.** To reach your local
workspaces from elsewhere, use the built-in relay/tunnel pairing (Settings → Relay), which
authenticates the device — do not put the local app behind a plain reverse proxy.

## Setup

### 1. DNS

Traefik here uses the **TLS-ALPN-01** ACME challenge (`--certificatesresolvers.letsencrypt.acme.tlschallenge=true`),
which requires the hostname to resolve to this server *before* the container starts.
Bringing it up early means a failed issuance, and repeated failures hit Let's Encrypt
rate limits.

Add an **A record** matching the existing subdomains:

```
Type: A    Name: agent    Value: 76.13.243.12    TTL: default
```

Verify before continuing:

```bash
dig +short A agent.niresh.tech     # must return 76.13.243.12
```

### 2. Configure `.env.remote`

```bash
PUBLIC_BASE_URL=https://agent.niresh.tech
PUBLIC_HOSTNAME=agent.niresh.tech
TRAEFIK_NETWORK=n8n-mkvx_proxy
TRAEFIK_ENTRYPOINT=websecure
TRAEFIK_CERTRESOLVER=letsencrypt
```

`PUBLIC_BASE_URL` becomes `SERVER_PUBLIC_BASE_URL` in the container. It is **not
cosmetic** — it builds the invitation links sent by email
(`crates/remote/src/routes/organization_members.rs:137`) and the review URLs. Leave it as
`localhost` and every invitation you send will be unusable.

### 3. Apply

```bash
cd crates/remote/starter
make rebuild
```

Setting `PUBLIC_HOSTNAME` automatically layers in `docker-compose.traefik.yml` (see the
`TRAEFIK_OVERLAY` line in the Makefile). Without it, the stack stays on loopback.

The container keeps its `127.0.0.1:13000` publish for local access, and additionally joins
the Traefik network for public routing.

### 4. Verify

```bash
curl -sI https://agent.niresh.tech | head -1        # expect 200
docker logs traefik 2>&1 | grep -i acme | tail      # cert issuance
```

## Hardening before you invite anyone

- **Rotate the bootstrap admin password.** `setup.sh` generated it and wrote it to
  `.env.remote`; it was also printed to a terminal. Change it once real accounts exist.
- **Read [SECURITY-TODO.md](SECURITY-TODO.md).** Upstream issue #3429 (unvalidated OAuth
  `return_to`) is still open. It is not reachable while using
  `SELF_HOST_LOCAL_AUTH_*`, but **fix it before enabling any OAuth provider** — on a
  public hostname that becomes a one-click account takeover.
- **Postgres stays on loopback.** `127.0.0.1:15433` — keep it that way; nothing external
  needs it.
- **Optional extra layer.** Traefik basic-auth in front of the app, as defence in depth
  while this fork's security backlog is open:
  ```bash
  htpasswd -nbB youruser 'yourpassword'   # then add as a basicauth middleware label
  ```

## Note on agent execution

Exposing the kanban publicly does not make agent execution remote. Agents always run on
the machine hosting the *local* server, in local git worktrees. See
[RUNNING-AGENTS.md](RUNNING-AGENTS.md) — including why that host should not be root.
