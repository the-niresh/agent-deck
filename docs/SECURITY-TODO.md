# Security backlog (agent-deck fork)

Upstream (BloopAI/vibe-kanban) was sunset on 2026-04-24 with open security issues
unpatched. This fork inherits them. Tracked here because upstream will never fix them.

## Fixed in this fork

### Invitation could be redeemed by any account (upstream #3430) — FIXED
**Severity:** Critical · **Fixed:** 2026-07-25

`InvitationRepository::accept_invitation` looked up the invitation by token alone and
never compared the invitation's `email` against the accepting user's. Any party holding
the token — a forwarded email, a leaked link — could join the organization with the
invited role, including Admin.

Fix: `crates/remote/src/db/invitations.rs` now requires `user_email` and rejects a
mismatch (case-insensitive) *before* any other validation, so a mismatched caller cannot
trigger side effects such as the expiry state transition.

## Open

### Unvalidated OAuth `return_to` enables one-click account takeover (upstream #3429)
**Severity:** Critical (when OAuth is enabled) · **Status:** OPEN — deferred 2026-07-25

**Current exposure: low.** This deployment authenticates via
`SELF_HOST_LOCAL_AUTH_EMAIL` / `SELF_HOST_LOCAL_AUTH_PASSWORD`, so no OAuth provider is
configured and the vulnerable path is not reachable.

**This becomes exploitable the moment a real OAuth provider (GitHub/Google/Entra) is
enabled.** Fix it *before* turning one on, not after.

Expected shape of the fix: validate `return_to` against an allowlist of same-origin
relative paths; reject absolute URLs and protocol-relative (`//host`) values. Start at
`crates/remote/src/auth/` and `crates/remote/src/routes/oauth.rs`.

### Other upstream security issues not yet triaged
- Review remaining open issues on the archived upstream repo before widening access.
- The stack currently binds to `127.0.0.1` only (`13000`, `15433`). Re-audit all of the
  above before exposing any port beyond loopback.
