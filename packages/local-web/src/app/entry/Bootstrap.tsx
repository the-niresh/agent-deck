import React from 'react';
import ReactDOM from 'react-dom/client';
import * as Sentry from '@sentry/react';
import { ClickToComponent } from 'click-to-react-component';
import { QueryClientProvider } from '@tanstack/react-query';
import posthog from 'posthog-js';
import { PostHogProvider } from 'posthog-js/react';
import App from '@web/app/entry/App';
import { CrashScreen } from '@agent-deck/ui/components/CrashScreen';
import '@/i18n';
import { router } from '@web/app/router';
import { oauthApi } from '@/shared/lib/api';
import { tokenManager } from '@/shared/lib/auth/tokenManager';
import { configureAuthRuntime } from '@/shared/lib/auth/runtime';
import '@/shared/types/modals';
import { queryClient } from '@/shared/lib/queryClient';
import { isTauriApp } from '@/shared/lib/platform';
import { initZoom, zoomIn, zoomOut, zoomReset } from '@/shared/lib/zoom';

const DEFAULT_TRACES_SAMPLE_RATE = 0.1;

/**
 * Out-of-range or unparseable values fall back to the default rather than
 * throwing: this runs at module load, and a malformed env var must not stop the
 * app from booting.
 */
function parseSampleRate(raw: string | undefined): number {
  const parsed = Number(raw);
  return raw !== undefined && Number.isFinite(parsed) && parsed >= 0 && parsed <= 1
    ? parsed
    : DEFAULT_TRACES_SAMPLE_RATE;
}

if (import.meta.env.VITE_SENTRY_DSN) {
  Sentry.init({
    dsn: import.meta.env.VITE_SENTRY_DSN,
    release: __SENTRY_RELEASE__,
    // Kept in step with the Rust side's SENTRY_TRACES_SAMPLE_RATE. This was 1.0,
    // which billed every page load and every router navigation as a transaction.
    tracesSampleRate: parseSampleRate(
      import.meta.env.VITE_SENTRY_TRACES_SAMPLE_RATE
    ),
    environment: import.meta.env.MODE === 'development' ? 'dev' : 'production',
    integrations: [Sentry.tanstackRouterBrowserTracingIntegration(router)],
  });
  Sentry.setTag('source', 'frontend');
}

const posthogKey =
  import.meta.env.VITE_POSTHOG_KEY ?? import.meta.env.VITE_POSTHOG_API_KEY;
const posthogHost =
  import.meta.env.VITE_POSTHOG_HOST ??
  import.meta.env.VITE_POSTHOG_API_ENDPOINT;

if (posthogKey && posthogHost) {
  posthog.init(posthogKey, {
    api_host: posthogHost,
    capture_pageview: false,
    capture_pageleave: true,
    capture_performance: true,
    autocapture: false,
    opt_out_capturing_by_default: true,
  });
} else {
  console.warn(
    'PostHog API key or endpoint not set. Analytics will be disabled.'
  );
}

// In the Tauri desktop app, implement custom zoom (Cmd/Ctrl + =/-/0) via root
// font-size scaling and block trackpad/touchpad pinch-to-zoom.
if (isTauriApp()) {
  initZoom();

  document.addEventListener('keydown', (e) => {
    const mod = e.metaKey || e.ctrlKey;
    if (!mod) return;

    if (e.key === '=' || e.key === '+') {
      e.preventDefault();
      zoomIn();
    } else if (e.key === '-') {
      e.preventDefault();
      zoomOut();
    } else if (e.key === '0') {
      e.preventDefault();
      zoomReset();
    }
  });

  document.addEventListener(
    'wheel',
    (e) => {
      if (e.ctrlKey) e.preventDefault();
    },
    { passive: false }
  );
  document.addEventListener('gesturestart', (e) => e.preventDefault());
  document.addEventListener('gesturechange', (e) => e.preventDefault());
}

configureAuthRuntime({
  getToken: () => tokenManager.getToken(),
  triggerRefresh: () => tokenManager.triggerRefresh(),
  registerShape: (shape) => tokenManager.registerShape(shape),
  getCurrentUser: () => oauthApi.getCurrentUser(),
});

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <PostHogProvider client={posthog}>
        <Sentry.ErrorBoundary
          fallback={({ error, componentStack }) => (
            <CrashScreen
              error={error instanceof Error ? error : undefined}
              componentStack={componentStack}
            />
          )}
          showDialog
        >
          <ClickToComponent />
          <App />
        </Sentry.ErrorBoundary>
      </PostHogProvider>
    </QueryClientProvider>
  </React.StrictMode>
);
