import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { OneTimeKeyModal } from "./OneTimeKeyModal"

/**
 * OneTimeKeyModal tests — the load-bearing guarantees:
 *
 * 1. Plaintext is present in the DOM when `open={true}`.
 * 2. The Copy button writes to `navigator.clipboard`.
 * 3. Closing via `onOpenChange(false)` is a no-op until ack.
 * 4. Clicking "I've saved it" calls `onAck` exactly once.
 * 5. The modal never writes to `localStorage` / `sessionStorage`.*/

beforeEach(() => {
  vi.restoreAllMocks()
})
afterEach(() => cleanup())

describe("OneTimeKeyModal", () => {
  it("renders the plaintext when open", () => {
    render(
      <OneTimeKeyModal open={true} plaintext="sia_secret_abc123" scope="write" onAck={() => {}} />,
    )
    expect(screen.getByTestId("one-time-key-plaintext")).toHaveTextContent("sia_secret_abc123")
  })

  it("does NOT render the plaintext when closed", () => {
    render(
      <OneTimeKeyModal open={false} plaintext="sia_secret_abc123" scope="write" onAck={() => {}} />,
    )
    // Dialog is not portaled/rendered when closed.
    expect(screen.queryByTestId("one-time-key-plaintext")).toBeNull()
    expect(document.body.textContent).not.toContain("sia_secret_abc123")
  })

  it("writes the plaintext to clipboard on Copy click", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    })

    render(
      <OneTimeKeyModal open={true} plaintext="sia_secret_abc123" scope="write" onAck={() => {}} />,
    )

    await act(async () => {
      fireEvent.click(screen.getByTestId("one-time-key-copy"))
      // Flush the clipboard promise.
      await Promise.resolve()
      await Promise.resolve()
    })
    expect(writeText).toHaveBeenCalledExactlyOnceWith("sia_secret_abc123")
  })

  it("does NOT invoke onAck when dialog close is attempted before acknowledgment", () => {
    const onAck = vi.fn()
    render(
      <OneTimeKeyModal open={true} plaintext="sia_secret_abc123" scope="write" onAck={onAck} />,
    )

    // Press Escape on the dialog content — radix routes this through
    // `onOpenChange(false)`, which our component blocks pre-ack.
    fireEvent.keyDown(screen.getByTestId("one-time-key-modal"), {
      key: "Escape",
      code: "Escape",
    })
    expect(onAck).not.toHaveBeenCalled()
  })

  it("invokes onAck when 'I've saved it' is clicked", () => {
    const onAck = vi.fn()
    render(
      <OneTimeKeyModal open={true} plaintext="sia_secret_abc123" scope="write" onAck={onAck} />,
    )

    fireEvent.click(screen.getByTestId("one-time-key-ack"))
    expect(onAck).toHaveBeenCalledOnce()
  })

  it("attaches a beforeunload listener while open + un-ack'd, removes it after unmount", () => {
    const addSpy = vi.spyOn(window, "addEventListener")
    const removeSpy = vi.spyOn(window, "removeEventListener")

    const { unmount } = render(
      <OneTimeKeyModal open={true} plaintext="sia_secret_abc123" scope="write" onAck={() => {}} />,
    )

    const beforeUnloadAdds = addSpy.mock.calls.filter(([event]) => event === "beforeunload")
    expect(beforeUnloadAdds.length).toBeGreaterThanOrEqual(1)

    unmount()

    const beforeUnloadRemoves = removeSpy.mock.calls.filter(([event]) => event === "beforeunload")
    expect(beforeUnloadRemoves.length).toBeGreaterThanOrEqual(1)
  })

  it("labels the capability line by the key's actual scope", () => {
    // regression: the modal used to hardcode "write scope" for every key,
    // mislabeling read (download-only) keys.
    const { rerender } = render(
      <OneTimeKeyModal open={true} plaintext="sia_secret_abc123" scope="read" onAck={() => {}} />,
    )
    const modal = () => screen.getByTestId("one-time-key-modal")
    expect(modal()).toHaveTextContent(/read\s+scope/)
    expect(modal()).toHaveTextContent(/download/)
    expect(modal().textContent).not.toContain("upload new xorbs")

    rerender(
      <OneTimeKeyModal open={true} plaintext="sia_secret_abc123" scope="write" onAck={() => {}} />,
    )
    expect(modal()).toHaveTextContent(/write\s+scope/)
    expect(modal()).toHaveTextContent(/upload new xorbs/)
  })

  it("does NOT persist the plaintext to localStorage or sessionStorage", () => {
    const localSet = vi.spyOn(Storage.prototype, "setItem")
    render(
      <OneTimeKeyModal open={true} plaintext="sia_secret_abc123" scope="write" onAck={() => {}} />,
    )

    // ThemeProvider may have written the theme key; that's fine. What we
    // care about is that NO call includes the plaintext as a value or key.
    for (const [key, value] of localSet.mock.calls) {
      expect(String(key)).not.toContain("sia_secret_abc123")
      expect(String(value)).not.toContain("sia_secret_abc123")
    }
  })
})
