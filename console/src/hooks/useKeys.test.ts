import { readFileSync } from "node:fs"
import { join } from "node:path"
import { describe, expect, it } from "vitest"

/**
 * `useKeys` is exercised end-to-end via `Onboarding.test.tsx` (real hook,
 * mocked `casFetch`). This file adds a single grep-style source guard that
 * enforces : the hook source MUST NOT reference `localStorage` or
 * `sessionStorage` — the plaintext `CreatedKey.plaintext_key` lives only in
 * React state bounded by the `<OneTimeKeyModal>`.*/

/**
 * Strip single-line (`//`) and block (`/* ... *\/`) comments so that the
 * invariants can be checked against executable code only. Comments
 * may legitimately mention `localStorage` / `sessionStorage` in the
 * context of "MUST NOT be written to X" — we only care that the code
 * does not reference them.*/
function stripComments(source: string): string {
  return source
    .replace(/\/\*[\s\S]*?\*\//g, "") // block comments
    .replace(/(^|[^:])\/\/.*$/gm, "$1") // line comments (avoid http://)
}

describe("useKeys source invariants (D-45)", () => {
  it("does NOT reference localStorage or sessionStorage in executable code", () => {
    const source = stripComments(readFileSync(join(__dirname, "useKeys.ts"), "utf-8"))
    expect(source).not.toMatch(/localStorage/)
    expect(source).not.toMatch(/sessionStorage/)
  })

  it("exports CreatedKey with a plaintext_key field typed string", () => {
    const source = readFileSync(join(__dirname, "useKeys.ts"), "utf-8")
    // Loose structural check — the actual type shape is enforced by tsc.
    expect(source).toMatch(/plaintext_key:\s*string/)
  })
})
