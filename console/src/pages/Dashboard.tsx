import {
  ClockIcon,
  CubeIcon,
  GaugeIcon,
  GlobeIcon,
  KeyIcon,
  RocketLaunchIcon,
} from "@phosphor-icons/react"
import { Link } from "@tanstack/react-router"

import { StatsTile } from "@/components/StatsTile"
import { Button } from "@/components/ui/button"
import { Skeleton } from "@/components/ui/skeleton"
import { useKeys } from "@/hooks/useKeys"
import { useMe } from "@/hooks/useMe"
import { useModels } from "@/hooks/useModels"
import { usePlatformStats, useStats } from "@/hooks/useStats"
import { CAS_URL } from "@/lib/api"
import { eventHasCacheSemantics, formatEvent } from "@/lib/eventLabels"
import { formatBytes, formatRelative } from "@/lib/format"

/**
 * `/dashboard` — signed-in landing. Actionable at-a-glance:
 *
 * 1. KPI strip — uploaded · served · my models · keys.
 * 2. "Your models" — 5 most-recent repos owned by the session user.
 * 3. "Recent activity" — 5 most-recent usage_log events across all
 * of the user's keys, with deep-links into /assets/:hash.
 * 4. "Quick upload" — the single command to run after copying a key.
 *
 * All data comes from hooks already wired for other pages; no new CAS
 * endpoints are needed.*/

const MY_MODELS_LIMIT = 5
const RECENT_ACTIVITY_LIMIT = 5
const ALL_MODELS_LIMIT = 8

