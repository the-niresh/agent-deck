<p align="center">
  <a href="https://github.com/the-niresh/agent-deck">
    <picture>
      <source srcset="packages/public/vibe-kanban-logo-dark.svg" media="(prefers-color-scheme: dark)">
      <source srcset="packages/public/vibe-kanban-logo.svg" media="(prefers-color-scheme: light)">
      <img src="packages/public/vibe-kanban-logo.svg" alt="Vibe Kanban Logo">
    </picture>
  </a>
</p>

<p align="center">Self-hosted kanban for coding agents. Plan work on a board, let agents run it in isolated workspaces, review the diff before anything is pushed.</p>

> **agent-deck** — a self-hosted fork of [BloopAI/vibe-kanban](https://github.com/BloopAI/vibe-kanban),
> maintained independently. Upstream was sunset with its last commit on 2026-04-24 and its
> hosted cloud is gone, so links to `vibekanban.com`, upstream Discussions, and Discord have
> been removed rather than left to rot.
>
> **What this fork changes:**
> - **Kanban board restored.** Upstream `#3387` replaced the project board with an
>   export-only sunset page. That commit is reverted here, unconditionally — not behind a
>   build flag, so an unset env var can never silently gut the UI again.
> - **Cloud shutdown banners removed** from both the local and remote frontends.
> - **17 community PRs merged** that upstream never landed — MCP orchestration tooling,
>   memory and workflow fixes, additional agent models. See `git log`.
> - **Runs entirely on your own infrastructure.** No upstream cloud, no accounts.
>   Analytics are compile-time gated on `POSTHOG_API_KEY`, which is absent when building
>   from source, so a self-built binary reports nothing.
>
> See [Self-hosting](#self-hosting) below to bring up the full stack.

![](packages/public/vibe-kanban-screenshot-overview.png)

## Overview

In a world where software engineers spend most of their time planning and reviewing coding agents, the most impactful way to ship more is to get faster at planning and review.

agent-deck is built for this. Use kanban issues to plan work, either privately or with your team. When you're ready to begin, create workspaces where coding agents can execute.

- **Plan with kanban issues** — create, prioritise, and assign issues on a kanban board
- **Run coding agents in workspaces** — each workspace gives an agent a branch, a terminal, and a dev server
- **Review diffs and leave inline comments** — send feedback directly to the agent without leaving the UI
- **Preview your app** — built-in browser with devtools, inspect mode, and device emulation
- **Switch between 10+ coding agents** — Claude Code, Codex, Gemini CLI, GitHub Copilot, Amp, Cursor, OpenCode, Droid, CCR, and Qwen Code
- **Create pull requests and merge** — open PRs with AI-generated descriptions, review on GitHub, and merge

![](packages/public/vibe-kanban-screenshot-workspace.png)

One command. Describe the work, review the diff, ship it.

```bash
npx vibe-kanban
```


## Installation

> `npx vibe-kanban` installs the **published upstream package**, which is pinned at the
> sunset release and shows the export-only page. Build from this checkout instead.

Authenticate your coding agent CLI first (`claude`, `codex`, `gemini`, …), then:

```bash
pnpm i
pnpm run dev          # frontend + backend, ports auto-assigned
```

### Build prerequisites

- **Rust** — the toolchain is pinned by `rust-toolchain.toml` (`nightly-2025-12-04`);
  `rustup` installs it automatically on first build.
- **`LIBCLANG_PATH`** — `libsqlite3-sys` runs `bindgen`, which needs clang's builtin
  headers. If the build fails with `'stdarg.h' file not found`, point it at the LLVM
  install whose resource dir actually exists:
  ```bash
  export LIBCLANG_PATH=/usr/lib/llvm-18/lib
  ```
- **Desktop app is optional.** `crates/tauri-app` needs GTK system libraries. To build
  and test only the web stack, exclude it:
  ```bash
  cargo test --workspace --exclude vibe-kanban-tauri
  ```

## Architecture: two halves, different requirements

Worth understanding before self-hosting, because they have very different needs:

| | Backing store | Needs sign-in? |
|---|---|---|
| **Workspaces** — agent execution, branches, terminals, dev servers, diff review | local SQLite | No |
| **Projects / kanban issues** — the board | Postgres + ElectricSQL via `crates/remote` | Yes |

Workspaces run standalone with no extra infrastructure. **The kanban board reads through
ElectricSQL shapes**, so it needs the remote stack below — without it the board renders
empty.

## Self-hosting

`crates/remote/starter/` brings up the full stack (Postgres with logical replication,
ElectricSQL, and the remote server) and points this checkout's client at it:

```bash
cd crates/remote/starter
./setup.sh              # generates .env.remote (gitignored: JWT + admin secrets)
make start              # docker up, then launches THIS checkout's frontend
```

Auth uses `SELF_HOST_LOCAL_AUTH_EMAIL` / `SELF_HOST_LOCAL_AUTH_PASSWORD`, so **no OAuth
provider is required**. Credentials are printed by `setup.sh` and stored in
`.env.remote`. Other targets: `make logs`, `make status`, `make backup`, `make stop`.

## A note on Claude Code billing

The Claude executor invokes `claude -p` (headless mode) — see
`crates/executors/src/executors/claude.rs`. Since 2026-06-15, `claude -p` draws from a
**separate credit pool** rather than a Pro/Max subscription, so driving many agents
through this tool bills differently than using Claude Code interactively. Budget
accordingly, or use one of the other supported executors.

## Support

Open an issue on [this repo](https://github.com/the-niresh/agent-deck/issues).

Upstream's Discussions, Discord, and docs site are no longer maintained — the project was
sunset and its cloud shut down. Don't file agent-deck issues there.

## Development

### Prerequisites

- [Rust](https://rustup.rs/) (latest stable)
- [Node.js](https://nodejs.org/) (>=20)
- [pnpm](https://pnpm.io/) (>=8)

Additional development tools:
```bash
cargo install cargo-watch
cargo install sqlx-cli
```

Install dependencies:
```bash
pnpm i
```

### Running the dev server

```bash
pnpm run dev
```

This will start the backend and web app. A blank DB will be copied from the `dev_assets_seed` folder.

### Building the web app

To build just the web app:

```bash
cd packages/local-web
pnpm run build
```

### Build from source (macOS)

1. Run `./local-build.sh`
2. Test with `cd npx-cli && node bin/cli.js`

### Environment Variables

The following environment variables can be configured at build time or runtime:

| Variable | Type | Default | Description |
|----------|------|---------|-------------|
| `POSTHOG_API_KEY` | Build-time | Empty | PostHog analytics API key (disables analytics if empty) |
| `POSTHOG_API_ENDPOINT` | Build-time | Empty | PostHog analytics endpoint (disables analytics if empty) |
| `PORT` | Runtime | Auto-assign | **Production**: Server port. **Dev**: Frontend port (backend uses PORT+1) |
| `BACKEND_PORT` | Runtime | `0` (auto-assign) | Backend server port (dev mode only, overrides PORT+1) |
| `FRONTEND_PORT` | Runtime | `3000` | Frontend dev server port (dev mode only, overrides PORT) |
| `HOST` | Runtime | `127.0.0.1` | Backend server host |
| `MCP_HOST` | Runtime | Value of `HOST` | MCP server connection host (use `127.0.0.1` when `HOST=0.0.0.0` on Windows) |
| `MCP_PORT` | Runtime | Value of `BACKEND_PORT` | MCP server connection port |
| `DISABLE_WORKTREE_CLEANUP` | Runtime | Not set | Disable all git worktree cleanup including orphan and expired workspace cleanup (for debugging) |
| `VK_ALLOWED_ORIGINS` | Runtime | Not set | Comma-separated list of origins that are allowed to make backend API requests (e.g., `https://my-vibekanban-frontend.com`) |
| `VK_SHARED_API_BASE` | Runtime | Not set | Base URL for the remote/cloud API used by the local desktop app |
| `VK_SHARED_RELAY_API_BASE` | Runtime | Not set | Base URL for the relay API used by tunnel-mode connections |
| `VK_TUNNEL` | Runtime | Not set | Enable relay tunnel mode when set (requires relay API base URL) |

**Build-time variables** must be set when running `pnpm run build`. **Runtime variables** are read when the application starts.

#### Self-Hosting with a Reverse Proxy or Custom Domain

When running Vibe Kanban behind a reverse proxy (e.g., nginx, Caddy, Traefik) or on a custom domain, you must set the `VK_ALLOWED_ORIGINS` environment variable. Without this, the browser's Origin header won't match the backend's expected host, and API requests will be rejected with a 403 Forbidden error.

Set it to the full origin URL(s) where your frontend is accessible:

```bash
# Single origin
VK_ALLOWED_ORIGINS=https://vk.example.com

# Multiple origins (comma-separated)
VK_ALLOWED_ORIGINS=https://vk.example.com,https://vk-staging.example.com
```

### Remote Deployment

When running Vibe Kanban on a remote server (e.g., via systemctl, Docker, or cloud hosting), you can configure your editor to open projects via SSH:

1. **Access via tunnel**: Use Cloudflare Tunnel, ngrok, or similar to expose the web UI
2. **Configure remote SSH** in Settings → Editor Integration:
   - Set **Remote SSH Host** to your server hostname or IP
   - Set **Remote SSH User** to your SSH username (optional)
3. **Prerequisites**:
   - SSH access from your local machine to the remote server
   - SSH keys configured (passwordless authentication)
   - VSCode Remote-SSH extension

When configured, the "Open in VSCode" buttons will generate URLs like `vscode://vscode-remote/ssh-remote+user@host/path` that open your local editor and connect to the remote server.

Configure this under Settings → General in the app. (Upstream's hosted docs covered this,
but that site is no longer maintained.)

## Licence and attribution

agent-deck is licensed under the [Apache License 2.0](LICENSE), inherited from upstream.

This is a modified fork of [BloopAI/vibe-kanban](https://github.com/BloopAI/vibe-kanban).
Upstream's last commit was 2026-04-24; everything after that point in this repository is
independent work. Changes made in this fork are summarised at the top of this README and
recorded in full in `git log`.

Copyright for the original work remains with the upstream authors. The Apache 2.0 licence
grants no rights to upstream's names, logos, or other brand assets — those are not covered
by the code licence and are not claimed here.
