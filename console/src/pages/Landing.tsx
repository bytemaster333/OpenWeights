import { ArrowRight, CloudArrowUp, DownloadSimple, GithubLogo } from "@phosphor-icons/react"
import { useSearch } from "@tanstack/react-router"

import { OAuthErrorBanner, type OAuthErrorCode } from "@/components/OAuthErrorBanner"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { CAS_URL } from "@/lib/api"

export function LandingPage() {
  const search = useSearch({ strict: false }) as { error?: string }
  const errorCode = isOAuthErrorCode(search.error) ? search.error : undefined

  return (
    <main className="mx-auto max-w-4xl space-y-10 px-6 py-16">
      {errorCode ? <OAuthErrorBanner code={errorCode} /> : null}

      <section className="space-y-4">
        <h1 className="font-heading text-4xl font-medium tracking-tight">siahub</h1>
        <p className="max-w-2xl text-base/relaxed text-muted-foreground">
          a model hub with bytes on sia. point{" "}
          <code className="rounded bg-muted px-1 py-0.5 font-mono text-xs">HF_ENDPOINT</code> at
          a siahub deployment and use the <code className="rounded bg-muted px-1 py-0.5 font-mono text-xs">hf</code> cli as usual.
        </p>
      </section>

      <section className="grid gap-4 sm:grid-cols-2">
        <Card>
          <CardHeader className="flex flex-row items-center gap-2 pb-2">
            <CloudArrowUp weight="duotone" className="size-5 text-primary" />
            <CardTitle className="text-base">upload with hf cli</CardTitle>
          </CardHeader>
          <CardContent className="space-y-2 text-xs text-muted-foreground">
            <pre className="overflow-x-auto rounded bg-muted p-3 text-[0.7rem] leading-relaxed text-foreground">
              {`HF_TOKEN=<your-key> \\
HF_ENDPOINT=https://siahub.app \\
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
              {`HF_ENDPOINT=https://siahub.app \\
hf download <user>/<repo>`}
            </pre>
          </CardContent>
        </Card>
      </section>

      <section>
        <div className="flex flex-wrap gap-2">
          <Button asChild>
            <a href={`${CAS_URL}/auth/github/start`}>
              <GithubLogo data-icon="inline-start" weight="fill" />
              sign in with github
            </a>
          </Button>
          <Button asChild variant="outline">
            <a href="https://docs.siahub.app" target="_blank" rel="noreferrer">
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
