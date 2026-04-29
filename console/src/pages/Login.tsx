import { GithubLogo } from "@phosphor-icons/react"
import { Link } from "@tanstack/react-router"

import { Button } from "@/components/ui/button"
import { CAS_URL } from "@/lib/api"

/**
 * `/login` — explicit sign-in CTA. Earlier versions auto-redirected to
 * `/auth/github/start` on mount, which trapped any logged-out user who
 * landed here from a bookmark or external link directly into the OAuth
 * flow with no chance to read what they were signing into. We now render
 * a button so the click is intentional.
 *
 * Sign-out paths still target `/` (Landing) per AuthGuard; this page
 * exists for direct navigations only.
 */
export function LoginPage() {
  return (
    <main className="mx-auto flex min-h-svh max-w-md flex-col items-center justify-center gap-6 p-6">
      <div className="text-center">
        <h1 className="font-heading text-2xl font-medium tracking-tight">Sign in</h1>
        <p className="mt-2 text-sm text-muted-foreground">
          GitHub OAuth — used to identify your account and gate the upload API.
        </p>
      </div>
      <Button asChild>
        <a href={`${CAS_URL}/auth/github/start`}>
          <GithubLogo data-icon="inline-start" weight="fill" />
          Continue with GitHub
        </a>
      </Button>
      <Link to="/" className="text-xs text-muted-foreground hover:text-foreground">
        ← Back home
      </Link>
    </main>
  )
}
