import { useQuery } from "@tanstack/react-query"

/**
 * Conformance badge (CONSOLE-13).
 *
 * Fetches the static artifact that Phase 5's conformance CI job writes into
 * `console/public/conformance-badge.json` on every main-branch commit. The
 * file ships in the Docker image alongside the SPA bundle, so the browser
 * pulls it from the same origin — no CAS round-trip, no cookie.
 *
 * Shape (matches `.planning/phases/05-e2e-gates` deliverable):
 * ```json
 * {
 *   "status": "pass" | "fail" | "unknown",
 *   "last_run": "2026-04-21T12:34:56Z" | null,
 *   "commit": "abc1234" | null,
 *   "note": "populated by Phase 5 conformance CI artifact"
 * }
 * ```
 *
 * Poll cadence is 5 minutes; the artifact only changes per merge-to-main, so
 * anything tighter would burn CPU for zero signal. On fetch failure (404,
 * network), we return the `unknown` sentinel — the badge renders as grey
 * rather than "FAIL", so a missing artifact does not flag the deployment as
 * broken.
 *
 * Credentials are explicitly omitted: the badge file is public by design
 * (grant reviewers who open the app in an incognito tab still see it).
 */

const BADGE_URL = import.meta.env.VITE_CONFORMANCE_BADGE_URL ?? "/conformance-badge.json"

/** How often the header badge polls `/conformance-badge.json`. */
export const CONFORMANCE_POLL_INTERVAL_MS = 5 * 60_000

/** Stale-after threshold — the badge falls back to grey if no data yet. */
export const CONFORMANCE_STALE_AFTER_MS = 24 * 60 * 60_000

export type ConformanceStatus = "pass" | "fail" | "unknown"

export type ConformanceBadgeData = {
  status: ConformanceStatus
  last_run: string | null
  commit: string | null
  note?: string
}

/** Static fallback when the fetch fails; equivalent to "no signal yet". */
const UNKNOWN: ConformanceBadgeData = {
  status: "unknown",
  last_run: null,
  commit: null,
}

export function useConformance() {
  return useQuery<ConformanceBadgeData>({
    queryKey: ["conformance-badge"],
    queryFn: async () => {
      const res = await fetch(BADGE_URL, { credentials: "omit" })
      if (!res.ok) return UNKNOWN
      try {
        return (await res.json()) as ConformanceBadgeData
      } catch {
        return UNKNOWN
      }
    },
    refetchInterval: CONFORMANCE_POLL_INTERVAL_MS,
    refetchIntervalInBackground: false,
    staleTime: CONFORMANCE_POLL_INTERVAL_MS,
    retry: false,
  })
}

/**
 * Derives the effective UI state — treats a PASS/FAIL older than the stale
 * threshold as "unknown" so a long-stopped CI doesn't broadcast green forever.
 */
export function effectiveStatus(
  data: ConformanceBadgeData | undefined,
  now: number = Date.now(),
): ConformanceStatus {
  if (!data) return "unknown"
  if (data.status === "unknown") return "unknown"
  if (!data.last_run) return data.status
  const ts = Date.parse(data.last_run)
  if (Number.isNaN(ts)) return data.status
  if (now - ts > CONFORMANCE_STALE_AFTER_MS) return "unknown"
  return data.status
}
