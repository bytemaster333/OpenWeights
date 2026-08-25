import { ArrowRight, CloudArrowUp, CloudCheckIcon, DownloadSimple } from "@phosphor-icons/react"
import { Link, useSearch } from "@tanstack/react-router"

import { OAuthErrorBanner, type OAuthErrorCode } from "@/components/OAuthErrorBanner"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { usePlatformStats } from "@/hooks/useStats"
import { formatBytes } from "@/lib/format"

export function LandingPage() {
  const search = useSearch({ strict: false }) as { error?: string }
  const errorCode = isOAuthErrorCode(search.error) ? search.error : undefined
  const { data: platform } = usePlatformStats()

  return (
    <main className="mx-auto max-w-4xl space-y-10 px-6 py-16">
      {errorCode ? <OAuthErrorBanner code={errorCode} /> : null}

      <section className="space-y-4">
        <h1 className="font-heading text-4xl font-medium tracking-tight">openweights</h1>
        <p className="max-w-2xl text-base/relaxed text-muted-foreground">
          a model hub with bytes on sia. point{" "}
          <code className="rounded bg-muted px-1 py-0.5 font-mono text-xs">HF_ENDPOINT</code> at a
          openweights deployment and use the{" "}
          <code className="rounded bg-muted px-1 py-0.5 font-mono text-xs">hf</code> cli as usual.
        </p>
      </section>

      {/* Live platform numbers — proves "actually on Sia" without scrolling. */}
      {platform && (
        <section className="grid grid-cols-2 gap-3 sm:grid-cols-4">
          <Stat label="Models" value={String(platform.total_models)} />
          <Stat label="Stored" value={formatBytes(platform.total_size_bytes)} />
          <Stat label="Downloads" value={String(platform.total_downloads)} />
          <Stat
            label="On Sia"
            value={formatBytes(platform.bytes_on_sia)}
            sub={`${platform.xorbs_pinned}/${platform.xorbs_total} xorbs`}
            href="/setup"
            highlight
          />
        </section>
      )}

      <section className="grid gap-4 sm:grid-cols-2">
        <Card>
          <CardHeader className="flex flex-row items-center gap-2 pb-2">
            <CloudArrowUp weight="duotone" className="size-5 text-primary" />
            <CardTitle className="text-base">upload with hf cli</CardTitle>
          </CardHeader>
          <CardContent className="space-y-2 text-xs text-muted-foreground">
            <pre className="overflow-x-auto rounded bg-muted p-3 text-[0.7rem] leading-relaxed text-foreground">
              {`HF_TOKEN=<your-key> \\
HF_ENDPOINT=https://example.com \\
hf upload <user>/<repo> ./model.safetensors`}
            </pre>
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="flex flex-row items-center gap-2 pb-2">
            <DownloadSimple weight="duotone" className="size-5 text-primary" />
            <CardTitle className="text-base">download</CardTitle>
          </CardHeader>
          <CardContent className="space-y-2 text-xs text-muted-foreground">
            <pre className="overflow-x-auto rounded bg-muted p-3 text-[0.7rem] leading-relaxed text-foreground">
              {`HF_ENDPOINT=https://example.com \\
hf download <user>/<repo>`}
            </pre>
          </CardContent>
        </Card>
      </section>

      <section>
        <div className="flex flex-wrap gap-2">
          <Button asChild>
            <Link to="/login">sign in</Link>
          </Button>
          <Button asChild variant="outline">
            <a href="https://bytemaster333.github.io/OpenWeights" target="_blank" rel="noreferrer">
              docs
              <ArrowRight data-icon="inline-end" weight="regular" />
            </a>
          </Button>
        </div>
      </section>
    </main>
  )
}

const OAUTH_ERROR_CODES = [
  "oauth_state_mismatch",
  "oauth_code_missing",
  "github_token_exchange_failed",
  "oauth_client_not_configured",
  "oauth_callback_host_mismatch",
] as const satisfies readonly OAuthErrorCode[]

function isOAuthErrorCode(v: unknown): v is OAuthErrorCode {
  return typeof v === "string" && (OAUTH_ERROR_CODES as readonly string[]).includes(v)
}

function Stat({
  label,
  value,
  sub,
  href,
  highlight,
}: {
  label: string
  value: string
  sub?: string
  href?: string
  highlight?: boolean
}) {
  const content = (
    <>
      <div className="flex items-center gap-1.5 text-[0.7rem] uppercase tracking-wide text-muted-foreground">
        {highlight && <CloudCheckIcon size={12} weight="light" className="text-primary" />}
        {label}
      </div>
      <div className="font-heading text-xl font-medium tabular-nums">{value}</div>
      {sub && <div className="text-[0.7rem] text-muted-foreground">{sub}</div>}
    </>
  )
  const className = `rounded border bg-muted/10 px-4 py-3 ${
    highlight ? "border-primary/30 bg-primary/5" : ""
  } ${href ? "transition-colors hover:bg-muted/30" : ""}`
  return href ? (
    <Link to={href} className={className}>
      {content}
    </Link>
  ) : (
    <div className={className}>{content}</div>
  )
}
