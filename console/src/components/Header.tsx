import { Link } from "@tanstack/react-router"

import { ConformanceBadge } from "@/components/ConformanceBadge"
import { UserMenu } from "@/components/UserMenu"
import { useMe } from "@/hooks/useMe"

/**
 * App-chrome header — mounted in the root route so every signed-in page
 * sees it. Self-hides when `useMe()` returns `null` (unauth) so `/` (landing)
 * and `/login` stay chrome-less, matching the 04-03 UX.
 *
 * Layout:
 *
 *   [ SiaHub ]  Assets · Stats · Map · Keys · (Setup)       [ Badge ] [ User ]
 *
 * - `Setup` link is admin-only (matches the `<AdminGuard>` wrapping the
 *   route; showing the link to non-admins would be a dead-end).
 * - `ConformanceBadge` + `UserMenu` live in a trailing flex-gap cluster.
 * - Nav labels use `activeProps` so the current route is foreground-tinted
 *   without a custom hook.
 *
 * The `pending` state of `useMe()` renders `null` (not a skeleton) — the
 * header is chrome, so briefly vanishing is less jarring than a skeleton bar
 * on every navigation.
 */
export function Header() {
  const { data: user } = useMe()

  // Landing + login pages: no user, no chrome.
  if (!user) return null

  return (
    <header className="border-b bg-background" data-testid="app-header">
      <div className="mx-auto flex max-w-6xl items-center gap-6 px-6 py-3">
        <Link
          to="/dashboard"
          className="font-heading text-lg font-semibold tracking-tight"
          data-testid="app-header-logo"
        >
          SiaHub
        </Link>
        <nav
          className="flex gap-4 text-sm text-muted-foreground"
          aria-label="Primary"
          data-testid="app-header-nav"
        >
          <Link to="/assets" activeProps={{ className: "text-foreground" }}>
            Assets
          </Link>
          <Link to="/stats" activeProps={{ className: "text-foreground" }}>
            Stats
          </Link>
          <Link to="/stats/map" activeProps={{ className: "text-foreground" }}>
            Map
          </Link>
          <Link to="/keys" activeProps={{ className: "text-foreground" }}>
            Keys
          </Link>
          {user.is_admin && (
            <Link
              to="/setup"
              activeProps={{ className: "text-foreground" }}
              data-testid="app-header-setup-link"
            >
              Setup
            </Link>
          )}
        </nav>
        <div className="ml-auto flex items-center gap-4">
          <ConformanceBadge />
          <UserMenu user={user} />
        </div>
      </div>
    </header>
  )
}
