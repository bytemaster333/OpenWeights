import { useQuery } from "@tanstack/react-query"

import { casFetch } from "@/lib/api"

/**
 * `/setup` first-run status tiles.
 *
 * Data source: CAS `GET /admin/setup/status`, landed in (see
 * `cas/crates/siahub-cas-core/src/handlers/admin/setup.rs`). The endpoint
 * aggregates subsystem health checks:
 *
 * - Postgres: `SELECT 1` round-trip latency.
 * - Redis: `PING` round-trip latency.
 * - indexd: pings the configured `INDEXD_URL`; reports `synced` from the
 * consensus payload when reachable.
 * - GitHub OAuth: flags whether `GITHUB_OAUTH_CLIENT_ID|SECRET|CALLBACK_URL`
 * are all present in the CAS env — drives the "not configured" card
 * on `/setup`.
 * - V2 reconstruction: the operator-set `SIAHUB_V2_RECONSTRUCTION_ENABLED`
 * flag. Read-only informational.
 *
 * Refetch every 30s so an operator fixing `.env` sees the tiles flip within
 * half a minute of `docker compose restart cas`. Background refetch is off
 * — if the admin's browser tab is hidden, we don't need to spam CAS.*/

export type SubsystemStatus = {
  status: "ok" | "degraded" | "error"
  latency_ms?: number
}

export type SetupStatus = {
  postgres: SubsystemStatus
  redis: SubsystemStatus
  indexd: SubsystemStatus & {
    /** `true` when indexd's consensus is synced and wallet-funded.*/
    synced?: boolean
    /** Operator-configured URL ( — surfaced read-only).*/
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

/**
 * Public Sia subsystem snapshot — wallet address, formed contracts,
 * distinct host count, plus the siascan.com base for explorer deep-links.
 * Powers the "On Sia" panel that proves the deployment is actually
 * pinning bytes on the Sia network (vs. the local Postgres durable cache).
 */
export type ContractSummary = {
  id: string
  host_key: string
  formation: string | null
  size: number
  remaining_allowance: string
}

export type PlatformSia = {
  wallet_address: string | null
  wallet_spendable_hastings: string | null
  wallet_immature_hastings: string | null
  contract_count: number
  distinct_host_count: number
  contracts: ContractSummary[]
  siascan_base: string
  indexd_synced: boolean | null
}

export function usePlatformSia() {
  return useQuery<PlatformSia>({
    queryKey: ["platform-sia"],
    queryFn: () => casFetch<PlatformSia>("/api/platform/sia"),
    refetchInterval: 30_000,
    refetchIntervalInBackground: false,
  })
}
