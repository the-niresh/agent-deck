import type { ErrorEvent, EventHint } from '@sentry/react';

/**
 * Home-path redaction for Sentry events, ported from the Rust side's
 * `redact_home_paths` in `crates/utils/src/sentry.rs`.
 *
 * The two halves used to have opposite privacy postures against the same Sentry
 * org: the backend stripped home paths, repo paths, branch names and every
 * credential, and the browser stripped nothing at all. Agent Deck's UI displays
 * worktree paths constantly, so a frontend stack trace or breadcrumb could ship
 * exactly what the backend works to remove.
 *
 * Scope is deliberately the home-path regex only (AGENT-DECK-35). The backend's
 * `should_redact_literal` treats any 8+ character env value under a
 * TOKEN|SECRET|KEY|... key as sensitive and replaces it globally in the event
 * text, which over-redacts and makes events confusing to read. Porting it
 * verbatim would port that too, and the browser has no env vars to read anyway.
 */

// Kept character for character in step with the Rust regex so the two sides
// redact the same spans. `/root` has no username segment, hence the third arm.
const HOME_PATH_RE =
  /(?:\/home\/[^/\s"'<>]+|\/Users\/[^/\s"'<>]+|\/root)(?:\/[^\s"'<>]*)?/g;

const REDACTED = '[REDACTED_HOME_PATH]';

export function redactHomePaths(value: string): string {
  return value.replace(HOME_PATH_RE, REDACTED);
}

/**
 * Walks every string in an already-serialised event. Mirrors the Rust
 * `scrub_json_value`: the payload shape is Sentry's, not ours, and new fields
 * appear as the SDK changes. Walking everything means a new field carrying a
 * path is covered on the day it appears rather than the day someone notices.
 */
function scrubValue(value: unknown, seen: WeakSet<object>): unknown {
  if (typeof value === 'string') return redactHomePaths(value);

  if (Array.isArray(value)) {
    // A cycle would make this recurse forever inside beforeSend, on the error
    // path, where the process is already unhealthy. Bail rather than hang.
    if (seen.has(value)) return value;
    seen.add(value);
    for (let i = 0; i < value.length; i += 1) {
      value[i] = scrubValue(value[i], seen);
    }
    return value;
  }

  if (value !== null && typeof value === 'object') {
    if (seen.has(value)) return value;
    seen.add(value);
    const record = value as Record<string, unknown>;
    for (const key of Object.keys(record)) {
      record[key] = scrubValue(record[key], seen);
    }
    return record;
  }

  return value;
}

/**
 * `beforeSend` hook. Mutates in place: Sentry hands us the event it is about to
 * send and takes back whatever we return, so a copy buys nothing.
 *
 * Never throws. A scrubber that throws inside `beforeSend` drops the event, and
 * losing crash reports to a bug in the privacy filter is the worst of both
 * outcomes - so on failure we drop the event deliberately (returning null)
 * rather than let an unscrubbed one through.
 */
export function scrubEvent(
  event: ErrorEvent,
  _hint?: EventHint
): ErrorEvent | null {
  try {
    return scrubValue(event, new WeakSet()) as ErrorEvent;
  } catch {
    return null;
  }
}
