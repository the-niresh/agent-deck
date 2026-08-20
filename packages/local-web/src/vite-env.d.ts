/// <reference types="vite/client" />

interface ImportMetaEnv {
  readonly VITE_POSTHOG_KEY?: string;
  readonly VITE_POSTHOG_HOST?: string;
  readonly VITE_POSTHOG_API_KEY?: string;
  readonly VITE_POSTHOG_API_ENDPOINT?: string;
}

declare const __APP_VERSION__: string;
declare const __SENTRY_RELEASE__: string;
