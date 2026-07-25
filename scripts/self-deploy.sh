#!/usr/bin/env bash
# self-deploy.sh — pull-based continuous deployment.
#
# Polls the deploy branch and, when it has moved, rebuilds the stack.
# Trust flows one way: this machine reaches out to GitHub. GitHub is never
# given SSH keys, inbound access, or an open port here.
#
# Usage:
#   scripts/self-deploy.sh            # deploy if the remote has moved
#   scripts/self-deploy.sh --force    # deploy even if already up to date
#   scripts/self-deploy.sh --dry-run  # report what would happen, change nothing
#
# Exit codes: 0 = up to date or deployed OK · 1 = deploy failed (rolled back)
#             2 = refused to run (dirty tree, lock held, bad config)

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REMOTE="${DEPLOY_REMOTE:-agent-deck}"
BRANCH="${DEPLOY_BRANCH:-main}"
STARTER_DIR="$REPO_ROOT/crates/remote/starter"
LOCK_FILE="/var/lock/agent-deck-deploy.lock"
HEALTH_URL="${DEPLOY_HEALTH_URL:-http://127.0.0.1:13000/api/health}"
HEALTH_RETRIES="${DEPLOY_HEALTH_RETRIES:-30}"

FORCE=0
DRY_RUN=0
for arg in "$@"; do
  case "$arg" in
    --force)   FORCE=1 ;;
    --dry-run) DRY_RUN=1 ;;
    *) echo "unknown flag: $arg" >&2; exit 2 ;;
  esac
done

log() { printf '%s [deploy] %s\n' "$(date -Is)" "$*"; }
die() { log "ERROR: $*"; exit "${2:-2}"; }

# Serialise: a timer firing while a 15-minute rebuild is running must not
# start a second one.
exec 9>"$LOCK_FILE" || die "cannot open lock file $LOCK_FILE"
flock -n 9 || { log "another deploy is in progress; exiting"; exit 0; }

cd "$REPO_ROOT"

# Never deploy over uncommitted local work — it would be silently reverted by
# the reset below, and on this host that could mean losing hand-edited config.
if [[ -n "$(git status --porcelain --untracked-files=no)" ]]; then
  die "working tree has uncommitted changes; refusing to deploy"
fi

log "fetching $REMOTE/$BRANCH"
git fetch --quiet "$REMOTE" "$BRANCH"

LOCAL_SHA="$(git rev-parse HEAD)"
REMOTE_SHA="$(git rev-parse "$REMOTE/$BRANCH")"

if [[ "$LOCAL_SHA" == "$REMOTE_SHA" && "$FORCE" -eq 0 ]]; then
  log "already up to date at ${LOCAL_SHA:0:9}"
  exit 0
fi

log "update: ${LOCAL_SHA:0:9} -> ${REMOTE_SHA:0:9}"
git --no-pager log --oneline "$LOCAL_SHA..$REMOTE_SHA" 2>/dev/null | sed 's/^/           /' || true

if [[ "$DRY_RUN" -eq 1 ]]; then
  log "dry run — stopping before any change"
  exit 0
fi

rollback() {
  log "rolling back to ${LOCAL_SHA:0:9}"
  git reset --hard --quiet "$LOCAL_SHA"
  if ( cd "$STARTER_DIR" && make rebuild ) >/dev/null 2>&1; then
    log "rollback rebuilt previous revision"
  else
    log "ROLLBACK REBUILD FAILED — stack may be down, manual intervention needed"
  fi
}

log "checking out ${REMOTE_SHA:0:9}"
git reset --hard --quiet "$REMOTE_SHA"

log "rebuilding stack"
if ! ( cd "$STARTER_DIR" && make rebuild ); then
  log "build failed"
  rollback
  exit 1
fi

log "waiting for health at $HEALTH_URL"
for ((i = 1; i <= HEALTH_RETRIES; i++)); do
  if curl -sf -o /dev/null "$HEALTH_URL"; then
    log "healthy — deployed ${REMOTE_SHA:0:9}"
    exit 0
  fi
  sleep 2
done

log "health check failed after $((HEALTH_RETRIES * 2))s"
rollback
exit 1