export function DashboardPage() {
  const { data: user } = useMe()
  const { data: stats, isPending: statsPending } = useStats()
  const { data: platform, isPending: platformPending } = usePlatformStats()
  const { data: keys } = useKeys()
  const { data: models, isPending: modelsPending } = useModels()

  if (!user) return null

  // `useModels` returns ALL public repos on this deployment. Filter down
  // to rows whose `author` matches the session user's GitHub login — same
  // convention we use for ownership in ModelDetail.tsx.
  const myModelsAll = (models ?? []).filter((m) => m.author === user.login)
  const myModels = myModelsAll.slice(0, MY_MODELS_LIMIT)
  const myDownloadsTotal = myModelsAll.reduce(
    (n, m) => n + (m.downloadsTotal ?? 0),
    0,
  )

  const recentActivity = (stats?.recent_activity ?? []).slice(
    0,
    RECENT_ACTIVITY_LIMIT,
  )

  // "All models" widget — top N by recency across the platform. `useModels`
  // already returns rows ORDER BY r.updated_at DESC; we just slice.
  const allModels = (models ?? []).slice(0, ALL_MODELS_LIMIT)

  const uploadCmd = `HF_TOKEN=<your-siahub-key> HF_ENDPOINT=${CAS_URL} \\
  hf upload ${user.login}/<repo-name> ./files`

  return (
    <main className="mx-auto max-w-6xl space-y-8 px-6 py-8">
      <header>
        <div className="flex items-center gap-3">
          <GaugeIcon size={22} weight="light" className="text-muted-foreground" />
          <h1 className="font-heading text-2xl font-semibold tracking-tight">
            Welcome, @{user.login}
          </h1>
        </div>
        <p className="mt-1 text-sm text-muted-foreground">
          Your SiaHub footprint at a glance.
        </p>
      </header>

      {/* Platform-wide totals — same numbers for everyone signed in*/}
      <section>
        <h2 className="mb-3 flex items-center gap-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          <GlobeIcon size={14} weight="light" /> Platform totals
        </h2>
        <div className="grid grid-cols-2 gap-4 lg:grid-cols-4">
          <StatsTile
            label="Models"
            loading={platformPending}
            value={platform ? String(platform.total_models) : "—"}
            subtext={
              platform ? `${platform.total_users} contributor${platform.total_users === 1 ? "" : "s"}` : ""
            }
          />
          <StatsTile
            label="Stored"
            loading={platformPending}
            value={platform ? formatBytes(platform.total_size_bytes) : "—"}
            subtext={platform ? `${platform.total_files} file${platform.total_files === 1 ? "" : "s"}` : ""}
          />
          <StatsTile
            label="Downloads"
            loading={platformPending}
            value={platform ? String(platform.total_downloads) : "—"}
            subtext={platform ? `+${platform.downloads_today} today` : ""}
          />
          {/* "On Sia" — the load-bearing demo number: bytes really pinned on
              Sia hosts (vs the durable Postgres cache). Click-through to
              /setup which has the explorer drilldown. */}
          <Link
            to="/setup"
            className="rounded transition-colors hover:bg-muted/30"
          >
            <StatsTile
              label="On Sia"
              loading={platformPending}
              value={platform ? formatBytes(platform.bytes_on_sia) : "—"}
              subtext={
                platform
                  ? `${platform.xorbs_pinned}/${platform.xorbs_total} xorbs pinned`
                  : ""
              }
            />
          </Link>
        </div>
      </section>

      {/* User KPI strip*/}
      <section>
        <h2 className="mb-3 flex items-center gap-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          <GaugeIcon size={14} weight="light" /> Your footprint
        </h2>
        <div className="grid grid-cols-2 gap-4 lg:grid-cols-4">
          <StatsTile
            label="Bytes uploaded"
            loading={statsPending}
            value={stats ? formatBytes(stats.total_bytes_stored) : "—"}
            subtext="Sia-pending + Sia-pinned"
          />
          <StatsTile
            label="Downloads"
            loading={modelsPending}
            value={String(myDownloadsTotal)}
            subtext="on your models"
          />
          <StatsTile
            label="Your models"
            loading={modelsPending}
            value={String(myModels.length)}
            subtext={`${models?.length ?? 0} total`}
          />
          <StatsTile
            label="API keys"
            value={String(keys?.length ?? 0)}
            subtext="active"
          />
        </div>
      </section>

      {/* Two-column: Your models / Recent activity*/}
      <div className="grid gap-6 lg:grid-cols-2">
        <section className="rounded border bg-muted/10 p-4">
          <div className="mb-3 flex items-center justify-between">
            <h2 className="flex items-center gap-2 text-sm font-semibold">
              <CubeIcon size={16} weight="light" /> Your models
            </h2>
            <Link
              to="/models"
              className="text-xs text-muted-foreground hover:text-foreground"
            >
              See all →
            </Link>
          </div>

          {modelsPending && (
            <div className="space-y-2">
              <Skeleton className="h-10 w-full" />
              <Skeleton className="h-10 w-full" />
            </div>
          )}

          {!modelsPending && myModels.length === 0 && (
            <div className="py-6 text-center text-xs text-muted-foreground">
              No models yet. Push one with the command below.
            </div>
          )}

          {!modelsPending && myModels.length > 0 && (
            <ul className="divide-y divide-border text-sm">
              {myModels.map((m) => {
                const [owner, name] = m.id.split("/")
                return (
                  <li key={m.id}>
                    <Link
                      to="/models/$owner/$repo"
                      params={{ owner, repo: name }}
                      className="flex items-baseline justify-between gap-3 py-2 hover:bg-muted/30"
                    >
                      <span className="font-medium">{name}</span>
                      <span className="flex items-baseline gap-3 text-xs text-muted-foreground">
                        <span>{formatBytes(m.size)}</span>
                        <span>{formatRelative(m.lastModified)}</span>
                      </span>
                    </Link>
                  </li>
                )
              })}
            </ul>
          )}
        </section>

        <section className="rounded border bg-muted/10 p-4">
          <div className="mb-3 flex items-center justify-between">
            <h2 className="flex items-center gap-2 text-sm font-semibold">
              <ClockIcon size={16} weight="light" /> Recent activity
            </h2>
            <Link
              to="/stats"
              className="text-xs text-muted-foreground hover:text-foreground"
            >
              Full stats →
            </Link>
          </div>

          {statsPending && (
            <div className="space-y-2">
              <Skeleton className="h-8 w-full" />
              <Skeleton className="h-8 w-full" />
            </div>
          )}

          {!statsPending && recentActivity.length === 0 && (
            <div className="py-6 text-center text-xs text-muted-foreground">
              No activity yet.
            </div>
          )}

          {!statsPending && recentActivity.length > 0 && (
            <ul className="divide-y divide-border text-sm">
              {recentActivity.map((a, i) => (
                <li
                  key={`${a.ts}-${i}`}
                  className="flex items-baseline justify-between gap-2 py-2 text-xs"
                >
                  <span className="flex items-baseline gap-2">
                    <span className="font-medium">{formatEvent(a.event)}</span>
                    {a.hash && (
                      <Link
                        to="/assets/$hash"
                        params={{ hash: a.hash }}
                        className="font-mono text-muted-foreground hover:text-foreground"
                      >
                        {a.hash.slice(0, 8)}…
                      </Link>
                    )}
                  </span>
                  <span className="flex items-baseline gap-3 text-muted-foreground">
                    {a.bytes !== null && <span>{formatBytes(a.bytes)}</span>}
                    {eventHasCacheSemantics(a.event) &&
                      a.cache_hit !== null && (
                        <span className={a.cache_hit ? "text-primary" : ""}>
                          {a.cache_hit ? "✓ hit" : "✗ miss"}
                        </span>
                      )}
                    <span>{formatRelative(a.ts)}</span>
                  </span>
                </li>
              ))}
            </ul>
          )}
        </section>
      </div>

      {/* All models — platform-wide recency feed */}
      <section className="rounded border bg-muted/10 p-4">
        <div className="mb-3 flex items-center justify-between">
          <h2 className="flex items-center gap-2 text-sm font-semibold">
            <GlobeIcon size={16} weight="light" /> All models
          </h2>
          <Link
            to="/models"
            className="text-xs text-muted-foreground hover:text-foreground"
          >
            See all →
          </Link>
        </div>

        {modelsPending && (
          <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
            {Array.from({ length: 4 }).map((_, i) => (
              <Skeleton key={i} className="h-20 w-full" />
            ))}
          </div>
        )}

        {!modelsPending && allModels.length === 0 && (
          <div className="py-6 text-center text-xs text-muted-foreground">
            No models on this deployment yet.
          </div>
        )}

        {!modelsPending && allModels.length > 0 && (
          <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
            {allModels.map((m) => {
              const [owner, name] = m.id.split("/")
              return (
                <Link
                  key={m.id}
                  to="/models/$owner/$repo"
                  params={{ owner, repo: name }}
                  className="rounded border bg-background/40 p-3 text-sm transition-colors hover:border-primary/40 hover:bg-muted/30"
                >
                  <div className="truncate font-medium" title={m.id}>
                    {name}
                  </div>
                  <div className="mt-0.5 truncate text-xs text-muted-foreground">
                    @{owner}
                  </div>
                  <div className="mt-2 flex items-baseline justify-between gap-2 text-xs text-muted-foreground">
                    <span>{formatBytes(m.size)}</span>
                    <span>
                      {(m.downloadsTotal ?? 0).toLocaleString()}{" "}
                      <span className="text-muted-foreground/70">dl</span>
                    </span>
                  </div>
                </Link>
              )
            })}
          </div>
        )}
      </section>

      {/* Quick upload*/}
      <section className="rounded border bg-muted/20 p-4">
        <div className="mb-3 flex items-center gap-2">
          <RocketLaunchIcon size={16} weight="light" />
          <h2 className="text-sm font-semibold">Quick upload</h2>
        </div>
        <p className="mb-3 text-xs text-muted-foreground">
          Swap <code>&lt;your-siahub-key&gt;</code> for a plaintext key from{" "}
          <Link to="/keys" className="underline hover:text-foreground">
            /keys
          </Link>
          .
        </p>
        <pre className="overflow-x-auto rounded border bg-background p-3 font-mono text-xs">
          {uploadCmd}
        </pre>
      </section>

      {/* Footer CTAs*/}
      <section className="flex flex-wrap gap-2 border-t pt-6">
        <Button asChild variant="outline" size="sm">
          <Link to="/stats">View full stats →</Link>
        </Button>
        <Button asChild variant="outline" size="sm">
          <Link to="/keys">
            <KeyIcon data-icon="inline-start" weight="light" />
            Manage API keys
          </Link>
        </Button>
        <Button asChild variant="outline" size="sm">
          <Link to="/models">
            <CubeIcon data-icon="inline-start" weight="light" />
            Browse models
          </Link>
        </Button>
      </section>
    </main>
  )
}
