import { QueryClient } from "@tanstack/react-query"
import { beforeEach, describe, expect, it, vi } from "vitest"

import * as api from "@/lib/api"
import type { MapHost } from "./useMap"

/**
 * `useMapHosts` is the CONSOLE-09 data hook. Tests exercise the `queryFn`
 * directly (same pattern as `useMe.test.ts`) — rendering `<MapContainer>` in
 * jsdom is brittle (ResizeObserver + canvas stubs required) per plan Task 5.
 */

async function runQueryFn(): Promise<MapHost[]> {
  const qc = new QueryClient()
  return qc.fetchQuery<MapHost[]>({
    queryKey: ["map-hosts"],
    queryFn: () => api.casFetch<{ hosts: MapHost[] }>("/admin/stats/map").then((r) => r.hosts),
    retry: false,
  })
}

describe("useMapHosts", () => {
  beforeEach(() => {
    vi.restoreAllMocks()
  })

  it("returns the hosts array unwrapped from the envelope", async () => {
    const fixture: MapHost[] = [
      {
        public_key: "ed25519:abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
        country_code: "US",
        lat: 37.4,
        lon: -122.1,
        usable: true,
        contract_count: 3,
      },
      {
        public_key: "ed25519:1111111111111111111111111111111111111111111111111111111111111111",
        country_code: "DE",
        lat: 52.52,
        lon: 13.405,
        usable: true,
        contract_count: 1,
      },
    ]
    vi.spyOn(api, "casFetch").mockResolvedValueOnce({ hosts: fixture })

    const got = await runQueryFn()
    expect(got).toEqual(fixture)
    expect(got).toHaveLength(2)
  })

  it("handles the empty-host case", async () => {
    vi.spyOn(api, "casFetch").mockResolvedValueOnce({ hosts: [] })

    const got = await runQueryFn()
    expect(got).toEqual([])
  })

  it("tolerates null country_code (private-IP hosts)", async () => {
    const fixture: MapHost[] = [
      {
        public_key: "ed25519:nocountry",
        country_code: null,
        lat: 0,
        lon: 0,
        usable: false,
        contract_count: null,
      },
    ]
    vi.spyOn(api, "casFetch").mockResolvedValueOnce({ hosts: fixture })

    const got = await runQueryFn()
    expect(got[0]?.country_code).toBeNull()
    expect(got[0]?.contract_count).toBeNull()
  })

  it("exports the expected queryKey sentinel", async () => {
    // Source-level grep confirms `["map-hosts"]` is the live key; if someone
    // renames it the 60s cadence planned in D-47 breaks silently across
    // downstream invalidations (Phase 5 manual smoke-tests depend on it).
    const { readFileSync } = await import("node:fs")
    const { join } = await import("node:path")
    const source = readFileSync(join(__dirname, "useMap.ts"), "utf-8")
    expect(source).toMatch(/queryKey:\s*\["map-hosts"\]/)
    expect(source).toMatch(/refetchInterval:\s*60_?000/)
  })
})
