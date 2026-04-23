import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import {
  RouterProvider,
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
} from "@tanstack/react-router"
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import * as api from "@/lib/api"
import { AssetsPage } from "./Assets"

/**
 * `AssetsPage` tests. We mock `casFetch` rather than the hooks so that the
 * URL-binding + debounce + server-vs-client filter split are all exercised
 * through the real code path.
 *
 * Test plan:
 *   1. Renders empty state when CAS returns `{xorbs: []}`.
 *   2. Initial load fires GET /admin/xorbs with no query params + GET
 *      /admin/keys for the filter dropdown.
 *   3. Typing a 4-char prefix is NOT forwarded to CAS (still 8-hex rule
 *      server-side) but narrows the list client-side.
 *   4. Typing a full 8-char hex prefix is forwarded as `hash_prefix=...`
 *      after debounce + writes `?hash_prefix=...` to the URL.
 *   5. Renders a row per xorb with the pin-state badge + truncated hash.
 */

type FakeXorb = {
  hash: string
  sia_object_id: string | null
  size_bytes: number
  pin_state: "uploading" | "pinning" | "pinned" | "orphaned"
  uploaded_at: string
  uploader_key_id: string
}

const KEY_UUID = "00000000-0000-4000-8000-000000000001"

const FIXTURE_XORBS: FakeXorb[] = [
  {
    hash: "deadbeef".repeat(8),
    sia_object_id: "siaobj_01HK",
    size_bytes: 64 * 1024 * 1024,
    pin_state: "pinned",
    uploaded_at: "2026-04-20T12:00:00Z",
    uploader_key_id: KEY_UUID,
  },
  {
    hash: "cafebabe".repeat(8),
    sia_object_id: null,
    size_bytes: 4 * 1024,
    pin_state: "pinning",
    uploaded_at: "2026-04-20T13:00:00Z",
    uploader_key_id: KEY_UUID,
  },
]

const FIXTURE_KEYS = [
  {
    id: KEY_UUID,
    name: "onboarding",
    scope: "write",
    masked_prefix: "sia_live_deadbeef...",
    created_at: "2026-04-01T00:00:00Z",
    last_used_at: null,
  },
]

function renderAssets(initialEntries: string[] = ["/assets"]) {
  const qc = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  })
  const rootRoute = createRootRoute()
  const assetsRoute = createRoute({
    getParentRoute: () => rootRoute,
    path: "/assets",
    component: AssetsPage,
  })
  const assetDetailRoute = createRoute({
    getParentRoute: () => rootRoute,
    path: "/assets/$hash",
    component: () => <div data-testid="asset-detail-target">detail</div>,
  })
  const routeTree = rootRoute.addChildren([assetsRoute, assetDetailRoute])
  const router = createRouter({
    routeTree,
    history: createMemoryHistory({ initialEntries }),
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

/**
 * Shared CAS mock. Returns keys + xorbs + (when called with admin/me) a
 * minimal session user so `<UserMenu>` doesn't blow up. Route-dispatches on
 * the first positional arg.
 */
function mockCasFetch(xorbs: FakeXorb[], keys = FIXTURE_KEYS) {
  return vi.spyOn(api, "casFetch").mockImplementation(async (path: string) => {
    if (path.startsWith("/admin/keys")) {
      // useKeys unwraps `.keys` itself — return `{keys}`.
      return { keys } as never
    }
    if (path.startsWith("/admin/me")) {
      return {
        user: {
          id: 1,
          login: "test-user",
          email: null,
          avatar_url: null,
          is_admin: true,
        },
      } as never
    }
    if (path.startsWith("/admin/xorbs")) {
      return { xorbs } as never
    }
    throw new Error(`unexpected casFetch path: ${path}`)
  })
}

beforeEach(() => {
  vi.restoreAllMocks()
  vi.useRealTimers()
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText: vi.fn().mockResolvedValue(undefined) },
  })
  // Reset the real window.location.search so URL-sync assertions are clean.
  window.history.replaceState({}, "", "/assets")
})
afterEach(() => cleanup())

