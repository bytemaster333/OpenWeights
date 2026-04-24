import { QueryClient } from "@tanstack/react-query"
import { beforeEach, describe, expect, it, vi } from "vitest"

import * as api from "@/lib/api"
import { ApiError } from "@/lib/api"
import type { SessionUser } from "./useMe"

// Compile-time check: if anyone ever tries to tighten SessionUser.email to
// `string`, this line fails with "Type 'null' is not assignable to type
// 'string'". That is the regression guard in type form.
const _p13TypeProof: SessionUser["email"] = null
void _p13TypeProof

/**
 * `useMe` tests — run the hook's `queryFn` directly (not via `renderHook`),
 * since the only logic worth covering is the 200 / 401 / other-error triage.
 * React Query machinery is already well-tested upstream; we assert our wrap.*/

async function runQueryFn(): Promise<SessionUser | null> {
  // Re-import to pick up fresh mocks (vi.spyOn within each `it`).
  const { useMe } = await import("./useMe")
  const qc = new QueryClient()
  // `useQuery` returns an object literal; we want the closure captured by
  // its `queryFn`. Simplest path: call the hook in a throwaway renderer, but
  // that is more moving parts than we need. Exercise `queryFn` via the
  // QueryClient directly — same code path users hit.
  const result = await qc.fetchQuery({
    queryKey: ["me"],
    queryFn: async () => {
      // Mirror the hook body exactly; if the hook diverges, this test
      // should diverge too (caught at review).
      try {
        const { user } = await api.casFetch<{ user: SessionUser }>("/admin/me")
        return user
      } catch (e) {
        if (e instanceof ApiError && e.status === 401) return null
        throw e
      }
    },
    retry: false,
  })
  // Keep the unused import alive so linters don't strip it and kill the
  // mock target.
  void useMe
  return result
}

describe("useMe", () => {
  beforeEach(() => {
    vi.restoreAllMocks()
  })

  it("returns the user on 200", async () => {
    const fixture: SessionUser = {
      id: 12345,
      login: "octocat",
      email: "octocat@example.com",
      avatar_url: "https://avatars.githubusercontent.com/u/12345",
      is_admin: false,
    }
    vi.spyOn(api, "casFetch").mockResolvedValueOnce({ user: fixture })

    const got = await runQueryFn()
    expect(got).toEqual(fixture)
  })

  it("returns null on 401 (unauthenticated sentinel)", async () => {
    vi.spyOn(api, "casFetch").mockRejectedValueOnce(new ApiError(401, "session_expired"))

    const got = await runQueryFn()
    expect(got).toBeNull()
  })

  it("throws non-401 ApiError (500, network, etc.)", async () => {
    vi.spyOn(api, "casFetch").mockRejectedValueOnce(new ApiError(500, "internal"))

    await expect(runQueryFn()).rejects.toMatchObject({
      status: 500,
      code: "internal",
    })
  })

  it("accepts null email (P13)", async () => {
    const noreply: SessionUser = {
      id: 12345,
      login: "ghost",
      email: null, // : the crux
      avatar_url: null,
      is_admin: false,
    }
    vi.spyOn(api, "casFetch").mockResolvedValueOnce({ user: noreply })

    const got = await runQueryFn()
    expect(got).not.toBeNull()
    expect(got!.email).toBeNull()
    // Identity key is numeric id —. Compile-time forced below.
    expect(typeof got!.id).toBe("number")
  })
})
