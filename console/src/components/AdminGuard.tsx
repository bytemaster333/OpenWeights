import type { PropsWithChildren } from "react"

import { useMe } from "@/hooks/useMe"

/**
 * Client-side gate for admin-only pages (currently `/setup`).
 *
 * This is a UX convenience, NOT the security boundary. CAS enforces
 * `is_admin` on every admin endpoint server-side ( middleware). A
 * non-admin who guesses `/setup` would still get 403s from CAS if this
 * component didn't exist — they just wouldn't see the nice "Admins only"
 * message.
 *
 * Intentional choices:
 * - **No redirect.** A redirect to `/dashboard` would be lossy for two
 * scenarios: (a) a user whose `is_admin` flag was flipped mid-session
 * and a refresh would fix it, and (b) an operator misreading the URL —
 * they deserve the clear "Admins only" message, not a silent bounce.
 * - **Pending → neutral loading.** Same copy as `<AuthGuard>` so the two
 * nested guards (Auth outer, Admin inner) look visually continuous.
 * - **`useMe` already fetched upstream** — `<AuthGuard>` rendering this
 * component means the `/admin/me` query has resolved, so the child fetch
 * is a TanStack Query cache hit.*/
export function AdminGuard({ children }: PropsWithChildren) {
  const { data: user, isPending } = useMe()

  if (isPending) {
    return (
      <div
        className="flex min-h-svh items-center justify-center p-6"
        data-testid="admin-guard-loading"
      >
        <p className="text-sm text-muted-foreground">Loading…</p>
      </div>
    )
  }

  if (!user?.is_admin) {
    return (
      <main className="mx-auto max-w-lg px-6 py-16 text-center" data-testid="admin-guard-denied">
        <h1 className="font-heading text-2xl font-semibold">Admins only</h1>
        <p className="mt-2 text-sm text-muted-foreground">
          This page requires admin privileges. Ask the operator to flip your{" "}
          <code className="font-mono">is_admin</code> flag in Postgres.
        </p>
      </main>
    )
  }

  return <>{children}</>
}
