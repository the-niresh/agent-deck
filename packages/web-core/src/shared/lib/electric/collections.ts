import { electricCollectionOptions } from '@tanstack/electric-db-collection';
import { createCollection } from '@tanstack/react-db';

import { getAuthRuntime } from '@/shared/lib/auth/runtime';
import { getRemoteApiUrl, makeRequest } from '@/shared/lib/remoteApi';
import type { MutationDefinition, ShapeDefinition } from 'shared/remote-types';
import type {
  CollectionConfig,
  SyncError,
  SyncSourceStatus,
} from '@/shared/lib/electric/types';

type ElectricRow = Record<string, unknown> & { [key: string]: unknown };

type SourceMode = 'electric' | 'fallback';

type SourceRuntime = {
  mode: SourceMode;
  fallbackLocked: boolean;
  refreshers: Set<() => Promise<void>>;
  fallbackSwitchers: Set<() => void>;
  recoveryStarters: Set<() => void>;
  statusListeners: Set<(status: SyncSourceStatus) => void>;
  recoveryTimer: ReturnType<typeof globalThis.setTimeout> | null;
  retryAttempt: number;
};

type MutationFnParams = {
  transaction: {
    mutations: Array<{
      modified?: unknown;
      original?: unknown;
      key?: string;
      changes?: unknown;
    }>;
  };
};

type SyncParams = {
  collection: {
    isReady: () => boolean;
    onFirstReady: (callback: () => void) => void;
  };
  begin: () => void;
  write: (message: {
    type: 'insert' | 'update' | 'delete';
    value: ElectricRow;
    metadata?: Record<string, unknown>;
  }) => void;
  commit: () => void;
  markReady: () => void;
  truncate: () => void;
};

type LoadSubsetFn = (options: unknown) => true | Promise<void>;
type UnloadSubsetFn = (options: unknown) => void;

type SyncResult =
  | void
  | (() => void)
  | {
      cleanup?: () => void;
      loadSubset?: LoadSubsetFn;
      unloadSubset?: UnloadSubsetFn;
    };

type NormalizedSyncResult = {
  cleanup?: () => void;
  loadSubset?: LoadSubsetFn;
  unloadSubset?: UnloadSubsetFn;
};

type SyncConfigLike = {
  sync: (syncParams: SyncParams) => SyncResult;
  getSyncMetadata?: () => Record<string, unknown>;
  rowUpdateMode?: 'partial' | 'full';
};

const DEFAULT_GC_TIME_MS = 5 * 60 * 1000;
const ELECTRIC_READY_TIMEOUT_MS = 3000;
const FALLBACK_REFRESH_INTERVAL_MS = 30 * 1000;
const ELECTRIC_RECOVERY_INITIAL_DELAY_MS = 1000;
const ELECTRIC_RECOVERY_MAX_DELAY_MS = 30 * 1000;

const collectionCache = new Map<string, ReturnType<typeof createCollection>>();
const sourceRuntimes = new Map<string, SourceRuntime>();
const fallbackSnapshotCache = new Map<string, ElectricRow[]>();

class ErrorHandler {
  private lastErrorTime = 0;
  private lastErrorMessage = '';
  private consecutiveErrors = 0;
  private readonly baseDebounceMs = 1000;
  private readonly maxDebounceMs = 30000;

  shouldReport(message: string): boolean {
    const now = Date.now();
    const debounceMs = Math.min(
      this.baseDebounceMs * Math.pow(2, this.consecutiveErrors),
      this.maxDebounceMs
    );

    if (
      message === this.lastErrorMessage &&
      now - this.lastErrorTime < debounceMs
    ) {
      return false;
    }

    this.lastErrorTime = now;
    if (message === this.lastErrorMessage) {
      this.consecutiveErrors += 1;
    } else {
      this.consecutiveErrors = 0;
      this.lastErrorMessage = message;
    }

    return true;
  }
}

function buildUrl(baseUrl: string, params: Record<string, string>): string {
  let url = baseUrl;
  for (const [key, value] of Object.entries(params)) {
    url = url.replace(`{${key}}`, encodeURIComponent(value));
  }
  return url;
}

function buildFallbackRequestPath(
  fallbackUrl: string,
  params: Record<string, string>
): string {
  const path = buildUrl(fallbackUrl, params);
  const query = new URLSearchParams();

  for (const [key, value] of Object.entries(params)) {
    if (!value) continue;
    query.set(key, value);
  }

  const queryString = query.toString();
  return queryString ? `${path}?${queryString}` : path;
}

