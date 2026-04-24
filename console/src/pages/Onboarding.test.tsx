import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import {
  RouterProvider,
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
} from "@tanstack/react-router"
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import * as api from "@/lib/api"
import { OnboardingPage } from "./Onboarding"

/**
 * Onboarding page tests — wired against a real `useCreateKey` mutation that
 * goes through a mocked `casFetch`. This exercises the exact code path
 * users hit (no bespoke mock of the hook), while still isolating us from
 * CAS.*/

type CreatedKeyFixture = {
  id: string
  name: string
  scope: "read" | "write" | "admin"
  masked_prefix: string
  plaintext_key: string
  created_at: string
}

const FIXTURE: CreatedKeyFixture = {
  id: "key_01HKZXY",
  name: "onboarding",
  scope: "write",
  masked_prefix: "sia_live_xxx",
  plaintext_key: "sia_live_super_secret_abc123",
  created_at: "2026-04-21T12:00:00Z",
}

function renderOnboarding() {
  const qc = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  })

  const rootRoute = createRootRoute()
  const onboardingRoute = createRoute({
    getParentRoute: () => rootRoute,
    path: "/onboarding",
    component: OnboardingPage,
  })
  // Dashboard target for navigate click tests.
  const dashboardRoute = createRoute({
    getParentRoute: () => rootRoute,
    path: "/dashboard",
    component: () => <div data-testid="dashboard-page">dashboard</div>,
  })
  const routeTree = rootRoute.addChildren([onboardingRoute, dashboardRoute])
  const router = createRouter({
    routeTree,
    history: createMemoryHistory({ initialEntries: ["/onboarding"] }),
  })

  return {
    qc,
    router,
    ...render(
      <QueryClientProvider client={qc}>
        <RouterProvider router={router} />
      </QueryClientProvider>,
    ),
  }
}

beforeEach(() => {
  vi.restoreAllMocks()
  // Always mock clipboard to a resolved no-op so Sonner toasts don't throw.
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText: vi.fn().mockResolvedValue(undefined) },
  })
})
afterEach(() => cleanup())

describe("OnboardingPage", () => {
  it("fires exactly one POST /admin/keys on mount", async () => {
    const spy = vi.spyOn(api, "casFetch").mockResolvedValueOnce(FIXTURE)
    renderOnboarding()

    await waitFor(() => {
      expect(spy).toHaveBeenCalled()
    })
    // Exactly one call — StrictMode double-invoke guard.
    expect(spy).toHaveBeenCalledOnce()
    expect(spy).toHaveBeenCalledWith("/admin/keys", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name: "onboarding", scope: "write" }),
    })
  })

  it("opens the modal with plaintext on successful key creation", async () => {
    vi.spyOn(api, "casFetch").mockResolvedValueOnce(FIXTURE)
    renderOnboarding()

    await waitFor(() => {
      expect(screen.getByTestId("one-time-key-plaintext")).toHaveTextContent(FIXTURE.plaintext_key)
    })
  })

  it("renders the env block with HF_XET_DATA_DEFAULT_CAS_ENDPOINT + HF_XET_DATA_CUSTOM_HEADERS inlining the plaintext key", async () => {
    vi.spyOn(api, "casFetch").mockResolvedValueOnce(FIXTURE)
    renderOnboarding()

    await waitFor(() => {
      const env = screen.getByTestId("onboarding-env-block")
      expect(env).toHaveTextContent("HF_XET_DATA_DEFAULT_CAS_ENDPOINT")
      expect(env).toHaveTextContent("HF_XET_DATA_CUSTOM_HEADERS")
      expect(env).toHaveTextContent(FIXTURE.plaintext_key)
      expect(env).toHaveTextContent("Bearer")
    })
  })

  it("renders the huggingface-cli upload example", async () => {
    vi.spyOn(api, "casFetch").mockResolvedValueOnce(FIXTURE)
    renderOnboarding()

    await waitFor(() => {
      const upload = screen.getByTestId("onboarding-upload-block")
      expect(upload).toHaveTextContent("huggingface-cli upload")
    })
  })

  it("does NOT leak the plaintext to localStorage or sessionStorage", async () => {
    const localSet = vi.spyOn(Storage.prototype, "setItem")
    vi.spyOn(api, "casFetch").mockResolvedValueOnce(FIXTURE)
    renderOnboarding()

    await waitFor(() => {
      expect(screen.getByTestId("one-time-key-plaintext")).toBeInTheDocument()
    })

    for (const [key, value] of localSet.mock.calls) {
      expect(String(key)).not.toContain(FIXTURE.plaintext_key)
      expect(String(value)).not.toContain(FIXTURE.plaintext_key)
    }
  })

  it("copies the env block when the card's Copy button is clicked", async () => {
    vi.spyOn(api, "casFetch").mockResolvedValueOnce(FIXTURE)
    renderOnboarding()

    await waitFor(() => {
      expect(screen.getByTestId("onboarding-env-block-copy")).toBeInTheDocument()
    })

    await act(async () => {
      fireEvent.click(screen.getByTestId("onboarding-env-block-copy"))
      await Promise.resolve()
      await Promise.resolve()
    })

    const writeText = navigator.clipboard.writeText as ReturnType<typeof vi.fn>
    expect(writeText).toHaveBeenCalledOnce()
    const written = writeText.mock.calls[0]?.[0] as string
    expect(written).toContain("HF_XET_DATA_DEFAULT_CAS_ENDPOINT")
    expect(written).toContain(FIXTURE.plaintext_key)
  })

  it("surfaces an error state + retry CTA when key creation fails", async () => {
    vi.spyOn(api, "casFetch").mockRejectedValueOnce(new api.ApiError(500, "internal", "req-abc"))
    renderOnboarding()

    await waitFor(() => {
      expect(screen.getByTestId("onboarding-retry")).toBeInTheDocument()
    })
    // No plaintext should have surfaced in the error path.
    expect(screen.queryByTestId("one-time-key-plaintext")).toBeNull()
  })
})
