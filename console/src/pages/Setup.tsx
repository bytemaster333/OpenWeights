import {
  ArrowSquareOutIcon,
  CheckCircle,
  CloudCheckIcon,
  HeartbeatIcon,
  Warning,
  WarningCircle,
} from "@phosphor-icons/react"

import { Badge } from "@/components/ui/badge"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Skeleton } from "@/components/ui/skeleton"
import {
  type SubsystemStatus,
  usePlatformSia,
  useSetupStatus,
} from "@/hooks/useSetupStatus"
import { CAS_URL } from "@/lib/api"
import { formatBytes, formatHastingsToSC } from "@/lib/format"

/**
 * `/setup` — platform status.
 *
 * Surfaces the subsystems a regular user cares about: metadata database,
 * cache, Sia indexer. The V2 reconstruction tile is omitted here because
 * its phrasing ("multi-range reconstruction") is meaningless out of context
 * and reads as a WIP flag; operators who need the value run it via
 * `GET /admin/setup/status` directly.*/

function StatusIcon({ s }: { s: SubsystemStatus["status"] }) {
  if (s === "ok") return <CheckCircle weight="fill" className="size-4 text-primary" />
  if (s === "degraded") return <Warning weight="fill" className="size-4 text-foreground/70" />
  return <WarningCircle weight="fill" className="size-4 text-destructive" />
}

function StatusBadge({ s }: { s: SubsystemStatus }) {
  const variant =
    s.status === "ok" ? "default" : s.status === "degraded" ? "secondary" : "destructive"
  return (
    <Badge variant={variant} data-testid="setup-status-badge" data-status={s.status}>
      <StatusIcon s={s.status} />
      {s.status}
    </Badge>
  )
}

function latencyText(ms?: number): string {
  return ms === undefined ? "—" : `${ms.toFixed(1)} ms`
}