function buildCollectionId(
  table: string,
  params: Record<string, string>,
  hasMutations: boolean
): string {
  const sortedParams = Object.keys(params)
    .sort()
    .map((key) => params[key])
    .join('-');

  const base = sortedParams ? `${table}-${sortedParams}` : table;
  return hasMutations ? `${base}-mut` : base;
}

function buildSourceKey(table: string, params: Record<string, string>): string {
  const sortedEntries = Object.entries(params).sort(([a], [b]) =>
    a.localeCompare(b)
  );
  if (sortedEntries.length === 0) {
    return table;
  }

  const values = sortedEntries
    .map(([key, value]) => `${key}=${value}`)
    .join('&');
  return `${table}?${values}`;
}

function getRowKey(item: Record<string, unknown>): string {
  if ('id' in item && item.id) {
    return String(item.id);
  }

  return Object.entries(item)
    .filter(([key]) => key.endsWith('_id'))
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([, value]) => String(value))
    .join('-');
}

function normalizeSyncResult(result: SyncResult): NormalizedSyncResult {
  if (!result) return {};
  if (typeof result === 'function') {
    return { cleanup: result };
  }
  return result;
}

function getOrCreateSourceRuntime(sourceKey: string): SourceRuntime {
  const existing = sourceRuntimes.get(sourceKey);
  if (existing) {
    return existing;
  }

  const created: SourceRuntime = {
    mode: 'electric',
    fallbackLocked: false,
    refreshers: new Set(),
    fallbackSwitchers: new Set(),
    recoveryStarters: new Set(),
    statusListeners: new Set(),
    recoveryTimer: null,
    retryAttempt: 0,
  };
  sourceRuntimes.set(sourceKey, created);
  return created;
}

function getSourceStatus(runtime: SourceRuntime): SyncSourceStatus {
  return {
    mode: runtime.mode,
    isDegraded: runtime.fallbackLocked,
    retryAttempt: runtime.retryAttempt,
  };
}

function notifySourceStatus(runtime: SourceRuntime): void {
  const status = getSourceStatus(runtime);
  for (const listener of runtime.statusListeners) {
    listener(status);
  }
}

function clearSourceRecoveryTimer(runtime: SourceRuntime): void {
  if (runtime.recoveryTimer) {
    globalThis.clearTimeout(runtime.recoveryTimer);
    runtime.recoveryTimer = null;
  }
}

function scheduleSourceRecovery(sourceKey: string): void {
  const runtime = getOrCreateSourceRuntime(sourceKey);
  if (
    !runtime.fallbackLocked ||
    runtime.recoveryTimer ||
    runtime.recoveryStarters.size === 0
  ) {
    return;
  }

  const delay = Math.min(
    ELECTRIC_RECOVERY_INITIAL_DELAY_MS * Math.pow(2, runtime.retryAttempt),
    ELECTRIC_RECOVERY_MAX_DELAY_MS
  );

  runtime.recoveryTimer = globalThis.setTimeout(() => {
    runtime.recoveryTimer = null;
    if (!runtime.fallbackLocked || runtime.recoveryStarters.size === 0) {
      return;
    }

    runtime.retryAttempt += 1;
    notifySourceStatus(runtime);

    for (const starter of Array.from(runtime.recoveryStarters)) {
      starter();
    }
  }, delay);
}

function lockSourceToFallback(sourceKey: string): void {
  const runtime = getOrCreateSourceRuntime(sourceKey);
  const wasFallbackLocked = runtime.fallbackLocked;

  runtime.fallbackLocked = true;
  runtime.mode = 'fallback';

  if (!wasFallbackLocked) {
    runtime.retryAttempt = 0;
    notifySourceStatus(runtime);
  }

  const switchers = Array.from(runtime.fallbackSwitchers);
  for (const switcher of switchers) {
    switcher();
  }

  scheduleSourceRecovery(sourceKey);
}

function unlockSourceFallback(sourceKey: string): void {
  const runtime = getOrCreateSourceRuntime(sourceKey);
  if (!runtime.fallbackLocked) return;

  runtime.fallbackLocked = false;
  runtime.mode = 'electric';
  runtime.retryAttempt = 0;
  clearSourceRecoveryTimer(runtime);
  notifySourceStatus(runtime);
}

