import { Link } from "@tanstack/react-router"

import { StatsTile } from "@/components/StatsTile"
import { UserMenu } from "@/components/UserMenu"
import { Button } from "@/components/ui/button"
import { useKeys } from "@/hooks/useKeys"
import { useMe } from "@/hooks/useMe"
import { useStats } from "@/hooks/useStats"

/**
 * `/dashboard` (04-07 replaces the 04-03 stub).
 *
 * Compact welcome-view for a signed-in user: handle + 3 KPI tiles (stored,
 * bytes served, API key count) + quick links to `/stats` and `/keys`.
 *
 * Tiles reuse the same `useStats()` hook as the `/stats` page so the 10s
 * refetch loop is shared (one query, one cache entry, zero duplicated
 * network calls).
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

export function DashboardPage() {
  const { data: user } = useMe()
  const { data: stats, isPending: statsPending } = useStats()
  const { data: keys } = useKeys()

  if (!user) return null // Defensive — AuthGuard already bounced.

  return (
    <main className="mx-auto max-w-5xl space-y-8 px-6 py-10">
      <header className="flex items-center justify-between">
        <div>
          <h1 className="font-heading text-2xl font-medium tracking-tight">
            Welcome, @{user.login}
          </h1>
          <p className="mt-1 text-xs text-muted-foreground">
            {user.is_admin ? "Admin" : "Member"} — your current SiaHub footprint.
          </p>
        </div>
        <UserMenu user={user} />
      </header>

      <div className="grid grid-cols-1 gap-4 sm:grid-cols-3">
        <StatsTile
          label="Stored on Sia"
          loading={statsPending}
          value={stats ? bytes(stats.total_bytes_stored) : "—"}
        />
        <StatsTile
          label="Bytes served"
          loading={statsPending}
          value={stats ? bytes(stats.total_bytes_served) : "—"}
        />
        <StatsTile label="API keys" value={String(keys?.length ?? 0)} />
      </div>

      <section className="flex flex-wrap gap-2">
        <Button asChild variant="outline" size="sm">
          <Link to="/stats">View full stats →</Link>
        </Button>
        <Button asChild variant="outline" size="sm">
          <Link to="/keys">Manage API keys →</Link>
        </Button>
        <Button asChild variant="outline" size="sm">
          <Link to="/stats/benchmarks">Benchmarks →</Link>
        </Button>
      </section>
    </main>
  )
}