export function SetupPage() {
  const { data, isPending, error, refetch } = useSetupStatus()
  const { data: sia } = usePlatformSia()

  return (
    <main className="mx-auto max-w-5xl space-y-6 px-6 py-8" data-testid="setup-page">
      <header>
        <div className="flex items-center gap-3">
          <HeartbeatIcon
            size={22}
            weight="light"
            className="text-muted-foreground"
          />
          <h1 className="font-heading text-2xl font-semibold tracking-tight">
            Platform status
          </h1>
        </div>
        <p className="mt-1 text-sm text-muted-foreground">
          Live health of SiaHub subsystems.
        </p>
      </header>

      {isPending && (
        <div className="grid gap-4 sm:grid-cols-2" data-testid="setup-loading">
          <Skeleton className="h-28 w-full" />
          <Skeleton className="h-28 w-full" />
          <Skeleton className="col-span-full h-36 w-full" />
        </div>
      )}

      {!isPending && (error || !data) && (
        <div
          className="rounded border border-destructive/40 bg-destructive/10 p-4 text-sm text-destructive"
          data-testid="setup-error"
        >
          Could not reach SiaHub right now. The control plane is unavailable.
          <button
            type="button"
            className="ml-2 underline"
            onClick={() => void refetch()}
            data-testid="setup-retry"
          >
            Retry
          </button>
        </div>
      )}

      {!isPending && data && (
        <>
          <div className="grid gap-4 sm:grid-cols-2">
            <Card data-testid="setup-tile-postgres">
              <CardHeader className="flex flex-row items-center justify-between pb-2">
                <CardTitle>Metadata database</CardTitle>
                <StatusBadge s={data.postgres} />
              </CardHeader>
              <CardContent className="text-xs text-muted-foreground">
                Round-trip latency:{" "}
                <span className="font-mono">{latencyText(data.postgres.latency_ms)}</span>
              </CardContent>
            </Card>

            <Card data-testid="setup-tile-redis">
              <CardHeader className="flex flex-row items-center justify-between pb-2">
                <CardTitle>Cache</CardTitle>
                <StatusBadge s={data.redis} />
              </CardHeader>
              <CardContent className="text-xs text-muted-foreground">
                Round-trip latency:{" "}
                <span className="font-mono">{latencyText(data.redis.latency_ms)}</span>
              </CardContent>
            </Card>

            <Card data-testid="setup-tile-indexd" className="sm:col-span-2">
              <CardHeader className="flex flex-row items-center justify-between pb-2">
                <CardTitle>Sia indexer</CardTitle>
                <StatusBadge s={data.indexd} />
              </CardHeader>
              <CardContent className="space-y-1 text-xs text-muted-foreground">
                <div>
                  Chain synced:{" "}
                  <span
                    className="font-mono"
                    data-testid="setup-indexd-synced"
                    data-synced={data.indexd.synced ? "true" : "false"}
                  >
                    {data.indexd.synced ? "yes" : "no"}
                  </span>
                </div>
                <div>
                  Round-trip latency:{" "}
                  <span className="font-mono">{latencyText(data.indexd.latency_ms)}</span>
                </div>
                <p className="pt-1 text-[0.7rem]">
                  Self-hosted <code className="font-mono">indexd</code> node
                  tracking the Sia chain.
                </p>
              </CardContent>
            </Card>
          </div>

          {/* On-Sia network panel — proof we're actually pinning bytes
              on the Sia network. Every link below opens siascan.com. */}
          {sia && (
            <section
              className="rounded border bg-muted/10 p-4"
              data-testid="setup-tile-sia"
            >
              <div className="mb-3 flex items-center justify-between">
                <h2 className="flex items-center gap-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                  <CloudCheckIcon size={14} weight="light" /> On Sia network
                </h2>
                {sia.indexd_synced != null && (
                  <Badge
                    variant={sia.indexd_synced ? "default" : "secondary"}
                  >
                    {sia.indexd_synced ? "synced" : "syncing"}
                  </Badge>
                )}
              </div>

              {/* KPIs */}
              <div className="grid grid-cols-2 gap-4 sm:grid-cols-4">
                <div>
                  <div className="text-[0.7rem] uppercase text-muted-foreground">
                    Renter wallet
                  </div>
                  <div className="font-mono text-xs">
                    {formatHastingsToSC(sia.wallet_spendable_hastings)}
                  </div>
                </div>
                <div>
                  <div className="text-[0.7rem] uppercase text-muted-foreground">
                    Active contracts
                  </div>
                  <div className="font-mono text-sm">{sia.contract_count}</div>
                </div>
                <div>
                  <div className="text-[0.7rem] uppercase text-muted-foreground">
                    Distinct hosts
                  </div>
                  <div className="font-mono text-sm">
                    {sia.distinct_host_count}
                  </div>
                </div>
                <div>
                  <div className="text-[0.7rem] uppercase text-muted-foreground">
                    Network
                  </div>
                  <div className="font-mono text-sm">Zen testnet</div>
                </div>
              </div>

              {/* Wallet → siascan */}
              {sia.wallet_address && (
                <div className="mt-4 space-y-1">
                  <div className="text-[0.7rem] uppercase text-muted-foreground">
                    Renter address
                  </div>
                  <div className="flex flex-wrap items-center gap-2">
                    <code className="break-all font-mono text-xs">
                      {sia.wallet_address}
                    </code>
                    <a
                      href={`${sia.siascan_base}/address/${sia.wallet_address}`}
                      target="_blank"
                      rel="noreferrer"
                      className="inline-flex items-center gap-1 text-xs text-primary underline underline-offset-2 hover:no-underline"
                    >
                      View on siascan
                      <ArrowSquareOutIcon size={12} weight="light" />
                    </a>
                  </div>
                </div>
              )}

              {/* Contracts list */}
              {sia.contracts.length > 0 && (
                <div className="mt-4">
                  <div className="mb-2 text-[0.7rem] uppercase text-muted-foreground">
                    Contracts
                  </div>
                  <div className="overflow-x-auto rounded border">
                    <table className="w-full text-xs">
                      <thead className="bg-muted/40">
                        <tr>
                          <th className="px-2 py-1 text-left font-medium">
                            Contract
                          </th>
                          <th className="px-2 py-1 text-left font-medium">
                            Host
                          </th>
                          <th className="px-2 py-1 text-right font-medium">
                            Stored
                          </th>
                        </tr>
                      </thead>
                      <tbody>
                        {sia.contracts.map((c) => (
                          <tr
                            key={c.id}
                            className="border-t border-border/60"
                          >
                            <td className="px-2 py-1 font-mono">
                              <a
                                href={`${sia.siascan_base}/contract/${c.id}`}
                                target="_blank"
                                rel="noreferrer"
                                className="text-primary underline underline-offset-2 hover:no-underline"
                                title={c.id}
                              >
                                {c.id.slice(0, 12)}…
                              </a>
                            </td>
                            <td className="px-2 py-1 font-mono">
                              <a
                                href={`${sia.siascan_base}/host/${c.host_key.replace(/^ed25519:/, "")}`}
                                target="_blank"
                                rel="noreferrer"
                                className="text-muted-foreground hover:text-foreground hover:underline"
                                title={c.host_key}
                              >
                                {c.host_key.replace(/^ed25519:/, "").slice(0, 10)}…
                              </a>
                            </td>
                            <td className="px-2 py-1 text-right tabular-nums">
                              {formatBytes(c.size)}
                            </td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                </div>
              )}
            </section>
          )}

          <section className="rounded border bg-muted/10 p-4">
            <h2 className="mb-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
              Deployment
            </h2>
            <dl className="grid gap-x-6 gap-y-1 text-sm sm:grid-cols-[max-content_1fr]">
              <dt className="text-muted-foreground">CAS endpoint</dt>
              <dd className="font-mono text-xs">{CAS_URL}</dd>
            </dl>
          </section>
        </>
      )}
    </main>
  )
}
