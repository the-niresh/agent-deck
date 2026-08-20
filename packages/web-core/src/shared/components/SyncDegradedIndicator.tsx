import { useEffect, useState } from 'react';
import { useProjectContext } from '@/shared/hooks/useProjectContext';

export function SyncDegradedIndicator() {
  const { isSyncDegraded, syncDegradedSince } = useProjectContext();
  const [now, setNow] = useState(Date.now());

  useEffect(() => {
    if (!isSyncDegraded) return;
    const interval = globalThis.setInterval(() => setNow(Date.now()), 60_000);
    return () => globalThis.clearInterval(interval);
  }, [isSyncDegraded]);

  if (!isSyncDegraded || !syncDegradedSince) return null;

  const minutes = Math.max(1, Math.floor((now - syncDegradedSince) / 60_000));
  const staleFor =
    minutes < 60 ? `${minutes} min` : `${Math.floor(minutes / 60)} hr`;

  return (
    <div
      role="status"
      aria-live="polite"
      tabIndex={0}
      className="m-base rounded border border-error/50 bg-error/10 px-base py-half text-base text-high focus:outline-none focus:ring-1 focus:ring-brand"
    >
      Data is not live. It may be stale for about {staleFor}.
    </div>
  );
}
