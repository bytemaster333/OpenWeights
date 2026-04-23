import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import {
  RouterProvider,
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
} from "@tanstack/react-router"
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import * as api from "@/lib/api"
import { KeysPage } from "./Keys"

/**
 * Keys page tests — exercise the real `useKeys` / `useCreateKey` /
 * `useRevokeKey` hooks end-to-end against a mocked `casFetch`. This keeps
 * the contract between the table UI + the request shape under test.
 */

type ListedKeyFixture = {
  id: string
  name: string
  scope: "read" | "write" | "admin"
  masked_prefix: string
  created_at: string
  last_used_at: string | null
}

type CreatedKeyFixture = ListedKeyFixture & {
  plaintext_key: string
}

const ROW_A: ListedKeyFixture = {
  id: "key_A",
  name: "laptop-1",
  scope: "write",
  masked_prefix: "sia_live_aaaa",
  created_at: "2026-04-01T12:00:00Z",
  last_used_at: "2026-04-20T12:00:00Z",
}
const ROW_B: ListedKeyFixture = {
  id: "key_B",
  name: "ci-runner",
  scope: "read",
  masked_prefix: "sia_live_bbbb",
  created_at: "2026-04-10T12:00:00Z",
  last_used_at: null,
}

const CREATED: CreatedKeyFixture = {
  id: "key_NEW",
  name: "new-box",
  scope: "write",
  masked_prefix: "sia_live_nnnn",
  created_at: "2026-04-21T12:00:00Z",
  last_used_at: null,
  plaintext_key: "sia_live_PLAINTEXT_XYZ_super_secret",
}