function registerFallbackSwitcher(
  sourceKey: string,
  switcher: () => void
): () => void {
  const runtime = getOrCreateSourceRuntime(sourceKey);
  runtime.fallbackSwitchers.add(switcher);

  if (runtime.fallbackLocked) {
    switcher();
  }

  return () => {
    runtime.fallbackSwitchers.delete(switcher);
  };
}

function registerRecoveryStarter(
  sourceKey: string,
  starter: () => void
): () => void {
  const runtime = getOrCreateSourceRuntime(sourceKey);
  runtime.recoveryStarters.add(starter);
  scheduleSourceRecovery(sourceKey);

  return () => {
    runtime.recoveryStarters.delete(starter);
    if (runtime.recoveryStarters.size === 0) {
      clearSourceRecoveryTimer(runtime);
    }
  };
}

/**
 * Subscribe to a shape source's transport status.
 * This is for UI state only. Collection data and callers without a listener
 * keep their existing behavior.
 */
export function subscribeToShapeSourceStatus(
  shape: Pick<ShapeDefinition<unknown>, 'table'>,
  params: Record<string, string>,
  listener: (status: SyncSourceStatus) => void
): () => void {
  const runtime = getOrCreateSourceRuntime(buildSourceKey(shape.table, params));
  runtime.statusListeners.add(listener);
  listener(getSourceStatus(runtime));

  return () => {
    runtime.statusListeners.delete(listener);
  };
}

function registerFallbackRefresher(
  sourceKey: string,
  refresher: () => Promise<void>
): () => void {
  const runtime = getOrCreateSourceRuntime(sourceKey);
  runtime.refreshers.add(refresher);
  return () => {
    runtime.refreshers.delete(refresher);
  };
}

function invalidateFallbackCache(sourceKey: string): void {
  fallbackSnapshotCache.delete(sourceKey);
}

function refreshFallbackSource(sourceKey: string): void {
  const runtime = getOrCreateSourceRuntime(sourceKey);
  for (const refresher of runtime.refreshers) {
    void refresher();
  }
}

function isAbortError(error: unknown): boolean {
  return error instanceof DOMException && error.name === 'AbortError';
}

function isPageVisible(): boolean {
  return document.visibilityState === 'visible';
}

function isCancelledErrorMessage(message?: string): boolean {
  if (!message) return false;
  return /\bcancell?ed\b/i.test(message);
}

function isTransientElectricFailure(error: unknown): boolean {
  if (isAbortError(error)) return true;
  if (!isPageVisible()) return true;

  const message = error instanceof Error ? error.message : String(error);
  return isCancelledErrorMessage(message);
}

function isTransientElectricShapeError(error: {
  name?: string;
  message?: string;
}): boolean {
  if (error.name === 'AbortError') return true;
  if (!isPageVisible()) return true;
  return isCancelledErrorMessage(error.message);
}

function createErrorReporter(
  config?: CollectionConfig
): (error: SyncError) => void {
  const handler = new ErrorHandler();

  return (error: SyncError) => {
    if (!handler.shouldReport(error.message)) return;

    if (isPageVisible()) {
      console.error('Shape sync error:', error);
    }
    config?.onError?.(error);
  };
}

function createErrorHandlingFetch(args: {
  onError: (error: SyncError) => void;
  onElectricUnavailable: () => void;
  onElectricAvailable: () => void;
  isPaused: () => boolean;
}) {
  return async (
    input: RequestInfo | URL,
    init?: RequestInit
  ): Promise<Response> => {
    if (args.isPaused()) {
      throw new DOMException(
        'Shape request aborted: not authenticated',
        'AbortError'
      );
    }

    try {
      const response = await fetch(input, init);
      if (response.ok) {
        args.onElectricAvailable();
      }
      return response;
    } catch (error) {
      if (isTransientElectricFailure(error)) {
        throw error;
      }

      const message = error instanceof Error ? error.message : 'Network error';
      args.onError({ message });
      args.onElectricUnavailable();
      throw error;
    }
  };
}

