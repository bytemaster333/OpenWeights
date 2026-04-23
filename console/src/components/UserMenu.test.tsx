import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { SessionUser } from "@/hooks/useMe"
import * as api from "@/lib/api"
import { UserMenu } from "./UserMenu"

/**
 * P13 regression tests for UserMenu.
 *
 * The single load-bearing invariant: email (null OR `@users.noreply...`)
 * MUST NOT appear in the rendered DOM. The display handle is always
 * `@{login}`. If a future refactor tries to "be helpful" and render the
 * email as a tooltip or secondary label, these tests fail.
 */

function renderWithProviders(ui: React.ReactElement) {
  const qc = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  return {
    qc,
    ...render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>),
  }
}

beforeEach(() => {
  vi.restoreAllMocks()
})
afterEach(() => cleanup())

describe("UserMenu P13 compliance", () => {
  it("does NOT render the word 'null' when email is null", () => {
    const user: SessionUser = {
      id: 1,
      login: "ghost",
      email: null,
      avatar_url: "https://avatars.example/ghost.png",
      is_admin: false,
    }
    const { container } = renderWithProviders(<UserMenu user={user} />)

    // Check the full rendered text for the literal string "null".
    expect(container.textContent).not.toMatch(/null/i)
    // And render the handle.
    expect(screen.getByTestId("user-menu-handle")).toHaveTextContent("@ghost")
  })

  it("does NOT render the `@users.noreply.github.com` email", () => {
    const user: SessionUser = {
      id: 2,
      login: "octocat",
      email: "octocat@users.noreply.github.com",
      avatar_url: null,
      is_admin: false,
    }
    const { container } = renderWithProviders(<UserMenu user={user} />)

    // Neither the noreply domain nor ANY @-domain email substring should
    // leak out.
    expect(container.textContent).not.toMatch(/@users\.noreply\.github\.com/)
    expect(container.textContent).not.toMatch(/[\w.+-]+@[\w.-]+\.\w+/)
    expect(screen.getByTestId("user-menu-handle")).toHaveTextContent("@octocat")
  })

  it("does NOT render a regular email either — login is the only identity shown", () => {
    const user: SessionUser = {
      id: 3,
      login: "alice",
      email: "alice@example.com",
      avatar_url: null,
      is_admin: true,
    }
    const { container } = renderWithProviders(<UserMenu user={user} />)

    expect(container.textContent).not.toMatch(/alice@example\.com/)
    expect(screen.getByTestId("user-menu-handle")).toHaveTextContent("@alice")
  })

  it("falls back to a user icon when avatar_url is null", () => {
    const user: SessionUser = {
      id: 4,
      login: "x",
      email: null,
      avatar_url: null,
      is_admin: false,
    }
    renderWithProviders(<UserMenu user={user} />)

    expect(screen.queryByTestId("user-menu-avatar")).toBeNull()
    expect(screen.getByTestId("user-menu-avatar-fallback")).toBeInTheDocument()
  })
})

describe("UserMenu sign out", () => {
  it("calls api.logout when Sign out is clicked (AUTH-05)", async () => {
    const spy = vi.spyOn(api, "logout").mockResolvedValue(undefined)
    // Stub window.location so the `href = "/"` assignment in onSuccess
    // doesn't try to actually navigate under jsdom.
    const original = window.location
    Object.defineProperty(window, "location", {
      writable: true,
      value: { ...original, href: "" },
    })

    const user: SessionUser = {
      id: 5,
      login: "octocat",
      email: null,
      avatar_url: "https://avatars.example/octocat.png",
      is_admin: false,
    }
    renderWithProviders(<UserMenu user={user} />)

    await act(async () => {
      fireEvent.click(screen.getByTestId("user-menu-signout"))
      // Let the useMutation promise chain settle inside act().
      await Promise.resolve()
      await Promise.resolve()
    })
    expect(spy).toHaveBeenCalledOnce()

    Object.defineProperty(window, "location", { writable: true, value: original })
  })
})
