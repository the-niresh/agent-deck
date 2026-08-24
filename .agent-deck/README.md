# Roles

A role is a named bundle of instructions and executor defaults, committed here so it
is reviewed and versioned with the code it governs.

Select one when starting a workspace (`role_id` on `start_workspace`, or
`ExecutorConfig.role_id`). The file is read from the worktree at spawn time.

## Format

`<name>.md`, where the filename stem is the role's name. TOML frontmatter is
optional; a file that is only instructions is a valid role.

```markdown
+++
description = "Owns deploys, infra and CI"
executors = ["CLAUDE_CODE", "CODEX"]   # omit to permit every executor
model = "opus"                         # a default, not an override
claude_agent = "cloud-engineer"        # Claude Code only, ignored elsewhere
reasoning = "high"
permission_policy = "SUPERVISED"       # AUTO | SUPERVISED | PLAN
+++

Prefer boring infrastructure.
```

## What applies where

The instructions are prepended to the opening prompt, which is why a role works on
every executor rather than only on Claude Code. The frontmatter fields fill in
config the caller left unset - an explicit choice always wins over the role's
default.

`executors` is the one field that enforces rather than defaults: starting a run on
an executor the role does not permit fails instead of quietly proceeding.
