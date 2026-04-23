import { CheckCircle, Warning, WarningCircle } from "@phosphor-icons/react"

import { OAuthErrorBanner } from "@/components/OAuthErrorBanner"
import { Badge } from "@/components/ui/badge"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { type SubsystemStatus, useSetupStatus } from "@/hooks/useSetupStatus"

/**
 * `/setup` — first-run / diagnostic page for the self-hosting operator
 * (CONSOLE-11, CONSOLE-12, OPS-01).
 *
 * Read-only status tiles only — no toggles, no "fix it" buttons. Every knob
 * on this page is sourced from the CAS `.env` file on the host, and the
 * self-host guide (DOCS-02) walks the operator through each one.
 *
 * Tiles (data source in parens):
 *   1. Postgres              (CAS `/admin/setup/status` → pg round-trip)
 *   2. Redis                 (CAS `/admin/setup/status` → redis PING)
 *   3. indexd                (CAS `/admin/setup/status` → indexd self-report +
 *                             consensus-synced flag; gotcha #6)
 *   4. GitHub OAuth          (CAS `/admin/setup/status` → env var presence;
 *                             P14 mitigation — missing config shows operator
 *                             copy + OAuthErrorBanner with hint)
 *   5. V2 reconstruction     (CAS `/admin/setup/status` → feature flag; read-
 *                             only per gotcha #3 / plan Ambiguity 3)
 *
 * D-09 / notes.md: indexer is `indexd` only — no backend choice, no toggle.
 * The URL is surfaced read-only so the operator can see which `INDEXD_URL`
 * they configured.
 */

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

  if (isPending) {
    return (
      <main className="mx-auto max-w-4xl px-6 py-10" data-testid="setup-loading">
        <p className="text-sm text-muted-foreground">Loading status…</p>
      </main>
    )
  }

  if (error || !data) {
    return (
      <main className="mx-auto max-w-4xl px-6 py-10" data-testid="setup-error">
        <h1 className="font-heading text-2xl font-semibold">Setup</h1>
        <p className="mt-2 text-sm text-destructive">
          Failed to load status. The CAS service may be down.
        </p>
        <button
          type="button"
          className="mt-4 text-xs underline"
          onClick={() => void refetch()}
          data-testid="setup-retry"
        >
          Retry
        </button>
      </main>
    )
  }

  const oauthMissing = !data.github_oauth.configured

  return (
    <main className="mx-auto max-w-4xl space-y-6 px-6 py-10" data-testid="setup-page">
      <header>
        <h1 className="font-heading text-2xl font-semibold tracking-tight">Setup</h1>
        <p className="mt-1 text-sm text-muted-foreground">
          Self-hosted operator diagnostics. Read-only — all values are sourced from the CAS{" "}
          <code className="font-mono">.env</code>.
        </p>
      </header>

      {oauthMissing && (
        // P14 mitigation — the OAuth banner on /setup gives operators the
        // exact remedy copy even before they try to sign someone in.
        <OAuthErrorBanner code="oauth_client_not_configured" />
      )}

      <div className="grid gap-4 sm:grid-cols-2">
        <Card data-testid="setup-tile-postgres">
          <CardHeader className="flex flex-row items-center justify-between pb-2">
            <CardTitle>Postgres</CardTitle>
            <StatusBadge s={data.postgres} />
          </CardHeader>
          <CardContent className="text-xs text-muted-foreground">
            Latency: <span className="font-mono">{latencyText(data.postgres.latency_ms)}</span>
          </CardContent>
        </Card>

        <Card data-testid="setup-tile-redis">
          <CardHeader className="flex flex-row items-center justify-between pb-2">
            <CardTitle>Redis</CardTitle>
            <StatusBadge s={data.redis} />
          </CardHeader>
          <CardContent className="text-xs text-muted-foreground">
            Latency: <span className="font-mono">{latencyText(data.redis.latency_ms)}</span>
          </CardContent>
        </Card>

        <Card data-testid="setup-tile-indexd">
          <CardHeader className="flex flex-row items-center justify-between pb-2">
            <CardTitle>indexd (self-hosted)</CardTitle>
            <StatusBadge s={data.indexd} />
          </CardHeader>
          <CardContent className="space-y-1 text-xs text-muted-foreground">
            <div>
              URL:{" "}
              <code className="font-mono" data-testid="setup-indexd-url">
                {data.indexd.url}
              </code>
            </div>
            <div>
              Synced:{" "}
              <span
                className="font-mono"
                data-testid="setup-indexd-synced"
                data-synced={data.indexd.synced ? "true" : "false"}
              >
                {data.indexd.synced ? "yes" : "no"}
              </span>
            </div>
            <div>
              Latency: <span className="font-mono">{latencyText(data.indexd.latency_ms)}</span>
            </div>
            <p className="pt-1 text-[0.7rem]">
              Indexer backend is <code className="font-mono">indexd</code> (D-09); no runtime
              choice.
            </p>
          </CardContent>
        </Card>

        <Card data-testid="setup-tile-oauth">
          <CardHeader className="flex flex-row items-center justify-between pb-2">
            <CardTitle>GitHub OAuth</CardTitle>
            <Badge
              variant={data.github_oauth.configured ? "default" : "destructive"}
              data-testid="setup-oauth-badge"
              data-configured={data.github_oauth.configured ? "true" : "false"}
            >
              {data.github_oauth.configured ? (
                <CheckCircle weight="fill" data-icon="inline-start" />
              ) : (
                <WarningCircle weight="fill" data-icon="inline-start" />
              )}
              {data.github_oauth.configured ? "configured" : "not configured"}
            </Badge>
          </CardHeader>
          <CardContent className="space-y-2 text-xs text-muted-foreground">
            {data.github_oauth.configured ? (
              <p>Client ID + secret detected in .env.</p>
            ) : (
              <>
                <p>
                  Set <code className="font-mono">GITHUB_OAUTH_CLIENT_ID</code>,{" "}
                  <code className="font-mono">GITHUB_OAUTH_CLIENT_SECRET</code>, and{" "}
                  <code className="font-mono">GITHUB_OAUTH_CALLBACK_URL</code> in{" "}
                  <code className="font-mono">.env</code>, then restart siahub-cas.
                </p>
                <p className="text-[0.7rem]">
                  P14 hint: the callback URL registered on GitHub must exactly match{" "}
                  <code className="font-mono">GITHUB_OAUTH_CALLBACK_URL</code>.
                </p>
              </>
            )}
          </CardContent>
        </Card>

        <Card data-testid="setup-tile-v2" className="sm:col-span-2">
          <CardHeader className="flex flex-row items-center justify-between pb-2">
            <CardTitle>V2 reconstruction</CardTitle>
            <Badge
              variant={data.v2_reconstruction_enabled ? "default" : "secondary"}
              data-testid="setup-v2-badge"
            >
              {data.v2_reconstruction_enabled ? "enabled" : "disabled"}
            </Badge>
          </CardHeader>
          <CardContent className="space-y-1 text-xs text-muted-foreground">
            <div>
              Flag:{" "}
              <code className="font-mono" data-testid="setup-v2-flag">
                {String(data.v2_reconstruction_enabled)}
              </code>
            </div>
            <p className="pt-1">
              Read-only informational — operator-set in <code className="font-mono">.env</code>
              as <code className="font-mono">SIAHUB_V2_RECONSTRUCTION_ENABLED</code>. Phase 5 flips
              this for the hosted demo once the gateway's multi-range serving lands (gotcha #3).
            </p>
          </CardContent>
        </Card>
      </div>
    </main>
  )
}
