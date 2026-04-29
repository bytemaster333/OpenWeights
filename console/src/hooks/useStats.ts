import { useQuery } from "@tanstack/react-query"

import { type ApiError, casFetch } from "@/lib/api"

/**
 * CONSOLE-07 — live stats. `GET /admin/stats` shape mirrors
 * `cas/crates/siahub-cas-core/src/handlers/admin/stats.rs` (landed in 04-01).
 *
 * Load-bearing contract notes:
 * - `cache_hit_rate` is `[0, 1]` (CAS casts the `AVG(...)` to `double
 * precision`; NULL rows are excluded). The UI multiplies by 100 for
 * display.
 * - `total_bytes_stored` / `total_bytes_served` / `provider_count` are all
 * `i64` serialized as JSON number; at consumer scale this stays well
 * within the IEEE-754 safe-int range (< 2^53) for v1.
 * - `per_key[i].key_id` is a UUID string (CAS `api_keys.id`). `key_id` is
 * NOT joined with the user-facing key label here; the `/keys` page
 * fetches `/admin/keys` separately and can cross-reference by id.
 * - `recent_activity[i].ts` is an ISO-8601 string (chrono `DateTime<Utc>`
 * defaults to RFC 3339). The UI calls `toLocaleTimeString` for display.
 * - `recent_activity[i].hash` is the lowercase hex of `usage_log.xorb_hash`
 * (display-only — NOT the xet-core byte-reversed merkle encoding, which
 * is irrelevant for raw BYTEA column reads per CAS' handler comment).
 */

export type PerKeyStats = {
  key_id: string
  bytes_stored: number
  bytes_served: number
}

export type ActivityRow = {
  ts: string
  hash: string | null
  event: string
  bytes: number | null
  cache_hit: boolean | null
}

export type StatsResponse = {
  total_bytes_stored: number
  total_bytes_served: number
  /** COUNT of `event='download'` rows attributed to the caller's content. */
  total_downloads: number
  /** `[0, 1]` — UI multiplies by 100 for percent display. */
  /** `[0, 1]` over downloads with non-null `cache_hit`. `null` when there
   * are zero eligible rows — UI should render "—" not "0.0%". */
  cache_hit_rate: number | null
  provider_count: number
  per_key: PerKeyStats[]
  recent_activity: ActivityRow[]
}

/**
 * TanStack Query hook for the live stats panel (CONSOLE-07).
 *
 * D-47 lock: `refetchInterval: 10_000` with `refetchIntervalInBackground: false`.
 * We do NOT want to spin the gateway/CAS when the tab is backgrounded — the
 * user cannot see the dashboard, so the refresh is wasted. TanStack pauses
 * the interval automatically when `document.visibilityState !== "visible"`.
 *
 * `retry: false` keeps the loop tight on transient failures; the next 10s
 * tick is the "retry". `staleTime: 0` ensures the refetch actually fires
 * rather than being short-circuited by the default staleTime from
 * `queryClient.ts` (which would suppress the background tick).
 */
export const STATS_REFETCH_INTERVAL_MS = 10_000

export type PlatformStats = {
  total_models: number
  total_users: number
  total_files: number
  total_size_bytes: number
  total_downloads: number
  total_bytes_served: number
  downloads_today: number
  bytes_served_today: number
  /** Xorbs that have a real `sia_object_id` and are `pin_state='pinned'` —
   * the bytes that ACTUALLY landed on Sia hosts (vs durable Postgres cache).*/
  xorbs_pinned: number
  /** Total xorb count regardless of pin state. */
  xorbs_total: number
  /** Sum of size_bytes for pinned xorbs — "bytes really on Sia". */
  bytes_on_sia: number
}

/**
 * Public platform-wide aggregate counters. Used on `/dashboard` and
 * `/stats` to surface the deployment's collective footprint alongside the
 * per-user numbers from `/admin/stats`. Anonymous-readable so the tiles
 * mount before session resolution (parallels HF's home page counters).
 */
export function usePlatformStats() {
  return useQuery<PlatformStats, ApiError>({
    queryKey: ["platform-stats"],
    queryFn: () => casFetch<PlatformStats>("/api/platform/stats"),
    refetchInterval: STATS_REFETCH_INTERVAL_MS,
    refetchIntervalInBackground: false,
    retry: false,
    staleTime: 0,
  })
}

export function useStats() {
  return useQuery<StatsResponse, ApiError>({
    queryKey: ["stats"],
    queryFn: () => casFetch<StatsResponse>("/admin/stats"),
    refetchInterval: STATS_REFETCH_INTERVAL_MS,
    refetchIntervalInBackground: false,
    retry: false,
    staleTime: 0,
  })
}

/**
 * CONSOLE-08 — benchmarks panel. Sourced from `public/benchmarks.json`
 * (static file), NOT from a CAS endpoint.
 *
 * Rationale (decision captured in 04-07-SUMMARY.md):
 * - Plan 04-07 Task 2 flags `/admin/stats/benchmarks` as out-of-scope for
 * CAS 04-01 (it shipped 11 handlers; benchmarks not among them).
 * - Phase 5 CI is the authoritative producer of benchmark numbers; they
 * live in `docs/benchmarks.md` + a machine-readable JSON next to it.
 * - Option (b) from the plan: the console reads the static JSON, which is
 * overwritten by Phase 5's `bench` job the same way 04-02 set up
 * `conformance-badge.json`. Zero real-time dependency on CAS for a
 * value that changes at most monthly.
 *
 * Query key stays under TanStack so that page unmount cancels the fetch
 * and the 10s refetch keeps the pages feeling live together.
 */
export type BenchmarkRow = {
  scenario: string
  siahub_mbps: number | null
  hf_baseline_mbps: number | null
}

export type BenchmarksResponse = {
  /** ISO-8601 timestamp of the Phase 5 bench run that produced the row set,
 * or `null` if the file is still the v1 placeholder. */
  generated_at: string | null
  rows: BenchmarkRow[]
}

async function fetchBenchmarks(): Promise<BenchmarksResponse> {
  // Same-origin static fetch — console's nginx (Dockerfile stage 2) serves
  // this under `/benchmarks.json`. `credentials` does not matter here.
  const res = await fetch("/benchmarks.json", { headers: { Accept: "application/json" } })
  if (!res.ok) {
    throw new Error(`benchmarks.json ${res.status}`)
  }
  return (await res.json()) as BenchmarksResponse
}

export function useBenchmarks() {
  return useQuery<BenchmarksResponse, Error>({
    queryKey: ["benchmarks"],
    queryFn: fetchBenchmarks,
    // Benchmarks rarely change — keep them on a lazy refetch cadence.
    refetchInterval: false,
    staleTime: 5 * 60_000,
    retry: false,
  })
}
