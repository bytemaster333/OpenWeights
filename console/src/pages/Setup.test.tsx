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
 * we cover the loading placeholder, the error + retry state, and the
 * degraded-indexd (synced=false) variant.
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
