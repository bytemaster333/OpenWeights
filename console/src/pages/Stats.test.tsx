import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import {
  RouterProvider,
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
} from "@tanstack/react-router"
import { cleanup, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { StatsResponse } from "@/hooks/useStats"
import * as api from "@/lib/api"
import { StatsPage } from "./Stats"

/**
 * render test. Mocks `casFetch` to short-circuit `/admin/stats`,
 * `/admin/me`, and `/admin/keys`, so the page renders synthetic data and we
 * can assert the per-key usage table resolves key names.*/

const STATS_FIXTURE: StatsResponse = {
  total_bytes_stored: 1_073_741_824, // 1.00 GB
  total_bytes_served: 524_288, // 512 KB
  total_downloads: 0,
  cache_hit_rate: 0.875,
  provider_count: 3,
  per_key: [
    { key_id: "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", bytes_stored: 2048, bytes_served: 4096 },
    { key_id: "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb", bytes_stored: 1024, bytes_served: 0 },
  ],
  recent_activity: [
    {
      ts: "2026-04-21T12:00:00Z",
      hash: "abcdef0123456789feedcafe",
      event: "download",
      bytes: 8192,
      cache_hit: true,
    },
    {
      ts: "2026-04-21T11:59:00Z",
      hash: null,
      event: "xorb_upload",
      bytes: null,
      cache_hit: null,
    },
  ],
}

const ME_FIXTURE = {
  user: {
    id: 42,
    login: "testuser",
    email: null,
    avatar_url: null,
    is_admin: false,
  },
}

const KEYS_FIXTURE = {
  keys: [
    {
      id: "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
      name: "prod-upload-key",
      scope: "write" as const,
      masked_prefix: "sia_live_abc...",
      created_at: "2026-04-20T00:00:00Z",
      last_used_at: null,
    },
    {
      id: "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb",
      name: "read-only",
      scope: "read" as const,
      masked_prefix: "sia_live_def...",
      created_at: "2026-04-20T00:00:00Z",
      last_used_at: null,
    },
  ],
}

function renderStats() {
  const qc = new QueryClient({
    defaultOptions: { queries: { retry: false, staleTime: 0 } },
  })
  const rootRoute = createRootRoute()
  const statsRoute = createRoute({
    getParentRoute: () => rootRoute,
    path: "/stats",
    component: StatsPage,
  })
  const benchmarksRoute = createRoute({
    getParentRoute: () => rootRoute,
    path: "/stats/benchmarks",
    component: () => <div>benchmarks</div>,
  })
  const routeTree = rootRoute.addChildren([statsRoute, benchmarksRoute])
  const router = createRouter({
    routeTree,
    history: createMemoryHistory({ initialEntries: ["/stats"] }),
  })
  return render(
    <QueryClientProvider client={qc}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  )
}

beforeEach(() => {
  vi.restoreAllMocks()
  vi.spyOn(api, "casFetch").mockImplementation(async (path: string) => {
    if (path === "/admin/stats") return STATS_FIXTURE as never
    if (path === "/admin/me") return ME_FIXTURE as never
    if (path === "/admin/keys") return KEYS_FIXTURE as never
    throw new Error(`unexpected path: ${path}`)
  })
})

afterEach(() => {
  cleanup()
})

describe("<StatsPage>", () => {
  it("renders per-key usage rows with resolved key names", async () => {
    renderStats()
    expect(await screen.findByText("prod-upload-key")).toBeInTheDocument()
    expect(screen.getByText("read-only")).toBeInTheDocument()
  })
})