describe("AssetsPage", () => {
  it("renders the empty state when no xorbs match the current filter", async () => {
    mockCasFetch([])
    renderAssets()
    await waitFor(() => {
      expect(screen.getByTestId("assets-empty")).toHaveTextContent(
        "No xorbs match the current filter.",
      )
    })
  })

  it("fires an initial GET /admin/xorbs with no query params", async () => {
    const spy = mockCasFetch(FIXTURE_XORBS)
    renderAssets()
    await waitFor(() => {
      expect(
        screen.getByTestId(`asset-link-${FIXTURE_XORBS[0]!.hash.slice(0, 8)}`),
      ).toBeInTheDocument()
    })
    const xorbCalls = spy.mock.calls.filter((c) => String(c[0] ?? "").startsWith("/admin/xorbs"))
    expect(xorbCalls.length).toBeGreaterThanOrEqual(1)
    expect(String(xorbCalls[0]![0])).toBe("/admin/xorbs")
  })

  it("renders one row per xorb with a pin-state badge and truncated hash", async () => {
    mockCasFetch(FIXTURE_XORBS)
    renderAssets()

    // First row — deadbeef x8 truncated to 16 chars + ellipsis.
    await waitFor(() => {
      expect(screen.getByText(/^deadbeefdeadbeef/)).toBeInTheDocument()
    })
    expect(screen.getByText("pinned")).toBeInTheDocument()
    expect(screen.getByText("pinning")).toBeInTheDocument()
    expect(screen.getByText("64 MB")).toBeInTheDocument()
  })

  it("forwards a full 8-hex prefix as ?hash_prefix= after debounce and syncs the URL", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    const spy = mockCasFetch(FIXTURE_XORBS)
    renderAssets()

    await waitFor(() => {
      expect(screen.getByTestId("assets-search")).toBeInTheDocument()
    })

    fireEvent.change(screen.getByTestId("assets-search"), {
      target: { value: "deadbeef" },
    })

    // Debounce window — settle the 250ms timer.
    await vi.advanceTimersByTimeAsync(300)
    vi.useRealTimers()

    await waitFor(() => {
      const xorbCalls = spy.mock.calls.filter((c) => String(c[0] ?? "").startsWith("/admin/xorbs?"))
      expect(xorbCalls.some((c) => String(c[0]).includes("hash_prefix=deadbeef"))).toBe(true)
    })
    // URL reflects the settled filter.
    expect(window.location.search).toContain("hash_prefix=deadbeef")
  })

  it("does NOT forward a sub-8-char prefix to CAS (server-side 8-hex rule) but still filters client-side", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    const spy = mockCasFetch(FIXTURE_XORBS)
    renderAssets()

    await waitFor(() => {
      expect(screen.getByTestId("assets-search")).toBeInTheDocument()
    })

    fireEvent.change(screen.getByTestId("assets-search"), { target: { value: "dead" } })
    await vi.advanceTimersByTimeAsync(300)
    vi.useRealTimers()

    // No CAS call ever includes hash_prefix=dead (which would 400).
    await waitFor(() => {
      expect(window.location.search).toContain("hash_prefix=dead")
    })
    const xorbCalls = spy.mock.calls.filter((c) => String(c[0] ?? "").startsWith("/admin/xorbs"))
    for (const c of xorbCalls) {
      expect(String(c[0])).not.toContain("hash_prefix=dead")
    }

    // Client-side narrowing: "dead…" row remains, "cafe…" row gone.
    await waitFor(() => {
      expect(screen.queryByText(/^cafebabecafebabe/)).toBeNull()
    })
    expect(screen.getByText(/^deadbeefdeadbeef/)).toBeInTheDocument()
  })

  it("copies the full hash when the inline copy button is clicked", async () => {
    mockCasFetch(FIXTURE_XORBS)
    renderAssets()
    await waitFor(() => {
      expect(
        screen.getByTestId(`asset-copy-${FIXTURE_XORBS[0]!.hash.slice(0, 8)}`),
      ).toBeInTheDocument()
    })
    fireEvent.click(screen.getByTestId(`asset-copy-${FIXTURE_XORBS[0]!.hash.slice(0, 8)}`))
    const writeText = navigator.clipboard.writeText as ReturnType<typeof vi.fn>
    await waitFor(() => {
      expect(writeText).toHaveBeenCalledWith(FIXTURE_XORBS[0]!.hash)
    })
  })
})
