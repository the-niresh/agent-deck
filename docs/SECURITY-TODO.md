# Security backlog (agent-deck fork)

Upstream (BloopAI/agent-deck) was sunset on 2026-04-24 with open security issues
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

### ~~Unvalidated OAuth `return_to`~~ (upstream #3429) — FIXED 2026-07-26
**Severity:** Critical (when OAuth is enabled) · **Status:** FIXED 2026-07-26

`is_allowed_return_to` (`crates/remote/src/auth/handoff.rs`) *looked* like an allowlist —
it checked loopback and same-origin https — but then returned `true` unconditionally for
everything else, with the comment "Rely on PKCE for security".

**PKCE does not cover this.** PKCE binds an auth code to whoever *initiated* the flow. The
attack is that the attacker initiates it (their own `app_challenge`, `return_to` pointing
at their server) and induces a victim to complete it. The victim's `app_code` is delivered
to the attacker, who already holds the matching verifier — a one-click account takeover.

Fix: the function now fails closed, allowing only http loopback (the desktop app's
ephemeral port) and same-host https matching `SERVER_PUBLIC_BASE_URL`. Covered by
`rejects_external_return_to`, which asserts rejection of external hosts, lookalike hosts
(`agent.niresh.tech.evil.com`), subdomains, http downgrade of the real host, and
non-http schemes.

### Other upstream security issues not yet triaged
- Review remaining open issues on the archived upstream repo before widening access.
- The stack currently binds to `127.0.0.1` only (`13000`, `15433`). Re-audit all of the
  above before exposing any port beyond loopback.
