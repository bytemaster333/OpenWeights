import { Link } from "@tanstack/react-router"

import { UserMenu } from "@/components/UserMenu"
import { useMe } from "@/hooks/useMe"

/**
 * App-chrome header — mounted in the root route so every signed-in page
 * sees it. Self-hides when `useMe` returns `null` (unauth) so `/` (landing)
 * and `/login` stay chrome-less, matching the UX.
 *
 * Layout:
 *
 * [ SiaHub ] Models · Assets · Stats · Map · Keys · Status [ User ]
 *
 * - `UserMenu` lives in the trailing right cluster. The conformance badge
 * was removed when the conformance-harness CI pipeline stopped emitting
 * a status file — "unknown" chrome is worse than no chrome.
 * - Nav labels use `activeProps` so the current route is foreground-tinted
 * without a custom hook.
 *
 * The `pending` state of `useMe` renders `null` (not a skeleton) — the
 * header is chrome, so briefly vanishing is less jarring than a skeleton bar
 * on every navigation.*/
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
          <Link to="/models" activeProps={{ className: "text-foreground" }}>
            Models
          </Link>
          <Link to="/assets" activeProps={{ className: "text-foreground" }}>
            Assets
          </Link>
          <Link
            to="/stats"
            activeOptions={{ exact: true }}
            activeProps={{ className: "text-foreground" }}
          >
            Stats
          </Link>
          <Link to="/stats/map" activeProps={{ className: "text-foreground" }}>
            Map
          </Link>
          <Link to="/keys" activeProps={{ className: "text-foreground" }}>
            Keys
          </Link>
          <Link
            to="/setup"
            activeProps={{ className: "text-foreground" }}
            data-testid="app-header-setup-link"
          >
            Status
          </Link>
          <a
            href="https://docs.siahub.app"
            target="_blank"
            rel="noreferrer"
            className="hover:text-foreground"
          >
            Docs
          </a>
        </nav>
        <div className="ml-auto flex items-center gap-4">
          <UserMenu user={user} />
        </div>
      </div>
    </header>
  )
}
