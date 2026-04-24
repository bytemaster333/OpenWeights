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
 * render tests. Mocks `casFetch` to short-circuit both
 * `/admin/stats` and `/admin/me` + `/admin/keys`, so the page renders
 * synthetic data and we can assert the 4 tiles, per-key table, and
 * recent-activity table all render as expected.*/

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
  it("renders the 4 KPI tiles with formatted values", async () => {
    renderStats()
    expect(await screen.findByText("Stored on Sia")).toBeInTheDocument()
    // 1_073_741_824 B = 1.00 GB (v < 10 branch in bytes)
    expect(await screen.findByText("1.00 GB")).toBeInTheDocument()
    // "Bytes served" / "Bytes stored" also appear as table-column headers,
    // so we query all occurrences and assert at least one (= the tile).
    expect(screen.getAllByText("Bytes served").length).toBeGreaterThanOrEqual(1)
    expect(screen.getByText("512 KB")).toBeInTheDocument()
    expect(screen.getByText("Cache hit rate")).toBeInTheDocument()
    expect(screen.getByText("87.5%")).toBeInTheDocument()
    expect(screen.getByText("API keys with usage")).toBeInTheDocument()
    expect(screen.getByText("3")).toBeInTheDocument()
  })

  it("renders per-key usage rows with resolved key names", async () => {
    renderStats()
    expect(await screen.findByText("prod-upload-key")).toBeInTheDocument()
    expect(screen.getByText("read-only")).toBeInTheDocument()
  })

  it("renders recent activity rows with event type and bytes", async () => {
    renderStats()
    await screen.findByText("download")
    expect(screen.getByText("download")).toBeInTheDocument()
    expect(screen.getByText("xorb_upload")).toBeInTheDocument()
    // 8192 bytes renders as "8.00 KB" (v < 10 branch)
    expect(screen.getByText("8.00 KB")).toBeInTheDocument()
    // Null bytes render as "—"
    const dashCells = screen.getAllByText("—")
    expect(dashCells.length).toBeGreaterThanOrEqual(1)
  })

  it("renders the benchmarks footer link", async () => {
    renderStats()
    expect(await screen.findByText(/View benchmarks/)).toBeInTheDocument()
    const link = screen.getByText(/docs\/benchmarks\.md/)
    expect(link).toBeInTheDocument()
  })
})
