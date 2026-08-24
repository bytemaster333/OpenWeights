import { useNavigate } from "@tanstack/react-router"
import { type PropsWithChildren, useEffect } from "react"

import { useMe } from "@/hooks/useMe"

/**
 * Higher-order wrapper for protected routes.
 *
 * Behavior:
 * - `/admin/me` → 200 → render children.
 * - `/admin/me` → 401 → navigate to `/` (the public landing, which hosts the sign-in button).
 * - Query pending → render a neutral "Loading…" placeholder.
 * - Query error (500, network, etc.) → propagate to `<AppErrorBoundary>`
 * via the thrown `ApiError` from `useMe`.
 *
 * This is the only client-side auth gate in the app; CAS is the source of
 * truth and enforces the cookie on every request. The guard is a
 * UX convenience — a URL-typed `/dashboard` visit by an unauth'd user
 * shouldn't render the shell before bouncing.*/
export function AuthGuard({ children }: PropsWithChildren) {
  const { data: user, isPending, isError, error } = useMe()
  const navigate = useNavigate()

  useEffect(() => {
    // `user === null` is the "unauthenticated" sentinel that `useMe` returns
    // on 401 (NOT `undefined`, which is the pending state in TanStack Query).
    if (!isPending && user === null) {
      void navigate({ to: "/" })
    }
  }, [isPending, user, navigate])

  if (isPending) {
    return (
      <div className="flex min-h-svh items-center justify-center p-6">
        <p className="text-sm text-muted-foreground">Loading…</p>
      </div>
    )
  }

  if (isError) {
    // Let `<AppErrorBoundary>` handle it — throwing from a component is the
    // React-idiomatic way to surface a render-path error to the boundary.
    throw error
  }

  // Unauth'd — `useEffect` above has already scheduled a redirect; render
  // nothing this tick to avoid a flash of protected content.
  if (user === null) return null

  return <>{children}</>
}
