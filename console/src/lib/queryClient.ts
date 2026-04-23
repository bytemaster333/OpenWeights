import { QueryClient } from "@tanstack/react-query"

/**
 * Shared TanStack Query client (D-39).
 * - `refetchOnWindowFocus: false` — console is not latency-sensitive; avoid spam.
 * - `staleTime: 5_000` — cheap de-dup for hooks mounted in quick succession.
 * - `retry: 1` — one retry is enough; the gateway/CAS don't benefit from churn.
 *
 * Per-query `refetchInterval` overrides (e.g. `/stats` at 10s — D-47) live on
 * the hooks themselves, not here.
 */
export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      refetchOnWindowFocus: false,
      retry: 1,
      staleTime: 5_000,
    },
  },
})
