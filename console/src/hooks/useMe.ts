import { useQuery } from "@tanstack/react-query"

import { ApiError, casFetch } from "@/lib/api"

/**
 * Canonical session user shape. Mirrors `MeResponse.user` from CAS (see
 * `cas/crates/siahub-cas-core/src/handlers/admin/me.rs`) and enforces three
 * load-bearing invariants at the type level:
 *
 * - ****: `id` is the GitHub numeric id (`i64`). It is the primary
 * key for all relational foreign references in the CAS schema. Never use
 * `email` or `login` as an identity key.
 * - **P13**: `email` is `string | null`. GitHub users with a `noreply`
 * configuration surface with a null email; the console renders `@{login}`
 * in both the null case and the `@users.noreply.github.com` case.
 * - `avatar_url` is also nullable — some users have no avatar. The UI should
 * fall back to a login-initial badge (future work) when null.*/
export type SessionUser = {
  /** : GitHub numeric id, NOT email.*/
  id: number
  login: string
  /** : may be null or `@users.noreply.github.com`. Never rendered raw.*/
  email: string | null
  avatar_url: string | null
  is_admin: boolean
}

type MeResponse = { user: SessionUser }

/**
 * React Query hook that resolves the current session user.
 *
 * 401 → returns `null` (unauthenticated); consumers such as `<AuthGuard>`
 * use that signal to redirect to `/login`.
 *
 * Other errors propagate as `ApiError` so that `<AppErrorBoundary>` (render
 * crashes) or future Sonner toasts (feature plans) can surface them.
 *
 * `refetchInterval` is intentionally omitted: session liveness is tracked
 * server-side via the rolling cookie TTL; polling `/admin/me` from
 * every authenticated page would burn CPU for zero user-observable benefit.*/
export function useMe() {
  return useQuery<SessionUser | null, ApiError>({
    queryKey: ["me"],
    queryFn: async () => {
      try {
        const { user } = await casFetch<MeResponse>("/admin/me")
        return user
      } catch (e) {
        if (e instanceof ApiError && e.status === 401) {
          // Unauthenticated — not an error, just a signal to redirect.
          return null
        }
        throw e
      }
    },
    retry: false,
    staleTime: 30_000,
  })
}
