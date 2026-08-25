import { ChartLineIcon, GlobeIcon } from "@phosphor-icons/react"
import { Link } from "@tanstack/react-router"
import { useMemo, useState } from "react"

import { StatsTile } from "@/components/StatsTile"
import { Button } from "@/components/ui/button"
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table"
import { useKeys } from "@/hooks/useKeys"
import { usePlatformStats, useStats } from "@/hooks/useStats"
import { eventHasCacheSemantics, formatEvent } from "@/lib/eventLabels"
import { formatBytes, formatRelative } from "@/lib/format"

/**
 * `/stats` — live usage counters + per-key + recent activity.
 *
 * The `useStats` hook refetches every 10 s (TanStack pauses it when the
 * tab is hidden). Per-key rows cross-reference the `/admin/keys` list for
 * human labels — revoked keys fall back to a short id prefix.
 *
 * Zero-usage keys are hidden by default; the "Show idle keys" toggle
 * reveals them for the rare case where an operator wants to audit all keys
 * regardless of activity.*/

export function StatsPage() {
  const { data, isPending } = useStats()
  const { data: platform, isPending: platformPending } = usePlatformStats()
  const { data: keys } = useKeys()
  const [showIdle, setShowIdle] = useState(false)

  const keyLabel = useMemo(() => {
    const m = new Map<string, string>()
    for (const k of keys ?? []) m.set(k.id, k.name)
    return (id: string) => m.get(id) ?? `${id.slice(0, 8)}…`
  }, [keys])

  const perKey = data?.per_key ?? []
  const visibleKeys = showIdle
    ? perKey
    : perKey.filter((r) => r.bytes_stored > 0 || r.bytes_served > 0)
  const idleCount = perKey.length - visibleKeys.length

  return (
    <main className="mx-auto max-w-6xl space-y-6 px-6 py-8">
      <header>
        <div className="flex items-center gap-3">
          <ChartLineIcon size={22} weight="light" className="text-muted-foreground" />
          <h1 className="font-heading text-2xl font-semibold tracking-tight">Stats</h1>
        </div>
        <p className="mt-1 text-sm text-muted-foreground">
          Live usage counters. Refreshes every 10s.
        </p>
      </header>

      {/* Platform-wide totals*/}
      <section>
        <h2 className="mb-3 flex items-center gap-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          <GlobeIcon size={14} weight="light" /> Platform totals
        </h2>
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-4">
          <StatsTile
            label="Models"
            loading={platformPending}
            value={platform ? String(platform.total_models) : "—"}
            subtext={
              platform
                ? `${platform.total_users} contributor${platform.total_users === 1 ? "" : "s"}`
                : ""
            }
          />
          <StatsTile
            label="Stored"
            loading={platformPending}
            value={platform ? formatBytes(platform.total_size_bytes) : "—"}
            subtext={
              platform ? `${platform.total_files} file${platform.total_files === 1 ? "" : "s"}` : ""
            }
          />
          <StatsTile
            label="Downloads"
            loading={platformPending}
            value={platform ? String(platform.total_downloads) : "—"}
            subtext={platform ? `+${platform.downloads_today} today` : ""}
          />
          <StatsTile
            label="Bytes served"
            loading={platformPending}
            value={platform ? formatBytes(platform.total_bytes_served) : "—"}
            subtext={platform ? `+${formatBytes(platform.bytes_served_today)} today` : ""}
          />
        </div>
      </section>

      {/* Your usage*/}
      <section>
        <h2 className="mb-3 flex items-center gap-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          <ChartLineIcon size={14} weight="light" /> Your usage
        </h2>
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-4">
          <StatsTile
            label="Bytes uploaded"
            loading={isPending}
            value={data ? formatBytes(data.total_bytes_stored) : "—"}
            subtext="pending + pinned"
          />
          <StatsTile
            label="Downloads"
            loading={isPending}
            value={data ? String(data.total_downloads) : "—"}
            subtext={data ? formatBytes(data.total_bytes_served) : "bytes"}
          />
          <StatsTile
            label="Cache hit rate"
            loading={isPending}
            value={
              data && data.cache_hit_rate !== null
                ? `${(data.cache_hit_rate * 100).toFixed(1)}%`
                : "—"
            }
            subtext={data && data.cache_hit_rate === null ? "no downloads yet" : "of downloads"}
          />
          <StatsTile
            label="Keys with usage"
            loading={isPending}
            value={data ? String(data.provider_count) : "—"}
            subtext="distinct downloaders"
          />
        </div>
      </section>

      {/* Per-key usage*/}
      <section>
        <header className="mb-3 flex items-baseline justify-between gap-3">
          <h2 className="font-heading text-sm font-semibold">Per-key usage</h2>
          {idleCount > 0 && (
            <Button variant="ghost" size="sm" onClick={() => setShowIdle((v) => !v)}>
              {showIdle
                ? `Hide ${idleCount} idle key${idleCount === 1 ? "" : "s"}`
                : `Show ${idleCount} idle key${idleCount === 1 ? "" : "s"}`}
            </Button>
          )}
        </header>
        <div className="rounded border">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Key</TableHead>
                <TableHead className="text-right">Bytes uploaded</TableHead>
                <TableHead className="text-right">Bytes served</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {visibleKeys.length === 0 && (
                <TableRow>
                  <TableCell
                    colSpan={3}
                    className="py-10 text-center text-xs text-muted-foreground"
                  >
                    No activity yet.
                  </TableCell>
                </TableRow>
              )}
              {visibleKeys.map((r) => (
                <TableRow key={r.key_id}>
                  <TableCell className="font-medium">{keyLabel(r.key_id)}</TableCell>
                  <TableCell className="text-right tabular-nums">
                    {formatBytes(r.bytes_stored)}
                  </TableCell>
                  <TableCell className="text-right tabular-nums">
                    {formatBytes(r.bytes_served)}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
      </section>

      {/* Recent activity*/}
      <section>
        <h2 className="mb-3 font-heading text-sm font-semibold">Recent activity</h2>
        <div className="rounded border">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Time</TableHead>
                <TableHead>Event</TableHead>
                <TableHead>Xorb</TableHead>
                <TableHead className="text-right">Bytes</TableHead>
                <TableHead className="text-right">Cache</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {data?.recent_activity.length === 0 && (
                <TableRow>
                  <TableCell
                    colSpan={5}
                    className="py-10 text-center text-xs text-muted-foreground"
                  >
                    No activity yet.
                  </TableCell>
                </TableRow>
              )}
              {data?.recent_activity.map((a, i) => (
                <TableRow key={`${a.ts}-${i}`}>
                  <TableCell
                    className="text-xs text-muted-foreground"
                    title={new Date(a.ts).toLocaleString()}
                  >
                    {formatRelative(a.ts)}
                  </TableCell>
                  <TableCell className="text-sm">{formatEvent(a.event)}</TableCell>
                  <TableCell className="font-mono text-xs">
                    {a.hash ? (
                      <Link
                        to="/assets/$hash"
                        params={{ hash: a.hash }}
                        className="text-muted-foreground hover:text-foreground hover:underline"
                        title={a.hash}
                      >
                        {a.hash.slice(0, 12)}…
                      </Link>
                    ) : (
                      <span className="text-muted-foreground">—</span>
                    )}
                  </TableCell>
                  <TableCell className="text-right tabular-nums">
                    {a.bytes !== null ? (
                      formatBytes(a.bytes)
                    ) : (
                      <span className="text-muted-foreground">—</span>
                    )}
                  </TableCell>
                  <TableCell className="text-right text-xs">
                    {eventHasCacheSemantics(a.event) && a.cache_hit !== null ? (
                      <span className={a.cache_hit ? "text-primary" : "text-muted-foreground"}>
                        {a.cache_hit ? "✓ hit" : "✗ miss"}
                      </span>
                    ) : (
                      <span className="text-muted-foreground/50">—</span>
                    )}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
      </section>
    </main>
  )
}
