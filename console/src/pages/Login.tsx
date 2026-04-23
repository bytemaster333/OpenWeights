import { useEffect } from "react"

import { CAS_URL } from "@/lib/api"

/**
 * `/login` — pure redirect-to-CAS entry point.
 *
 * Exists as an alias so that links captured elsewhere (error pages,
 * `<AuthGuard>` unauth redirect, external links) land somewhere sensible
 * rather than on a 404 if the user typed it directly. Redirects to CAS
 * `/auth/github/start`, which handles the OAuth round-trip.
 *
 * Uses `window.location.href` (not TanStack `<Link>`/`navigate`) because
 * the destination is a different origin — client-side routing would fail.
 */
export function LoginPage() {
  useEffect(() => {
    window.location.href = `${CAS_URL}/auth/github/start`
  }, [])

  return (
    <main className="mx-auto flex min-h-svh max-w-md items-center justify-center p-6">
      <p className="text-sm text-muted-foreground">Redirecting to GitHub…</p>
    </main>
  )
}
