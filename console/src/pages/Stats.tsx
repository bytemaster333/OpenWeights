import { Link } from "@tanstack/react-router"
import { useMemo } from "react"

import { StatsTile } from "@/components/StatsTile"
import { UserMenu } from "@/components/UserMenu"
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
import { useMe } from "@/hooks/useMe"
import { useStats } from "@/hooks/useStats"

/**
 * `/stats` (CONSOLE-07).
 *
 * Four KPI tiles + per-key usage table + recent-activity table. The
 * `useStats()` hook refetches every 10s (D-47); TanStack pauses the loop
 * when the tab is backgrounded.
 *
 * `per_key` rows from CAS only carry `key_id` (UUID). To surface a
 * human-readable label we cross-reference against the `/admin/keys` list
 * (`useKeys()`), matching by id. Keys that have been revoked but still
 * have historical usage rows will render their id prefix as a fallback —
 * intentional: the user can still see "key X served Y bytes" even after
 * they've deleted it.
 */

function bytes(n: number): string {
  const units = ["B", "KB", "MB", "GB", "TB", "PB"]
  let v = n
  let i = 0
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024
    i++
  }
  return `${v.toFixed(v < 10 && i > 0 ? 2 : 0)} ${units[i]}`
}

export function StatsPage() {
  const { data: user } = useMe()
  const { data, isPending } = useStats()
  const { data: keys } = useKeys()

  // Map key_id → human label so `per_key` rows render something readable.
  // Revoked keys fall back to a truncated id ("b3f02..." style) so
  // historical usage rows are still attributable.
  const keyLabel = useMemo(() => {
    const m = new Map<string, string>()
    for (const k of keys ?? []) {
      m.set(k.id, k.name)
    }
    return (id: string) => m.get(id) ?? `${id.slice(0, 8)}…`
  }, [keys])

  return (
    <main className="mx-auto max-w-6xl space-y-8 px-6 py-10">
      <header className="flex items-center justify-between">
        <div>
          <h1 className="font-heading text-2xl font-medium tracking-tight">Stats</h1>
          <p className="mt-1 text-xs text-muted-foreground">
            Live usage counters. Refreshes every 10 seconds.
          </p>
        </div>
        {user && <UserMenu user={user} />}
      </header>

      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-4">
        <StatsTile
          label="Stored on Sia"
          loading={isPending}
          value={data ? bytes(data.total_bytes_stored) : "—"}
        />
        <StatsTile
          label="Bytes served"
          loading={isPending}
          value={data ? bytes(data.total_bytes_served) : "—"}
        />
        <StatsTile
          label="Cache hit rate"
          loading={isPending}
          value={data ? `${(data.cache_hit_rate * 100).toFixed(1)}%` : "—"}
          subtext="from gateway download events"
        />
        <StatsTile
          label="API keys with usage"
          loading={isPending}
          value={data ? String(data.provider_count) : "—"}
          subtext="distinct keys emitting events"
        />
      </div>

      <section>
        <header className="mb-3 flex items-center justify-between">
          <h2 className="font-heading text-base font-medium">Per-key usage</h2>
        </header>
        <div className="rounded-none bg-card ring-1 ring-foreground/10">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Key</TableHead>
                <TableHead className="text-right">Bytes stored</TableHead>
                <TableHead className="text-right">Bytes served</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {data?.per_key.length === 0 && (
                <TableRow>
                  <TableCell colSpan={3} className="text-center text-xs text-muted-foreground">
                    No usage yet. Upload a file via{" "}
                    <code className="font-mono">huggingface-cli upload</code> to populate this
                    table.
                  </TableCell>
                </TableRow>
              )}
              {data?.per_key.map((r) => (
                <TableRow key={r.key_id}>
                  <TableCell className="font-medium">{keyLabel(r.key_id)}</TableCell>
                  <TableCell className="text-right tabular-nums">{bytes(r.bytes_stored)}</TableCell>
                  <TableCell className="text-right tabular-nums">{bytes(r.bytes_served)}</TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
      </section>

      <section>
        <header className="mb-3 flex items-center justify-between">
          <h2 className="font-heading text-base font-medium">Recent activity</h2>
        </header>
        <div className="rounded-none bg-card ring-1 ring-foreground/10">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Time</TableHead>
                <TableHead>Event</TableHead>
                <TableHead>Xorb hash</TableHead>
                <TableHead className="text-right">Bytes</TableHead>
                <TableHead>Cache hit</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {data?.recent_activity.length === 0 && (
                <TableRow>
                  <TableCell colSpan={5} className="text-center text-xs text-muted-foreground">
                    No activity yet.
                  </TableCell>
                </TableRow>
              )}
              {data?.recent_activity.map((a, i) => (
                <TableRow key={`${a.ts}-${i}`}>
                  <TableCell className="font-mono text-xs">
                    {new Date(a.ts).toLocaleTimeString()}
                  </TableCell>
                  <TableCell>
                    <code className="font-mono text-xs">{a.event}</code>
                  </TableCell>
                  <TableCell className="font-mono text-xs">
                    {a.hash ? `${a.hash.slice(0, 12)}…` : "—"}
                  </TableCell>
                  <TableCell className="text-right tabular-nums">
                    {a.bytes !== null ? bytes(a.bytes) : "—"}
                  </TableCell>
                  <TableCell>{a.cache_hit === null ? "—" : a.cache_hit ? "yes" : "no"}</TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
      </section>

      <footer className="flex items-center justify-between border-t border-foreground/10 pt-6 text-xs text-muted-foreground">
        <p>
          For the authoritative Phase 5 benchmark report see{" "}
          <a
            href="https://github.com/siahub/siahub/blob/main/docs/benchmarks.md"
            className="underline underline-offset-4 hover:text-foreground"
          >
            docs/benchmarks.md
          </a>
          .
        </p>
        <Button asChild variant="outline" size="sm">
          <Link to="/stats/benchmarks">View benchmarks →</Link>
        </Button>
      </footer>
    </main>
  )
}
