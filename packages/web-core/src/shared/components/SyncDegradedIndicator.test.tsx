// @vitest-environment jsdom
import { act, createContext } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

type SyncParams = {
  collection: {
    isReady: () => boolean;
    onFirstReady: (callback: () => void) => void;
  };
  begin: () => void;
  write: () => void;
  commit: () => void;
  markReady: () => void;
  truncate: () => void;
};
const rig = vi.hoisted(() => ({
  shapeOptions: new Map<string, Record<string, unknown>>(),
  cleanup: vi.fn(),
}));

vi.mock('@tanstack/electric-db-collection', () => ({
  electricCollectionOptions: (options: {
    shapeOptions: Record<string, unknown>;
  }) => {
    const table = (options.shapeOptions.url as string)
      .split('/')
      .at(-1) as string;
    rig.shapeOptions.set(table, options.shapeOptions);
    return {
      sync: {
        sync: (params: SyncParams) => {
          params.collection.onFirstReady(() => {});
          return { cleanup: rig.cleanup, loadSubset: () => true };
        },
      },
    };
  },
}));
vi.mock('@tanstack/react-db', () => ({
  createCollection: (options: {
    sync: { sync: (params: SyncParams) => unknown };
  }) => {
    options.sync.sync({
      collection: { isReady: () => false, onFirstReady: () => {} },
      begin: () => {},
      write: () => {},
      commit: () => {},
      markReady: () => {},
      truncate: () => {},
    });
    return {};
  },
  useLiveQuery: () => ({ data: [], isLoading: false }),
}));
vi.mock('@/shared/lib/auth/runtime', () => ({
  getAuthRuntime: () => ({
    getToken: async () => 'test-token',
    registerShape: () => {},
    triggerRefresh: async () => {},
  }),
}));
vi.mock('@/shared/hooks/useSyncErrorContext', () => ({
  useSyncErrorContext: () => null,
}));
vi.mock('@/shared/lib/hmrContext', () => ({
  createHmrContext: <T,>(_: string, defaultValue: T) =>
    createContext<T>(defaultValue),
}));
vi.mock('@/shared/lib/remoteApi', () => ({
  getRemoteApiUrl: () => 'http://remote.test',
  makeRequest: async () => new Response(JSON.stringify({ issues: [] })),
}));

async function flushEffects() {
  await act(async () => {
    for (let i = 0; i < 10; i += 1) {
      await Promise.resolve();
    }
  });
}

describe('SyncDegradedIndicator', () => {
  let container: HTMLDivElement;
  let root: Root;
  beforeEach(() => {
    globalThis.IS_REACT_ACT_ENVIRONMENT = true;
    vi.resetModules();
    vi.clearAllMocks();
    rig.shapeOptions.clear();
    vi.useFakeTimers();
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => new Response('{}', { status: 200 }))
    );
    container = document.createElement('div');
    document.body.append(container);
    root = createRoot(container);
  });
  afterEach(() => {
    root.unmount();
    container.remove();
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it('shows fallback status, clears after recovery, and does not rerender on retry state', async () => {
    const { ProjectProvider } = await import(
      '@/shared/providers/remote/ProjectProvider'
    );
    const { SyncDegradedIndicator } = await import('./SyncDegradedIndicator');
    const { useProjectContext } = await import(
      '@/shared/hooks/useProjectContext'
    );
    const collections = await import('@/shared/lib/electric/collections');
    const subscribeToStatus = collections.subscribeToShapeSourceStatus;
    const unsubscribe = vi.fn();
    const subscribe = vi
      .spyOn(collections, 'subscribeToShapeSourceStatus')
      .mockImplementation((...args) => {
        const cleanup = subscribeToStatus(...args);
        return () => {
          unsubscribe();
          cleanup();
        };
      });
    let boardRenders = 0;
    function Board() {
      useProjectContext();
      boardRenders += 1;
      return <SyncDegradedIndicator />;
    }
    await act(async () => {
      root.render(
        <ProjectProvider projectId="project-1">
          <Board />
        </ProjectProvider>
      );
    });
    await flushEffects();
    const liveRegion = container.querySelector('[role="status"]');
    expect(liveRegion).not.toBeNull();
    expect(liveRegion?.textContent).toBe('');
    const issueOptions = rig.shapeOptions.get('issues');
    expect(issueOptions).toBeDefined();
    vi.mocked(globalThis.fetch).mockResolvedValueOnce(
      new Response('unavailable', { status: 503 })
    );
    await act(async () => {
      await (issueOptions?.fetchClient as (input: string) => Promise<Response>)(
        'http://remote.test/v1/shape'
      );
      (
        issueOptions?.onError as (error: {
          status: number;
          message: string;
        }) => void
      )({ status: 503, message: 'Electric is unavailable' });
    });
    await flushEffects();
    expect(container.textContent).toContain('Data is not live');
    expect(container.textContent).toContain('less than a minute');
    await act(async () => {
      await vi.advanceTimersByTimeAsync(999);
    });
    await flushEffects();
    const rendersWhileDegraded = boardRenders;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    await flushEffects();
    expect(boardRenders).toBe(rendersWhileDegraded);
    await act(async () => {
      await (issueOptions?.fetchClient as (input: string) => Promise<Response>)(
        'http://remote.test/v1/shape'
      );
    });
    await flushEffects();
    expect(container.textContent).not.toContain('Data is not live');
    root.unmount();
    expect(subscribe).toHaveBeenCalledTimes(10);
    expect(unsubscribe).toHaveBeenCalledTimes(10);
    expect(rig.cleanup).toHaveBeenCalled();
  });
});
