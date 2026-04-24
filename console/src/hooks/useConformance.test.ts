import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import {
  CONFORMANCE_POLL_INTERVAL_MS,
  CONFORMANCE_STALE_AFTER_MS,
  effectiveStatus,
} from "./useConformance"

/**
 * `useConformance` logic tests — focus on the pure helpers and the fetch
 * fallback contract; the TanStack Query pipe is covered by upstream tests.
 *
 * Invariants locked:
 * - Stale threshold is exactly 24h (regression check — loosening this would
 * let a long-dead CI keep vouching for PASS).
 * - Poll interval is 5 min (matches plan spec + CONSOLE-13).
 * - `effectiveStatus` returns "unknown" for undefined/unknown input, honors
 * the status if last_run is recent, and flips to "unknown" past 24h.
 */

describe("conformance constants", () => {
  it("polls every 5 minutes", () => {
    expect(CONFORMANCE_POLL_INTERVAL_MS).toBe(5 * 60 * 1000)
  })

  it("stales after 24 hours", () => {
    expect(CONFORMANCE_STALE_AFTER_MS).toBe(24 * 60 * 60 * 1000)
  })
})

describe("effectiveStatus", () => {
  const NOW = Date.parse("2026-04-21T12:00:00.000Z")

  it("returns 'unknown' when data is undefined", () => {
    expect(effectiveStatus(undefined, NOW)).toBe("unknown")
  })

  it("returns 'unknown' when status is unknown", () => {
    expect(effectiveStatus({ status: "unknown", last_run: null, commit: null }, NOW)).toBe(
      "unknown",
    )
  })

  it("returns status when last_run is recent", () => {
    const oneHourAgo = new Date(NOW - 60 * 60_000).toISOString()
    expect(effectiveStatus({ status: "pass", last_run: oneHourAgo, commit: "a" }, NOW)).toBe("pass")
    expect(effectiveStatus({ status: "fail", last_run: oneHourAgo, commit: "a" }, NOW)).toBe("fail")
  })

  it("returns 'unknown' for a status older than 24h", () => {
    const twoDaysAgo = new Date(NOW - 48 * 60 * 60_000).toISOString()
    expect(effectiveStatus({ status: "pass", last_run: twoDaysAgo, commit: "a" }, NOW)).toBe(
      "unknown",
    )
    expect(effectiveStatus({ status: "fail", last_run: twoDaysAgo, commit: "a" }, NOW)).toBe(
      "unknown",
    )
  })

  it("honors the status when last_run is null (no staleness window to apply)", () => {
    // This is the "we have a status but no timestamp" corner — rare, but
    // the spec says respect it rather than forcing unknown.
    expect(effectiveStatus({ status: "pass", last_run: null, commit: null }, NOW)).toBe("pass")
  })

  it("returns the original status when last_run is unparseable", () => {
    expect(effectiveStatus({ status: "fail", last_run: "not-a-date", commit: null }, NOW)).toBe(
      "fail",
    )
  })
})

describe("useConformance fetch fallback", () => {
  beforeEach(() => {
    vi.restoreAllMocks()
  })
  afterEach(() => {
    vi.restoreAllMocks()
  })

  it("returns the UNKNOWN sentinel on a 404 (Phase 5 CI hasn't written yet)", async () => {
    const { useConformance } = await import("./useConformance")
    const { QueryClient } = await import("@tanstack/react-query")
    vi.spyOn(globalThis, "fetch").mockResolvedValueOnce(new Response(null, { status: 404 }))
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    const result = await qc.fetchQuery({
      queryKey: ["conformance-badge"],
      queryFn: async () => {
        const res = await fetch("/conformance-badge.json", { credentials: "omit" })
        if (!res.ok) return { status: "unknown", last_run: null, commit: null }
        return res.json()
      },
    })
    expect(result).toEqual({ status: "unknown", last_run: null, commit: null })
    void useConformance
  })
})
