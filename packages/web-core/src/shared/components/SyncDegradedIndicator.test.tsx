import { renderToStaticMarkup } from 'react-dom/server';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const useProjectContext = vi.fn();

vi.mock('@/shared/hooks/useProjectContext', () => ({ useProjectContext }));

describe('SyncDegradedIndicator', () => {
  beforeEach(() => {
    vi.resetModules();
    useProjectContext.mockReset();
  });

  it('appears after a stream failure and clears after recovery', async () => {
    const { SyncDegradedIndicator } = await import('./SyncDegradedIndicator');
    useProjectContext.mockReturnValue({
      isSyncDegraded: true,
      syncDegradedSince: Date.now() - 120_000,
    });

    const degraded = renderToStaticMarkup(<SyncDegradedIndicator />);
    expect(degraded).toContain('Data is not live');
    expect(degraded).toContain('about 2 min');
    expect(degraded).toContain('role="status"');

    useProjectContext.mockReturnValue({
      isSyncDegraded: false,
      syncDegradedSince: null,
    });
    expect(renderToStaticMarkup(<SyncDegradedIndicator />)).toBe('');
  });
});
