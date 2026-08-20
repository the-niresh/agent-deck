import { useEffect, useState } from 'react';
import { useProjectContext } from '@/shared/hooks/useProjectContext';

export function SyncDegradedIndicator() {
  const { isSyncDegraded, syncDegradedSince } = useProjectContext();
  const [now, setNow] = useState<number | null>(null);

  useEffect(() => {
    if (!isSyncDegraded) {
      setNow(null);
      return;
    }

    setNow(Date.now());
    const interval = globalThis.setInterval(() => setNow(Date.now()), 60_000);
    return () => globalThis.clearInterval(interval);
  }, [isSyncDegraded]);

  const minutes = Math.max(
    0,
    Math.floor(
      ((now ?? syncDegradedSince ?? Date.now()) - (syncDegradedSince ?? 0)) /
        60_000
    )
  );
  const staleFor =
    minutes === 0
      ? 'less than a minute'
      : minutes < 60
        ? `${minutes} min`
        : `${Math.floor(minutes / 60)} hr`;

  return (
    <div
      role="status"
      aria-live="polite"
      className={
        isSyncDegraded
          ? 'm-base rounded border border-error/50 bg-error/10 px-base py-half text-base text-high'
          : 'sr-only'
      }
    >
      {isSyncDegraded && syncDegradedSince
        ? `Data is not live. It may be stale for about ${staleFor}.`
        : null}
    </div>
  );
}
