import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { cleanup, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import * as useSetup from "@/hooks/useSetupStatus"
import type { SetupStatus } from "@/hooks/useSetupStatus"
import { SetupPage } from "./Setup"

/**
 * `/setup` page tests.
 *
 * Exercises the component against a stubbed `useSetupStatus` hook. We don't
 * test the hook-plumbing here (that's a TanStack Query pipe tested elsewhere);
 * we test the copy, variants, and regression invariants:
 *
 * - 5 tiles render — Postgres, Redis, indexd, GitHub OAuth, V2.
 * - OAuth card shows "configured" / "not configured" (P14 copy).
 * - V2 card is read-only: no button / toggle / checkbox / switch is rendered.
 * - Indexer URL surfaces from `data.indexd.url` (CONSOLE-12).
 * - On OAuth missing, the OAuthErrorBanner (code=oauth_client_not_configured)
 * is rendered at the top of the page (P14 surfacing).
 */

type HookShape = ReturnType<typeof useSetup.useSetupStatus>

function mockSetup(partial: Partial<HookShape>) {
  const base = {
    data: undefined,
    error: null,
    isPending: false,
    refetch: vi.fn(),
  } as unknown as HookShape
  vi.spyOn(useSetup, "useSetupStatus").mockReturnValue({ ...base, ...partial } as HookShape)
}

const OK_STATUS: SetupStatus = {
  postgres: { status: "ok", latency_ms: 1.2 },
  redis: { status: "ok", latency_ms: 0.8 },
  indexd: {
    status: "ok",
    latency_ms: 12.4,
    synced: true,
    url: "http://indexd:9980",
  },
  github_oauth: { configured: true },
  v2_reconstruction_enabled: false,
}

const OAUTH_MISSING: SetupStatus = {
  ...OK_STATUS,
  github_oauth: { configured: false },
}

function renderPage() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return render(
    <QueryClientProvider client={qc}>
      <SetupPage />
    </QueryClientProvider>,
  )
}

beforeEach(() => {
  vi.restoreAllMocks()
})
afterEach(() => cleanup())

describe("SetupPage", () => {
  it("renders all 5 tiles when status is healthy", () => {
    mockSetup({ data: OK_STATUS, isPending: false })
    renderPage()
    expect(screen.getByTestId("setup-tile-postgres")).toBeInTheDocument()
    expect(screen.getByTestId("setup-tile-redis")).toBeInTheDocument()
    expect(screen.getByTestId("setup-tile-indexd")).toBeInTheDocument()
    expect(screen.getByTestId("setup-tile-oauth")).toBeInTheDocument()
    expect(screen.getByTestId("setup-tile-v2")).toBeInTheDocument()
  })

  it("shows the indexer URL read-only (CONSOLE-12)", () => {
    mockSetup({ data: OK_STATUS, isPending: false })
    renderPage()
    expect(screen.getByTestId("setup-indexd-url")).toHaveTextContent("http://indexd:9980")
  })

  it("shows 'configured' on the OAuth card when github_oauth.configured=true", () => {
    mockSetup({ data: OK_STATUS, isPending: false })
    renderPage()
    const badge = screen.getByTestId("setup-oauth-badge")
    expect(badge.getAttribute("data-configured")).toBe("true")
    expect(badge).toHaveTextContent(/configured/i)
  })

  it("shows 'not configured' + the OAuthErrorBanner when github_oauth.configured=false (P14)", () => {
    mockSetup({ data: OAUTH_MISSING, isPending: false })
    renderPage()
    const badge = screen.getByTestId("setup-oauth-badge")
    expect(badge.getAttribute("data-configured")).toBe("false")
    expect(badge).toHaveTextContent(/not configured/i)

    // P14 remediation banner — the one reserved specifically for /setup.
    const banner = screen.getByTestId("oauth-error-banner")
    expect(banner.getAttribute("data-error-code")).toBe("oauth_client_not_configured")

    // Env-var hint must be in the DOM for the operator to copy-paste. The
    // names appear in both the banner AND the card body, which is the point
    // (P14: one message is easy to miss) — so we assert ≥1 occurrence each.
    const tile = screen.getByTestId("setup-tile-oauth")
    expect(tile.textContent).toMatch(/GITHUB_OAUTH_CLIENT_ID/)
    expect(tile.textContent).toMatch(/GITHUB_OAUTH_CLIENT_SECRET/)
    expect(tile.textContent).toMatch(/GITHUB_OAUTH_CALLBACK_URL/)
  })

  it("does NOT render a toggle/switch/checkbox for v2_reconstruction_enabled (Ambiguity 3)", () => {
    mockSetup({ data: OK_STATUS, isPending: false })
    const { container } = renderPage()

    // Scope to the V2 card and assert there's no interactive flip.
    const v2Tile = screen.getByTestId("setup-tile-v2")
    expect(v2Tile.querySelector("button")).toBeNull()
    expect(v2Tile.querySelector("input[type='checkbox']")).toBeNull()
    expect(v2Tile.querySelector("[role='switch']")).toBeNull()

    // And the value surfaces as read-only text.
    expect(screen.getByTestId("setup-v2-flag")).toHaveTextContent(/false/)

    // Sanity — the retry button on the error path isn't accidentally on the
    // success path either.
    expect(container.querySelector("[data-testid='setup-retry']")).toBeNull()
  })

  it("shows the V2 'enabled' badge when flag is true", () => {
    mockSetup({
      data: { ...OK_STATUS, v2_reconstruction_enabled: true },
      isPending: false,
    })
    renderPage()
    expect(screen.getByTestId("setup-v2-badge")).toHaveTextContent(/enabled/)
    expect(screen.getByTestId("setup-v2-flag")).toHaveTextContent(/true/)
  })

  it("shows loading placeholder while pending", () => {
    mockSetup({ data: undefined, isPending: true })
    renderPage()
    expect(screen.getByTestId("setup-loading")).toBeInTheDocument()
  })

  it("shows error state + retry button when the query errors", () => {
    mockSetup({
      data: undefined,
      isPending: false,
      error: new Error("boom"),
    } as Partial<HookShape>)
    renderPage()
    expect(screen.getByTestId("setup-error")).toBeInTheDocument()
    expect(screen.getByTestId("setup-retry")).toBeInTheDocument()
  })

  it("reflects degraded status on the indexd tile (synced=false)", () => {
    mockSetup({
      data: {
        ...OK_STATUS,
        indexd: {
          status: "degraded",
          latency_ms: 50,
          synced: false,
          url: "http://indexd:9980",
        },
      },
      isPending: false,
    })
    renderPage()
    expect(screen.getByTestId("setup-indexd-synced")).toHaveTextContent(/no/)
  })
})
