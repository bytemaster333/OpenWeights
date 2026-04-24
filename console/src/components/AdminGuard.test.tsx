import { cleanup, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import * as useMeMod from "@/hooks/useMe"
import type { SessionUser } from "@/hooks/useMe"
import { AdminGuard } from "./AdminGuard"

/**
 * `<AdminGuard>` tests.
 *
 * The component's contract:
 * - isPending → render the neutral "Loading…" placeholder (same copy as
 * AuthGuard so stacked guards look continuous).
 * - not admin → render the "Admins only" message and NOT the children.
 * - admin → render the children.
 *
 * CAS enforces `is_admin` server-side; this guard is pure UX.*/

type MeHookShape = ReturnType<typeof useMeMod.useMe>

function mockMe(result: Partial<MeHookShape>) {
  vi.spyOn(useMeMod, "useMe").mockReturnValue(result as MeHookShape)
}

const NON_ADMIN: SessionUser = {
  id: 1,
  login: "member",
  email: null,
  avatar_url: null,
  is_admin: false,
}

const ADMIN: SessionUser = {
  id: 2,
  login: "admin",
  email: null,
  avatar_url: null,
  is_admin: true,
}

beforeEach(() => {
  vi.restoreAllMocks()
})
afterEach(() => cleanup())

describe("<AdminGuard>", () => {
  it("renders the Loading placeholder while pending", () => {
    mockMe({ data: undefined, isPending: true })
    render(
      <AdminGuard>
        <div data-testid="admin-child">secret</div>
      </AdminGuard>,
    )
    expect(screen.getByTestId("admin-guard-loading")).toBeInTheDocument()
    expect(screen.queryByTestId("admin-child")).toBeNull()
  })

  it("renders 'Admins only' for a non-admin user and hides children", () => {
    mockMe({ data: NON_ADMIN, isPending: false })
    render(
      <AdminGuard>
        <div data-testid="admin-child">secret</div>
      </AdminGuard>,
    )
    const denied = screen.getByTestId("admin-guard-denied")
    expect(denied).toHaveTextContent(/Admins only/)
    expect(denied).toHaveTextContent(/is_admin/)
    expect(screen.queryByTestId("admin-child")).toBeNull()
  })

  it("renders 'Admins only' for an unauthenticated user (data=null)", () => {
    // useMe returns null on 401 — AdminGuard should treat this as a deny
    // rather than crashing on `user.is_admin`.
    mockMe({ data: null, isPending: false })
    render(
      <AdminGuard>
        <div data-testid="admin-child">secret</div>
      </AdminGuard>,
    )
    expect(screen.getByTestId("admin-guard-denied")).toBeInTheDocument()
    expect(screen.queryByTestId("admin-child")).toBeNull()
  })

  it("renders children for an admin user", () => {
    mockMe({ data: ADMIN, isPending: false })
    render(
      <AdminGuard>
        <div data-testid="admin-child">secret</div>
      </AdminGuard>,
    )
    expect(screen.getByTestId("admin-child")).toBeInTheDocument()
    expect(screen.queryByTestId("admin-guard-denied")).toBeNull()
  })
})
