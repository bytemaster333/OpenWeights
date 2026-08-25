import { GithubLogo } from "@phosphor-icons/react"
import { useMutation, useQuery } from "@tanstack/react-query"
import { Link } from "@tanstack/react-router"
import { useState } from "react"
import { toast } from "sonner"

import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { ApiError, CAS_URL, authMethods, login } from "@/lib/api"

/**
 * `/login` — explicit sign-in page. Renders whichever methods the operator
 * enabled (`GET /auth/methods`):
 *
 * - **password** — single-operator self-hosting. No GitHub OAuth app needed;
 *   the operator sets `OPENWEIGHTS_ADMIN_PASSWORD` in `.env`.
 * - **GitHub OAuth** — for teams; unchanged from the original flow.
 *
 * Both can be on at once. Sign-out paths target `/` (Landing) per AuthGuard.
 */
export function LoginPage() {
  const [password, setPassword] = useState("")
  const methods = useQuery({
    queryKey: ["auth-methods"],
    queryFn: authMethods,
    staleTime: 60_000,
    retry: false,
  })

  const loginMut = useMutation({
    mutationFn: () => login(password),
    onSuccess: () => {
      // Cookie is set; hard-navigate so every query refetches with the new
      // session (mirrors the OAuth callback's full redirect to /dashboard).
      window.location.assign("/dashboard")
    },
    onError: (err) => {
      const msg =
        err instanceof ApiError && err.status === 401
          ? "Incorrect password."
          : `Sign-in failed: ${(err as Error).message}`
      toast.error(msg)
    },
  })

  const showPassword = methods.data?.password ?? false
  const showGithub = methods.data?.github ?? false
  const nothingConfigured = methods.isSuccess && !showPassword && !showGithub

  return (
    <main className="mx-auto flex min-h-svh max-w-md flex-col items-center justify-center gap-6 p-6">
      <div className="text-center">
        <h1 className="font-heading text-2xl font-medium tracking-tight">Sign in</h1>
        <p className="mt-2 text-sm text-muted-foreground">
          Sign in to manage API keys, models, and assets.
        </p>
      </div>

      {showPassword && (
        <form
          className="flex w-full flex-col gap-3"
          onSubmit={(e) => {
            e.preventDefault()
            if (password && !loginMut.isPending) loginMut.mutate()
          }}
        >
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="admin-password">Password</Label>
            <Input
              id="admin-password"
              type="password"
              autoComplete="current-password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              placeholder="Admin password"
              data-testid="login-password"
            />
          </div>
          <Button
            type="submit"
            disabled={!password || loginMut.isPending}
            data-testid="login-submit"
          >
            {loginMut.isPending ? "Signing in…" : "Sign in"}
          </Button>
        </form>
      )}

      {showPassword && showGithub && (
        <div className="flex w-full items-center gap-3 text-xs text-muted-foreground">
          <span className="h-px flex-1 bg-border" />
          or
          <span className="h-px flex-1 bg-border" />
        </div>
      )}

      {showGithub && (
        <Button asChild variant={showPassword ? "outline" : "default"} className="w-full">
          <a href={`${CAS_URL}/auth/github/start`}>
            <GithubLogo data-icon="inline-start" weight="fill" />
            Continue with GitHub
          </a>
        </Button>
      )}

      {nothingConfigured && (
        <p className="text-center text-sm text-muted-foreground">
          No sign-in method is configured. Set <code>OPENWEIGHTS_ADMIN_PASSWORD</code> (or GitHub
          OAuth) in the deployment's <code>.env</code> and restart.
        </p>
      )}

      <Link to="/" className="text-xs text-muted-foreground hover:text-foreground">
        ← Back home
      </Link>
    </main>
  )
}
