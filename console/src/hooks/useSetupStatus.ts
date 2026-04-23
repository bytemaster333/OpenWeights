import { useQuery } from "@tanstack/react-query"

import { casFetch } from "@/lib/api"

/**
 * `/setup` first-run status tiles (CONSOLE-11, CONSOLE-12, OPS-01).
 *
 * Data source: CAS `GET /admin/setup/status`, landed in 04-01 (see
 * `cas/crates/siahub-cas-core/src/handlers/admin/setup.rs`). The endpoint
 * aggregates subsystem health checks:
 *
 * - Postgres: `SELECT 1` round-trip latency.
 * - Redis: `PING` round-trip latency.
 * - indexd: pings the configured `INDEXD_URL`; reports `synced` from the
 *   consensus payload when reachable (gotcha #6).
 * - GitHub OAuth: flags whether `GITHUB_OAUTH_CLIENT_ID|SECRET|CALLBACK_URL`
 *   are all present in the CAS env — drives the P14 "not configured" card
 *   on `/setup`.
 * - V2 reconstruction: the operator-set `SIAHUB_V2_RECONSTRUCTION_ENABLED`
 *   flag (gotcha #3). Read-only informational (Ambiguity 3).
 *
 * Refetch every 30s so an operator fixing `.env` sees the tiles flip within
 * half a minute of `docker compose restart cas`. Background refetch is off
 * — if the admin's browser tab is hidden, we don't need to spam CAS.
 */

export type SubsystemStatus = {
  status: "ok" | "degraded" | "error"
  latency_ms?: number
}

export type SetupStatus = {
  postgres: SubsystemStatus
  redis: SubsystemStatus
  indexd: SubsystemStatus & {
    /** `true` when indexd's consensus is synced and wallet-funded. */
    synced?: boolean
    /** Operator-configured URL (CONSOLE-12 — surfaced read-only). */
    url: string
  }
  github_oauth: { configured: boolean }
  v2_reconstruction_enabled: boolean
}

export function useSetupStatus() {
  return useQuery<SetupStatus>({
    queryKey: ["setup-status"],
    queryFn: () => casFetch<SetupStatus>("/admin/setup/status"),
    refetchInterval: 30_000,
    refetchIntervalInBackground: false,
  })
}
