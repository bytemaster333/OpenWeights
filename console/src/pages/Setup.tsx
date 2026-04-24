import {
  CheckCircle,
  HeartbeatIcon,
  Warning,
  WarningCircle,
} from "@phosphor-icons/react"

import { Badge } from "@/components/ui/badge"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Skeleton } from "@/components/ui/skeleton"
import { type SubsystemStatus, useSetupStatus } from "@/hooks/useSetupStatus"
import { CAS_URL } from "@/lib/api"

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
