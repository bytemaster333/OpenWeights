import { SignOut, User } from "@phosphor-icons/react"
import { useMutation, useQueryClient } from "@tanstack/react-query"

import { Button } from "@/components/ui/button"
import type { SessionUser } from "@/hooks/useMe"
import { logout } from "@/lib/api"

/**
 * Header user menu. Renders the signed-in GitHub handle + avatar, and a
 * "Sign out" button that hits CAS `POST /auth/logout`.
 *
 * ** (email nullability) contract:**
 *
 * This component NEVER renders `user.email`. GitHub users with a `noreply`
 * email surface as `null`, and even when an email is set it is PII we have
 * no reason to show in the chrome. Display identity is always `@{login}` —
 * a GitHub-native handle that every user has.
 *
 * Tests in `UserMenu.test.tsx` lock this: a snapshot scan for `null`,
 * `@users.noreply.github.com`, and a regex email pattern MUST all miss.*/
export function UserMenu({ user }: { user: SessionUser }) {
  const qc = useQueryClient()

  const logoutMut = useMutation({
    mutationFn: logout,
    onSuccess: () => {
      // Immediately invalidate the cached session so any in-flight render
      // path sees `null` before the navigation lands.
      qc.setQueryData(["me"], null)
      // Full navigation (not client-side) — we want the browser to drop
      // the (now-cleared) `siahub_session` cookie from its jar and any
      // feature plans' cached query state to be nuked via full reload.
      window.location.href = "/"
    },
  })

  return (
    <div className="flex items-center gap-2" data-testid="user-menu">
      {user.avatar_url ? (
        <img
          src={user.avatar_url}
          alt=""
          className="size-6 rounded-none ring-1 ring-foreground/10"
          data-testid="user-menu-avatar"
        />
      ) : (
        <span
          className="flex size-6 items-center justify-center bg-muted text-muted-foreground ring-1 ring-foreground/10"
          data-testid="user-menu-avatar-fallback"
        >
          <User weight="regular" className="size-3.5" />
        </span>
      )}
      {/* : always `@{login}`; NEVER `{email}`.*/}
      <span className="font-mono text-xs" data-testid="user-menu-handle">
        @{user.login}
      </span>
      <Button
        variant="ghost"
        size="sm"
        onClick={() => logoutMut.mutate()}
        disabled={logoutMut.isPending}
        data-testid="user-menu-signout"
      >
        <SignOut data-icon="inline-start" />
        Sign out
      </Button>
    </div>
  )
}
