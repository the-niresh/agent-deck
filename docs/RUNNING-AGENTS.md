# Running coding agents (permissions & the root problem)

## The error

```
--dangerously-skip-permissions cannot be used with root/sudo privileges for security reasons
```

The workspace is created correctly (branch, git panel, terminal all fine) — only the agent
process fails to launch.

## Why it happens

Three things combine:

1. **The server is running as root.** (`id -u` → `0`)
2. **The Claude executor passes `--dangerously-skip-permissions`** by default —
   `crates/executors/src/executors/claude.rs:177`, gated on the profile's
   `dangerously_skip_permissions` flag.
3. **The `claude` CLI itself hard-refuses that flag when running as root/sudo.** The guard is
   inside Claude Code, not in this project. Nothing here can "fix" it except changing 1 or 2.

## Option A — run as a non-root user (recommended)

Removes the dangerous combination instead of suppressing the warning about it.

An agent running as root with permissions skipped can reach everything on the host: other
repos, `~/.ssh`, the Docker socket, unrelated containers. That is precisely what the guard
is designed to prevent.

1. Create a dedicated user (e.g. `agent`).
2. Give it ownership of the repos you want agents to work in.
3. Authenticate the coding agent CLI **as that user** (`claude` login, etc.).
4. Run the agent-deck server as that user.

The Docker stack (Postgres/Electric/remote-server) is unaffected and needs no change.

## Option B — disable skip-permissions

Turn off `dangerously_skip_permissions` in the executor profile so `claude` uses its normal
permission model. No new user required.

**Caveat:** the executor runs `claude -p` (headless). There is no human present to answer
approval prompts, so agents will stall or fail on write operations until an explicit
`allowedTools` allowlist is configured per project. Workable, but needs tuning.

## What NOT to do

`IS_SANDBOX=1` makes Claude Code accept skip-permissions as root. It exists to signal a
**disposable throwaway container**.

Do not set it on a host that holds real repositories, SSH keys, or other people's services.
It silences the warning while leaving the entire risk intact — the opposite of a fix. It is
only defensible on a genuinely disposable VM whose loss costs nothing.

## Related

Agent execution is inherently **local** — it runs processes and git worktrees on the machine
hosting the server. See [DEPLOYMENT.md](DEPLOYMENT.md) for why the public hostname fronts the
authenticated remote server rather than the local workspaces app.
