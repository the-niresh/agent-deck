import { describe, it, expect } from 'vitest';
import type { ErrorEvent } from '@sentry/react';
import { redactHomePaths, scrubEvent } from './sentryScrub';

describe('redactHomePaths', () => {
  it('redacts the three home path shapes the Rust side redacts', () => {
    expect(redactHomePaths('failed at /home/niresh/work/agent-deck')).toBe(
      'failed at [REDACTED_HOME_PATH]'
    );
    expect(redactHomePaths('failed at /Users/niresh/work/agent-deck')).toBe(
      'failed at [REDACTED_HOME_PATH]'
    );
    expect(redactHomePaths('failed at /root/work/agent-deck')).toBe(
      'failed at [REDACTED_HOME_PATH]'
    );
  });

  it('redacts every occurrence, not just the first', () => {
    const scrubbed = redactHomePaths(
      'copied /home/niresh/a.txt to /home/niresh/b.txt'
    );
    expect(scrubbed).not.toContain('niresh');
    expect(scrubbed.match(/\[REDACTED_HOME_PATH\]/g)).toHaveLength(2);
  });

  it('stops at a quote or angle bracket so surrounding markup survives', () => {
    expect(redactHomePaths('<a href="/home/niresh/x">open</a>')).toBe(
      '<a href="[REDACTED_HOME_PATH]">open</a>'
    );
  });

  it('leaves paths that are not home paths alone', () => {
    expect(redactHomePaths('failed at /srv/claude/projects/agent-deck')).toBe(
      'failed at /srv/claude/projects/agent-deck'
    );
    expect(redactHomePaths('/homework/notes.txt')).toBe('/homework/notes.txt');
  });
});

describe('scrubEvent', () => {
  it('reaches strings nested anywhere in the event', () => {
    const event = {
      message: 'boom at /home/niresh/work/agent-deck/src/main.tsx',
      exception: {
        values: [
          {
            value: 'ENOENT /Users/niresh/.agent-deck/config.json',
            stacktrace: {
              frames: [{ filename: '/home/niresh/work/agent-deck/a.ts' }],
            },
          },
        ],
      },
      breadcrumbs: [
        { message: 'opened /root/worktrees/feature-x' },
        { message: 'clicked Approve' },
      ],
      extra: { cwd: '/home/niresh/work' },
    } as unknown as ErrorEvent;

    const scrubbed = JSON.stringify(scrubEvent(event));

    expect(scrubbed).not.toContain('niresh');
    expect(scrubbed).not.toContain('/root/worktrees');
    // Content that is not a home path must survive, or the events are useless.
    expect(scrubbed).toContain('clicked Approve');
  });

  it('returns the event rather than dropping it when there is nothing to scrub', () => {
    const event = { message: 'plain failure' } as ErrorEvent;
    expect(scrubEvent(event)).toEqual({ message: 'plain failure' });
  });

  it('survives a cycle instead of recursing forever', () => {
    const event = { message: 'at /home/niresh/x' } as unknown as Record<
      string,
      unknown
    >;
    event.self = event;

    const scrubbed = scrubEvent(event as unknown as ErrorEvent);

    expect(scrubbed).not.toBeNull();
    expect((scrubbed as unknown as Record<string, unknown>).message).toBe(
      'at [REDACTED_HOME_PATH]'
    );
  });
});