function createElectricShapeOptions(args: {
  shape: ShapeDefinition<unknown>;
  params: Record<string, string>;
  reportError: (error: SyncError) => void;
  onElectricUnavailable: () => void;
  onElectricAvailable: () => void;
}) {
  const authRuntime = getAuthRuntime();
  let isPaused = false;

  authRuntime.registerShape({
    pause: () => {
      isPaused = true;
    },
    resume: () => {
      isPaused = false;
    },
  });

  const url = buildUrl(args.shape.url, args.params);

  return {
    url: `${getRemoteApiUrl()}${url}`,
    params: args.params,
    headers: {
      Authorization: async () => {
        const token = await authRuntime.getToken();
        if (!token) {
          isPaused = true;
          return '';
        }
        return `Bearer ${token}`;
      },
    },
    parser: {
      timestamptz: (value: string) => value,
    },
    fetchClient: createErrorHandlingFetch({
      onError: args.reportError,
      onElectricUnavailable: args.onElectricUnavailable,
      onElectricAvailable: args.onElectricAvailable,
      isPaused: () => isPaused,
    }),
    onError: (error: { status?: number; message?: string; name?: string }) => {
      if (isPaused) return;
      if (isTransientElectricShapeError(error)) return;

      const status = error.status;
      const message = error.message || String(error);

      if (status === 401) {
        authRuntime.triggerRefresh().catch(() => {
          args.reportError({ status, message });
        });
        return;
      }

      args.reportError({ status, message });

      if (status === undefined || status >= 500) {
        args.onElectricUnavailable();
      }
    },
  };
}

function applySnapshot(syncParams: SyncParams, rows: ElectricRow[]): void {
  syncParams.begin();
  syncParams.truncate();

  for (const row of rows) {
    syncParams.write({
      type: 'insert',
      value: row,
      metadata: {},
    });
  }

  syncParams.commit();
  syncParams.markReady();
}

function extractFallbackRows(
  payload: unknown,
  table: string
): Array<ElectricRow> {
  if (!payload || typeof payload !== 'object') {
    throw new Error(`Fallback response for "${table}" is not an object`);
  }

  const rows = (payload as Record<string, unknown>)[table];
  if (!Array.isArray(rows)) {
    throw new Error(`Fallback response missing "${table}" array`);
  }

  return rows as Array<ElectricRow>;
}

async function parseResponseError(
  response: Response,
  fallbackMessage: string
): Promise<string> {
  try {
    const body = (await response.json()) as {
      message?: string;
      error?: string;
    };
    return body.message || body.error || fallbackMessage;
  } catch {
    return fallbackMessage;
  }
}

function createFallbackSync(args: {
  sourceKey: string;
  shape: ShapeDefinition<unknown>;
  params: Record<string, string>;
  reportError: (error: SyncError) => void;
}) {
  return (syncParams: SyncParams): SyncResult => {
    const runtime = getOrCreateSourceRuntime(args.sourceKey);
    runtime.mode = 'fallback';

    let isCleanedUp = false;
    let refreshPromise: Promise<void> | null = null;

    const refreshNow = async () => {
      if (refreshPromise) {
        return refreshPromise;
      }

      refreshPromise = (async () => {
        try {
          const response = await makeRequest(
            buildFallbackRequestPath(args.shape.fallbackUrl, args.params),
            { method: 'GET', cache: 'no-store' }
          );

          if (!response.ok) {
            const message = await parseResponseError(
              response,
              `Failed to fetch fallback ${args.shape.table}`
            );
            throw new Error(message);
          }

          const payload = (await response.json()) as unknown;
          const rows = extractFallbackRows(payload, args.shape.table);
          fallbackSnapshotCache.set(args.sourceKey, rows);

          if (!isCleanedUp) {
            applySnapshot(syncParams, rows);
          }
        } catch (error) {
          if (isAbortError(error)) return;

          const message =
            error instanceof Error ? error.message : 'Fallback fetch failed';
          args.reportError({ message });

          if (!isCleanedUp && !syncParams.collection.isReady()) {
            syncParams.markReady();
          }
        } finally {
          refreshPromise = null;
        }
      })();

      return refreshPromise;
    };

    const unregisterRefresher = registerFallbackRefresher(
      args.sourceKey,
      refreshNow
    );

    const cachedRows = fallbackSnapshotCache.get(args.sourceKey);
    if (cachedRows) {
      applySnapshot(syncParams, cachedRows);
    }

    void refreshNow();

    const intervalId = globalThis.setInterval(() => {
      void refreshNow();
    }, FALLBACK_REFRESH_INTERVAL_MS);

    return {
      cleanup: () => {
        isCleanedUp = true;
        globalThis.clearInterval(intervalId);
        unregisterRefresher();
      },
      loadSubset: () => true,
    };
  };
}