function renderKeys() {
  const qc = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  })

  const rootRoute = createRootRoute()
  const keysRoute = createRoute({
    getParentRoute: () => rootRoute,
    path: "/keys",
    component: KeysPage,
  })
  const routeTree = rootRoute.addChildren([keysRoute])
  const router = createRouter({
    routeTree,
    history: createMemoryHistory({ initialEntries: ["/keys"] }),
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

describe("KeysPage", () => {
  it("renders the empty state when the list is empty", async () => {
    vi.spyOn(api, "casFetch").mockResolvedValueOnce({ keys: [] })
    renderKeys()

    await waitFor(() => {
      expect(screen.getByTestId("keys-empty")).toBeInTheDocument()
    })
    expect(screen.getByTestId("keys-empty")).toHaveTextContent(
      /No keys yet — create one to start uploading/,
    )
  })

  it("renders rows with masked_prefix values and NEVER surfaces any plaintext_key-ish field", async () => {
    vi.spyOn(api, "casFetch").mockResolvedValueOnce({ keys: [ROW_A, ROW_B] })
    renderKeys()

    await waitFor(() => {
      expect(screen.getByTestId(`keys-row-${ROW_A.id}`)).toBeInTheDocument()
    })

    // Both prefixes rendered in their respective rows.
    expect(screen.getByTestId(`keys-prefix-${ROW_A.id}`)).toHaveTextContent(ROW_A.masked_prefix)
    expect(screen.getByTestId(`keys-prefix-${ROW_B.id}`)).toHaveTextContent(ROW_B.masked_prefix)

    // Names + scopes rendered.
    const rowA = screen.getByTestId(`keys-row-${ROW_A.id}`)
    expect(within(rowA).getByText(ROW_A.name)).toBeInTheDocument()
    expect(within(rowA).getByText(ROW_A.scope)).toBeInTheDocument()

    // Last-used null → "—".
    const rowB = screen.getByTestId(`keys-row-${ROW_B.id}`)
    expect(rowB).toHaveTextContent("—")

    // Negative assertion: the CAS payload must not expose a plaintext field
    // on a list row. Defensive catch if a future dev accidentally widens
    // `ListedKey` to include `plaintext_key`. Scoped to the table so the
    // page's explanatory copy ("Plaintext is shown once...") doesn't match.
    expect(screen.getByTestId("keys-table").textContent).not.toMatch(/plaintext/i)
  })

  it("opens the confirmation dialog on Revoke click and calls DELETE /admin/keys/{id} on confirm", async () => {
    const spy = vi.spyOn(api, "casFetch")
    // Initial list fetch.
    spy.mockResolvedValueOnce({ keys: [ROW_A] })
    // Revoke DELETE.
    spy.mockResolvedValueOnce(undefined as unknown as { keys: ListedKeyFixture[] })
    // Refetch after invalidate.
    spy.mockResolvedValueOnce({ keys: [] })

    renderKeys()

    await waitFor(() => {
      expect(screen.getByTestId(`keys-revoke-${ROW_A.id}`)).toBeInTheDocument()
    })

    // Open the AlertDialog.
    await act(async () => {
      fireEvent.click(screen.getByTestId(`keys-revoke-${ROW_A.id}`))
    })
    expect(
      screen.getByRole("alertdialog", { name: new RegExp(`Revoke "${ROW_A.name}"`, "i") }),
    ).toBeInTheDocument()

    // Confirm → DELETE call.
    await act(async () => {
      fireEvent.click(screen.getByTestId(`keys-revoke-confirm-${ROW_A.id}`))
      await Promise.resolve()
      await Promise.resolve()
    })

    // First call was the GET list, second is our DELETE.
    const deleteCall = spy.mock.calls.find(
      ([path, init]) =>
        path === `/admin/keys/${ROW_A.id}` &&
        (init as RequestInit | undefined)?.method === "DELETE",
    )
    expect(deleteCall).toBeTruthy()

    // Invalidation triggers a refetch that returns empty — wait for empty state.
    await waitFor(() => {
      expect(screen.getByTestId("keys-empty")).toBeInTheDocument()
    })
  })

  it("creates a key: submits POST /admin/keys and opens OneTimeKeyModal with plaintext", async () => {
    const spy = vi.spyOn(api, "casFetch")
    // Initial list fetch (empty).
    spy.mockResolvedValueOnce({ keys: [] })
    // POST /admin/keys.
    spy.mockResolvedValueOnce(CREATED)
    // Refetch after invalidate — now includes the new row.
    spy.mockResolvedValueOnce({
      keys: [
        {
          id: CREATED.id,
          name: CREATED.name,
          scope: CREATED.scope,
          masked_prefix: CREATED.masked_prefix,
          created_at: CREATED.created_at,
          last_used_at: null,
        },
      ],
    })

    renderKeys()

    await waitFor(() => {
      expect(screen.getByTestId("keys-create-submit")).toBeInTheDocument()
    })

    // Type name, keep scope=write (default), submit.
    fireEvent.change(screen.getByTestId("keys-create-name"), {
      target: { value: CREATED.name },
    })
    await act(async () => {
      fireEvent.click(screen.getByTestId("keys-create-submit"))
      await Promise.resolve()
      await Promise.resolve()
    })

    // POST body is correct.
    const postCall = spy.mock.calls.find(
      ([path, init]) =>
        path === "/admin/keys" && (init as RequestInit | undefined)?.method === "POST",
    )
    expect(postCall).toBeTruthy()
    expect((postCall?.[1] as RequestInit).body).toBe(
      JSON.stringify({ name: CREATED.name, scope: "write" }),
    )

    // OneTimeKeyModal renders plaintext.
    await waitFor(() => {
      expect(screen.getByTestId("one-time-key-plaintext")).toHaveTextContent(CREATED.plaintext_key)
    })

    // After invalidation + refetch, the new row shows the masked prefix
    // (NOT the plaintext).
    await waitFor(() => {
      expect(screen.getByTestId(`keys-prefix-${CREATED.id}`)).toHaveTextContent(
        CREATED.masked_prefix,
      )
    })
  })

  it("dismisses the OneTimeKeyModal after ack and never renders the plaintext in any table cell", async () => {
    const spy = vi.spyOn(api, "casFetch")
    spy.mockResolvedValueOnce({ keys: [] })
    spy.mockResolvedValueOnce(CREATED)
    spy.mockResolvedValueOnce({
      keys: [
        {
          id: CREATED.id,
          name: CREATED.name,
          scope: CREATED.scope,
          masked_prefix: CREATED.masked_prefix,
          created_at: CREATED.created_at,
          last_used_at: null,
        },
      ],
    })

    renderKeys()

    await waitFor(() => {
      expect(screen.getByTestId("keys-create-submit")).toBeInTheDocument()
    })

    fireEvent.change(screen.getByTestId("keys-create-name"), {
      target: { value: CREATED.name },
    })
    await act(async () => {
      fireEvent.click(screen.getByTestId("keys-create-submit"))
      await Promise.resolve()
      await Promise.resolve()
    })

    // Wait for modal, then ack.
    await waitFor(() => {
      expect(screen.getByTestId("one-time-key-plaintext")).toBeInTheDocument()
    })
    await act(async () => {
      fireEvent.click(screen.getByTestId("one-time-key-ack"))
      await Promise.resolve()
    })
    await waitFor(() => {
      expect(screen.queryByTestId("one-time-key-plaintext")).toBeNull()
    })

    // After ack, the table shows masked_prefix for the new row and the
    // plaintext literal is absent from the entire DOM (D-45 guarantee).
    await waitFor(() => {
      expect(screen.getByTestId(`keys-prefix-${CREATED.id}`)).toHaveTextContent(
        CREATED.masked_prefix,
      )
    })
    expect(document.body.textContent).not.toContain(CREATED.plaintext_key)
  })

  it("surfaces a list-query error via <Alert variant=destructive>", async () => {
    vi.spyOn(api, "casFetch").mockRejectedValueOnce(new api.ApiError(500, "internal", "req-abc"))
    renderKeys()

    await waitFor(() => {
      expect(screen.getByTestId("keys-list-error")).toBeInTheDocument()
    })
    expect(screen.getByTestId("keys-list-error")).toHaveTextContent(/internal/i)
  })
})
