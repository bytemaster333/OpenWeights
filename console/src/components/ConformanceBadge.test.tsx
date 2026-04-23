import { cleanup, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import * as useConf from "@/hooks/useConformance"
import { ConformanceBadge } from "./ConformanceBadge"

/**
 * `<ConformanceBadge>` tests.
 *
 * Covers:
 *  - PASS / FAIL / unknown all render distinct label + `data-status`
 *  - A PASS older than the 24h staleness threshold collapses to `unknown`
 *    (defensive: a silently-dead CI shouldn't keep vouching for green)
 *  - Hover title surfaces `last_run` + `commit` when present
 */

function mockHook(data: useConf.ConformanceBadgeData | undefined) {
  // Minimal shape — the component only reads `data`.
  vi.spyOn(useConf, "useConformance").mockReturnValue({
    data,
  } as ReturnType<typeof useConf.useConformance>)
}

beforeEach(() => {
  vi.restoreAllMocks()
})
afterEach(() => cleanup())

describe("<ConformanceBadge>", () => {
  it("renders PASS with green/default variant when status=pass and recent", () => {
    mockHook({
      status: "pass",
      last_run: new Date().toISOString(),
      commit: "abc1234",
    })
    render(<ConformanceBadge />)
    const el = screen.getByTestId("conformance-badge")
    expect(el).toHaveTextContent(/Conformance: PASS/)
    expect(el.getAttribute("data-status")).toBe("pass")
    // Variant encoded via Badge's `data-variant`.
    expect(el.getAttribute("data-variant")).toBe("default")
  })

  it("renders FAIL with destructive variant when status=fail", () => {
    mockHook({
      status: "fail",
      last_run: new Date().toISOString(),
      commit: "deadbee",
    })
    render(<ConformanceBadge />)
    const el = screen.getByTestId("conformance-badge")
    expect(el).toHaveTextContent(/Conformance: FAIL/)
    expect(el.getAttribute("data-status")).toBe("fail")
    expect(el.getAttribute("data-variant")).toBe("destructive")
  })

  it("renders unknown with secondary variant when data is undefined", () => {
    mockHook(undefined)
    render(<ConformanceBadge />)
    const el = screen.getByTestId("conformance-badge")
    expect(el).toHaveTextContent(/Conformance: unknown/)
    expect(el.getAttribute("data-status")).toBe("unknown")
    expect(el.getAttribute("data-variant")).toBe("secondary")
  })

  it("renders unknown when server reports status=unknown", () => {
    mockHook({
      status: "unknown",
      last_run: null,
      commit: null,
      note: "populated by Phase 5 conformance CI artifact",
    })
    render(<ConformanceBadge />)
    const el = screen.getByTestId("conformance-badge")
    expect(el.getAttribute("data-status")).toBe("unknown")
  })

  it("collapses a 48h-old PASS to 'unknown' via staleness check", () => {
    const twoDaysAgo = new Date(Date.now() - 48 * 60 * 60_000).toISOString()
    mockHook({
      status: "pass",
      last_run: twoDaysAgo,
      commit: "abc1234",
    })
    render(<ConformanceBadge />)
    const el = screen.getByTestId("conformance-badge")
    expect(el.getAttribute("data-status")).toBe("unknown")
  })

  it("exposes last_run + commit via title tooltip", () => {
    const iso = "2026-04-21T12:34:56.000Z"
    mockHook({ status: "pass", last_run: iso, commit: "abc1234" })
    render(<ConformanceBadge />)
    const el = screen.getByTestId("conformance-badge")
    const title = el.getAttribute("title") ?? ""
    expect(title).toMatch(/Last run:/)
    expect(title).toMatch(/Commit: abc1234/)
  })
})