function createHybridSync(args: {
  sourceKey: string;
  shape: ShapeDefinition<unknown>;
  params: Record<string, string>;
  reportError: (error: SyncError) => void;
  electricSync: SyncConfigLike['sync'];
}) {
  const fallbackSync = createFallbackSync({
    sourceKey: args.sourceKey,
    shape: args.shape,
    params: args.params,
    reportError: args.reportError,
  });

  return (syncParams: SyncParams): SyncResult => {
    const runtime = getOrCreateSourceRuntime(args.sourceKey);

    let isCleanedUp = false;
    let usingFallback = runtime.fallbackLocked;
    let timeoutId: ReturnType<typeof globalThis.setTimeout> | null = null;

    let activeSync = usingFallback
      ? normalizeSyncResult(fallbackSync(syncParams))
      : normalizeSyncResult(args.electricSync(syncParams));

    const switchToFallback = () => {
      if (isCleanedUp || usingFallback) return;
      usingFallback = true;

      activeSync.cleanup?.();
      activeSync = normalizeSyncResult(fallbackSync(syncParams));
    };

    const startElectricRecovery = () => {
      if (isCleanedUp || !usingFallback) return;

      activeSync.cleanup?.();
      usingFallback = false;
      activeSync = normalizeSyncResult(args.electricSync(syncParams));
      scheduleReadyTimeout();
    };

    const unregisterSwitcher = registerFallbackSwitcher(
      args.sourceKey,
      switchToFallback
    );
    const unregisterRecoveryStarter = registerRecoveryStarter(
      args.sourceKey,
      startElectricRecovery
    );

    const scheduleReadyTimeout = () => {
      timeoutId = globalThis.setTimeout(() => {
        if (isCleanedUp || usingFallback || syncParams.collection.isReady()) {
          return;
        }

        if (!isPageVisible()) {
          scheduleReadyTimeout();
          return;
        }

        args.reportError({
          message: `Electric sync timed out after ${ELECTRIC_READY_TIMEOUT_MS}ms, switching to fallback`,
        });
        lockSourceToFallback(args.sourceKey);
      }, ELECTRIC_READY_TIMEOUT_MS);
    };

    scheduleReadyTimeout();

    syncParams.collection.onFirstReady(() => {
      if (!usingFallback) {
        if (timeoutId) {
          globalThis.clearTimeout(timeoutId);
        }
      }
    });

    return {
      cleanup: () => {
        isCleanedUp = true;
        if (timeoutId) {
          globalThis.clearTimeout(timeoutId);
        }
        unregisterSwitcher();
        unregisterRecoveryStarter();
        activeSync.cleanup?.();
      },
      loadSubset: (options: unknown) =>
        activeSync.loadSubset ? activeSync.loadSubset(options) : true,
      unloadSubset: (options: unknown) => {
        activeSync.unloadSubset?.(options);
      },
    };
  };
}

function isSourceFallbackLocked(sourceKey: string): boolean {
  const runtime = getOrCreateSourceRuntime(sourceKey);
  return runtime.fallbackLocked;
}

function maybeRefreshFallbackAfterMutation(sourceKey: string): void {
  if (!isSourceFallbackLocked(sourceKey)) return;
  invalidateFallbackCache(sourceKey);
  refreshFallbackSource(sourceKey);
}

