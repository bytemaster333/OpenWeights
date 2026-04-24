import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import {
  RouterProvider,
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
} from "@tanstack/react-router"
import { cleanup, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import * as api from "@/lib/api"
import { AssetDetailPage } from "./AssetDetail"

/**
 * `AssetDetailPage` tests. Same mock-`casFetch`-through-the-hook pattern as
 * `Assets.test.tsx` to exercise the 404 → list-scan fallback that
 * `useAsset` relies on while CAS does not yet ship
 * `GET /admin/xorbs/{hash}` (see `useAssets.ts` docblock).
 *
 * Tests:
 * 1. Happy path — the detail endpoint 200s; every required field
 * renders.
 * 2. `sia_object_id: null` renders the em-dash placeholder (covers
 * pin_state="orphaned").
 * 3. `referencing_repos: ["user/repo"]` renders the list section.
 * 4. Unknown hash → "Not found" (detail 404 + scan also misses).*/

const HASH_A = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
const HASH_B = "cafebabecafebabecafebabecafebabecafebabecafebabecafebabecafebabe"
const KEY_UUID = "00000000-0000-4000-8000-000000000001"

function renderDetail(path: string) {
  const qc = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  })
  const rootRoute = createRootRoute()
  const detailRoute = createRoute({
    getParentRoute: () => rootRoute,
    path: "/assets/$hash",
    component: AssetDetailPage,
  })
  const assetsRoute = createRoute({
    getParentRoute: () => rootRoute,
    path: "/assets",
    component: () => <div data-testid="assets-target">assets list</div>,
  })
  const routeTree = rootRoute.addChildren([detailRoute, assetsRoute])
  const router = createRouter({
    routeTree,
    history: createMemoryHistory({ initialEntries: [path] }),
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
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText: vi.fn().mockResolvedValue(undefined) },
  })
})
afterEach(() => cleanup())

describe("AssetDetailPage", () => {
  it("renders every required field when the detail endpoint 200s", async () => {
    vi.spyOn(api, "casFetch").mockImplementation(async (path: string) => {
      if (path.startsWith("/admin/me")) {
        return {
          user: { id: 1, login: "u", email: null, avatar_url: null, is_admin: true },
        } as never
      }
      if (path === `/admin/xorbs/${HASH_A}`) {
        return {
          xorb: {
            hash: HASH_A,
            sia_object_id: "siaobj_0123456789",
            size_bytes: 1_073_741_824,
            pin_state: "pinned",
            uploaded_at: "2026-04-20T12:00:00Z",
            uploader_key_id: KEY_UUID,
          },
          referencing_repos: [],
        } as never
      }
      throw new Error(`unexpected path: ${path}`)
    })

    renderDetail(`/assets/${HASH_A}`)

    await waitFor(() => {
      expect(screen.getByTestId("asset-detail-hash")).toHaveTextContent(HASH_A)
    })
    expect(screen.getByTestId("asset-detail-sia-object-id")).toHaveTextContent("siaobj_0123456789")
    expect(screen.getByTestId("asset-detail-size")).toHaveTextContent("1.00 GB")
    expect(screen.getByText("pinned")).toBeInTheDocument()
    expect(screen.getByTestId("asset-detail-uploader-key")).toHaveTextContent(KEY_UUID)
  })

  it("renders an em-dash when sia_object_id is null (orphaned xorb)", async () => {
    vi.spyOn(api, "casFetch").mockImplementation(async (path: string) => {
      if (path.startsWith("/admin/me")) {
        return {
          user: { id: 1, login: "u", email: null, avatar_url: null, is_admin: true },
        } as never
      }
      if (path === `/admin/xorbs/${HASH_B}`) {
        return {
          xorb: {
            hash: HASH_B,
            sia_object_id: null,
            size_bytes: 512,
            pin_state: "orphaned",
            uploaded_at: "2026-04-20T12:00:00Z",
            uploader_key_id: KEY_UUID,
          },
          referencing_repos: [],
        } as never
      }
      throw new Error(`unexpected path: ${path}`)
    })

    renderDetail(`/assets/${HASH_B}`)

    await waitFor(() => {
      expect(screen.getByTestId("asset-detail-sia-object-id")).toHaveTextContent("—")
    })
    expect(screen.getByText("orphaned")).toBeInTheDocument()
  })

  it("renders the referencing-repos section when the detail endpoint returns repos", async () => {
    vi.spyOn(api, "casFetch").mockImplementation(async (path: string) => {
      if (path.startsWith("/admin/me")) {
        return {
          user: { id: 1, login: "u", email: null, avatar_url: null, is_admin: true },
        } as never
      }
      if (path === `/admin/xorbs/${HASH_A}`) {
        return {
          xorb: {
            hash: HASH_A,
            sia_object_id: "siaobj_X",
            size_bytes: 1024,
            pin_state: "pinned",
            uploaded_at: "2026-04-20T12:00:00Z",
            uploader_key_id: KEY_UUID,
          },
          referencing_repos: ["user/repo-a", "user/repo-b"],
        } as never
      }
      throw new Error(`unexpected path: ${path}`)
    })

    renderDetail(`/assets/${HASH_A}`)

    await waitFor(() => {
      expect(screen.getByTestId("asset-detail-referencing-repos")).toBeInTheDocument()
    })
    expect(screen.getByText("user/repo-a")).toBeInTheDocument()
    expect(screen.getByText("user/repo-b")).toBeInTheDocument()
  })

  it("renders 'Not found' when both the detail route and the list-scan fallback miss", async () => {
    vi.spyOn(api, "casFetch").mockImplementation(async (path: string) => {
      if (path.startsWith("/admin/me")) {
        return {
          user: { id: 1, login: "u", email: null, avatar_url: null, is_admin: true },
        } as never
      }
      if (path === `/admin/xorbs/${HASH_A}`) {
        throw new api.ApiError(404, "not_found")
      }
      if (path.startsWith("/admin/xorbs?")) {
        // List-scan fallback returns no matches.
        return { xorbs: [] } as never
      }
      throw new Error(`unexpected path: ${path}`)
    })

    renderDetail(`/assets/${HASH_A}`)

    await waitFor(() => {
      expect(screen.getByTestId("asset-detail-not-found")).toBeInTheDocument()
    })
  })
})
