import { beforeEach, describe, expect, it, vi } from 'vitest';

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

const electricSync = vi.fn();
let capturedShapeOptions: Record<string, unknown> | undefined;

vi.mock('@tanstack/electric-db-collection', () => ({
  electricCollectionOptions: (options: {
    shapeOptions: Record<string, unknown>;
  }) => {
    capturedShapeOptions = options.shapeOptions;
    return { sync: { sync: electricSync } };
  },
}));

vi.mock('@tanstack/react-db', () => ({
  createCollection: (options: {
    sync: { sync: (params: SyncParams) => unknown };
  }) => {
    options.sync.sync({
      collection: {
        isReady: () => false,
        onFirstReady: () => {},
      },
      begin: () => {},
      write: () => {},
      commit: () => {},
      markReady: () => {},
      truncate: () => {},
    });
    return {};
  },
}));

vi.mock('@/shared/lib/auth/runtime', () => ({
  getAuthRuntime: () => ({
    getToken: async () => 'test-token',
    registerShape: () => {},
    triggerRefresh: async () => {},
  }),
}));

vi.mock('@/shared/lib/remoteApi', () => ({
  getRemoteApiUrl: () => 'http://remote.test',
  makeRequest: async () =>
    new Response(JSON.stringify({ projects: [] }), { status: 200 }),
}));

describe('createShapeCollection', () => {
  beforeEach(() => {
    vi.resetModules();
    vi.clearAllMocks();
    capturedShapeOptions = undefined;
    vi.useFakeTimers();
    vi.stubGlobal('document', { visibilityState: 'visible' });
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => new Response('{}', { status: 200 }))
    );
  });

  it('resumes Electric after a source falls back', async () => {
    electricSync.mockImplementation(() => {
      if (electricSync.mock.calls.length === 2) {
        const fetchClient = capturedShapeOptions?.fetchClient as (
          input: string
        ) => Promise<Response>;
        void fetchClient('http://remote.test/v1/shape');
      }
      return {};
    });

    const { createShapeCollection, subscribeToShapeSourceStatus } =
      await import('./collections');
    const shape = {
      table: 'projects',
      url: '/shape/projects',
      fallbackUrl: '/fallback/projects',
    } as never;
    const params = { organization_id: 'org-1' };
    const statusListener = vi.fn();
    const unsubscribe = subscribeToShapeSourceStatus(
      shape,
      params,
      statusListener
    );

    createShapeCollection(shape, params);

    const onError = capturedShapeOptions?.onError as (error: {
      status: number;
      message: string;
    }) => void;
    onError({ status: 503, message: 'Electric is unavailable' });

    expect(statusListener).toHaveBeenCalledWith({
      mode: 'fallback',
      isDegraded: true,
      retryAttempt: 0,
    });

    await vi.advanceTimersByTimeAsync(1000);

    expect(electricSync).toHaveBeenCalledTimes(2);
    expect(statusListener).toHaveBeenLastCalledWith({
      mode: 'electric',
      isDegraded: false,
      retryAttempt: 0,
    });

    unsubscribe();
  });
});