function buildMutationHandlers(
  mutation: MutationDefinition<unknown, unknown, unknown>,
  sourceKey: string
) {
  return {
    onInsert: async ({
      transaction,
    }: MutationFnParams): Promise<{ txid: number[] } | void> => {
      const txids = await Promise.all(
        transaction.mutations.map(async (mutationItem) => {
          const data = mutationItem.modified as Record<string, unknown>;
          const response = await makeRequest(mutation.url, {
            method: 'POST',
            body: JSON.stringify(data),
          });

          if (!response.ok) {
            const message = await parseResponseError(
              response,
              `Failed to create ${mutation.name}`
            );
            throw new Error(message);
          }

          const result = (await response.json()) as { txid: number };
          return result.txid;
        })
      );

      maybeRefreshFallbackAfterMutation(sourceKey);

      if (isSourceFallbackLocked(sourceKey)) {
        return;
      }

      return { txid: txids };
    },

    onUpdate: async ({
      transaction,
    }: MutationFnParams): Promise<{ txid: number[] } | void> => {
      let txids: number[] = [];

      if (transaction.mutations.length > 1) {
        const updates = transaction.mutations.map((mutationItem) => {
          if (!mutationItem.key) {
            throw new Error(`Failed to update ${mutation.name}: missing key`);
          }

          return {
            id: String(mutationItem.key),
            ...(mutationItem.changes as Record<string, unknown>),
          };
        });

        const response = await makeRequest(`${mutation.url}/bulk`, {
          method: 'POST',
          body: JSON.stringify({ updates }),
        });

        if (!response.ok) {
          const message = await parseResponseError(
            response,
            `Failed to bulk update ${mutation.name}`
          );
          throw new Error(message);
        }

        const result = (await response.json()) as { txid: number };
        txids = [result.txid];
      } else {
        const mutationItem = transaction.mutations[0];
        if (!mutationItem?.key) {
          throw new Error(`Failed to update ${mutation.name}: missing key`);
        }

        const response = await makeRequest(
          `${mutation.url}/${mutationItem.key}`,
          {
            method: 'PATCH',
            body: JSON.stringify(mutationItem.changes),
          }
        );

        if (!response.ok) {
          const message = await parseResponseError(
            response,
            `Failed to update ${mutation.name}`
          );
          throw new Error(message);
        }

        const result = (await response.json()) as { txid: number };
        txids = [result.txid];
      }

      maybeRefreshFallbackAfterMutation(sourceKey);

      if (isSourceFallbackLocked(sourceKey)) {
        return;
      }

      return { txid: txids };
    },

    onDelete: async ({
      transaction,
    }: MutationFnParams): Promise<{ txid: number[] } | void> => {
      const txids = await Promise.all(
        transaction.mutations.map(async (mutationItem) => {
          const response = await makeRequest(
            `${mutation.url}/${mutationItem.key}`,
            {
              method: 'DELETE',
            }
          );

          if (!response.ok) {
            const message = await parseResponseError(
              response,
              `Failed to delete ${mutation.name}`
            );
            throw new Error(message);
          }

          const result = (await response.json()) as { txid: number };
          return result.txid;
        })
      );

      maybeRefreshFallbackAfterMutation(sourceKey);

      if (isSourceFallbackLocked(sourceKey)) {
        return;
      }

      return { txid: txids };
    },
  };
}

export function createShapeCollection<TRow extends ElectricRow>(
  shape: ShapeDefinition<TRow>,
  params: Record<string, string>,
  config?: CollectionConfig,
  mutation?: MutationDefinition<unknown, unknown, unknown>
) {
  const hasMutations = Boolean(mutation);
  const collectionId = buildCollectionId(shape.table, params, hasMutations);
  const sourceKey = buildSourceKey(shape.table, params);

  const cached = collectionCache.get(collectionId);
  if (cached) {
    return cached as typeof cached & { __rowType?: TRow };
  }

  const reportError = createErrorReporter(config);
  const onElectricUnavailable = () => lockSourceToFallback(sourceKey);
  const onElectricAvailable = () => unlockSourceFallback(sourceKey);

  const shapeOptions = createElectricShapeOptions({
    shape,
    params,
    reportError,
    onElectricUnavailable,
    onElectricAvailable,
  });

  const mutationHandlers = mutation
    ? buildMutationHandlers(mutation, sourceKey)
    : {};

  const electricOptions = electricCollectionOptions({
    id: collectionId,
    shapeOptions: shapeOptions as never,
    getKey: (item: ElectricRow) => getRowKey(item),
    gcTime: DEFAULT_GC_TIME_MS,
    ...mutationHandlers,
  } as never);

  const electricSyncConfig = electricOptions.sync as unknown as SyncConfigLike;

  const collectionOptions = {
    ...electricOptions,
    sync: {
      ...electricSyncConfig,
      sync: createHybridSync({
        sourceKey,
        shape,
        params,
        reportError,
        electricSync: electricSyncConfig.sync,
      }),
    },
  };

  const collection = createCollection(
    collectionOptions as never
  ) as unknown as ReturnType<typeof createCollection> & { __rowType?: TRow };

  collectionCache.set(collectionId, collection);
  return collection;
}
