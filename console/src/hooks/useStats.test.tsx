import { readFileSync } from "node:fs"
import { join } from "node:path"

import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { renderHook, waitFor } from "@testing-library/react"
import type { ReactNode } from "react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import * as api from "@/lib/api"
import { STATS_REFETCH_INTERVAL_MS, useStats } from "./useStats"

/**
 * CONSOLE-07 — `useStats` is the load-bearing hook behind `/stats`,
 * `/dashboard`, and the benchmark tiles. D-47 locks the refetch cadence at
 * 10 s with background pause. These tests enforce both invariants so a
 * future refactor can't silently halve/double the interval.
 *
 * Strategy: we verify the cadence two ways, each independent of the other:
 * 1. Constant assertion — the exported `STATS_REFETCH_INTERVAL_MS` is the
 * single source of truth.
 * 2. Source-grep — the hook body uses that constant (or the 10_000
 * literal) for `refetchInterval` AND `refetchIntervalInBackground` is
 * `false`. Driving fake timers through TanStack Query's internals
 * (AbortController, queue microtasks) is brittle under Vitest; the
 * grep check catches accidental edits without the flake.
 * 3. Runtime — a fresh mount fires exactly one `casFetch('/admin/stats')`
 * call so we also exercise the query wiring end-to-end.
 */

function wrapper() {
  const qc = new QueryClient({
    defaultOptions: { queries: { retry: false, staleTime: 0 } },
  })
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={qc}>{children}</QueryClientProvider>
  }
}

const FIXTURE = {
  total_bytes_stored: 1024,
  total_bytes_served: 2048,
  cache_hit_rate: 0.75,
  provider_count: 2,
  per_key: [],
  recent_activity: [],
}

describe("useStats constant", () => {
  it("exports STATS_REFETCH_INTERVAL_MS as exactly 10_000 ms (D-47)", () => {
    expect(STATS_REFETCH_INTERVAL_MS).toBe(10_000)
  })
})

describe("useStats source invariants (D-47)", () => {
  /**
   * Guards the 10s cadence + background-pause flag from being edited out
   * by hand. Pairs with the runtime test below (proves the hook actually
   * wires to `/admin/stats` and returns data).
   */
  it("hook source uses the constant for refetchInterval", () => {
    const source = readFileSync(join(__dirname, "useStats.ts"), "utf-8")
    expect(source).toMatch(/refetchInterval:\s*(STATS_REFETCH_INTERVAL_MS|10_000|10000)/)
  })

  it("hook source pauses refetch when tab is backgrounded", () => {
    const source = readFileSync(join(__dirname, "useStats.ts"), "utf-8")
    expect(source).toMatch(/refetchIntervalInBackground:\s*false/)
  })
})

describe("useStats runtime wiring", () => {
  beforeEach(() => {
    vi.restoreAllMocks()
  })

  afterEach(() => {
    vi.restoreAllMocks()
  })

  it("calls casFetch('/admin/stats') on mount and exposes the response", async () => {
    const spy = vi.spyOn(api, "casFetch").mockResolvedValue(FIXTURE)
    const { result } = renderHook(() => useStats(), { wrapper: wrapper() })
    await waitFor(() => expect(result.current.data).toBeDefined())
    expect(spy).toHaveBeenCalledWith("/admin/stats")
    expect(spy).toHaveBeenCalledTimes(1)
    expect(result.current.data).toEqual(FIXTURE)
  })
})
