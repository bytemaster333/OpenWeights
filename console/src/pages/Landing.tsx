import { GithubLogo, WarningCircle } from "@phosphor-icons/react"
import { useSearch } from "@tanstack/react-router"

import { OAuthErrorBanner, type OAuthErrorCode } from "@/components/OAuthErrorBanner"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import { CAS_URL } from "@/lib/api"

/**
 * Landing page (route `/`). Primary entry point for unauthenticated users.
 *
 * AUTH-01, AUTH-02: sole authentication surface is the "Sign in with GitHub"
 * button, which performs a plain same-site navigation to CAS
 * `/auth/github/start`. CAS mints an OAuth state nonce, stores it in
 * `oauth_state` (TTL 10m), and 302s to GitHub — the browser, not the
 * console, crosses the cross-origin boundary.
 *
 * P14 surfacing: when the callback fails, CAS's `CallbackError` emits a
 * stable `code` (`oauth_state_mismatch`, `oauth_code_missing`,
 * `github_token_exchange_failed`). The callback handler appends
 * `?error=<code>` to the redirect URL so this page can render a friendly
 * banner; see `OAuthErrorBanner` for the code → copy map.
 */
export function LandingPage() {
  const search = useSearch({ strict: false }) as { error?: string }
  const errorCode = isOAuthErrorCode(search.error) ? search.error : undefined

  return (
    <main className="mx-auto flex min-h-svh max-w-2xl flex-col justify-center gap-6 px-6 py-16">
      {errorCode ? <OAuthErrorBanner code={errorCode} /> : null}

      <div>
        <h1 className="font-heading text-3xl font-medium tracking-tight">SiaHub</h1>
        <p className="mt-3 text-sm/relaxed text-muted-foreground">
          A Xet-compatible storage backend on Sia. Point{" "}
          <code className="rounded-none bg-muted px-1 py-0.5 font-mono text-xs">
            HF_XET_DATA_DEFAULT_CAS_ENDPOINT
          </code>{" "}
          at a SiaHub deployment and keep using{" "}
          <code className="rounded-none bg-muted px-1 py-0.5 font-mono text-xs">
            huggingface-cli
          </code>{" "}
          unchanged.
        </p>
      </div>

      <Card>
        <CardHeader>
          <CardTitle>Get started</CardTitle>
          <CardDescription>Sign in with GitHub to mint an API key.</CardDescription>
        </CardHeader>
        <CardContent>
          <Button asChild>
            {/*
              A plain `<a href>` (not TanStack `<Link>`) — the destination
              is CAS, a different origin. React-router-style client
              navigation would no-op against a same-SPA route table.
            */}
            <a href={`${CAS_URL}/auth/github/start`}>
              <GithubLogo data-icon="inline-start" weight="fill" />
              Sign in with GitHub
            </a>
          </Button>
        </CardContent>
      </Card>

      <p className="flex items-center gap-1.5 text-xs text-muted-foreground">
        <WarningCircle weight="regular" className="size-3.5" />
        Operator console only. Runs end-user uploads go through{" "}
        <code className="rounded-none bg-muted px-1 py-0.5 font-mono text-xs">huggingface-cli</code>
        , not the browser.
      </p>
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
