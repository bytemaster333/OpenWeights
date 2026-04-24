import { cleanup, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it } from "vitest"

import { OAuthErrorBanner, type OAuthErrorCode } from "./OAuthErrorBanner"

/**
 * mitigation — verify every error code emitted by CAS's
 * `CallbackError::into_response` has console-side copy, and that the error
 * banner surfaces the code + an operator hint. Tests one row per code so
 * when CAS adds a new variant, plan-checker wedges the gap instantly.*/

afterEach(() => cleanup())

const CASES: Array<{
  code: OAuthErrorCode
  titleFragment: RegExp
  hintFragment: RegExp
}> = [
  {
    code: "oauth_state_mismatch",
    titleFragment: /state mismatch/i,
    hintFragment: /signing in again/i,
  },
  {
    code: "oauth_code_missing",
    titleFragment: /code missing/i,
    hintFragment: /approve the consent screen/i,
  },
  {
    code: "github_token_exchange_failed",
    titleFragment: /token exchange failed/i,
    hintFragment: /GITHUB_OAUTH_CLIENT_(ID|SECRET)/,
  },
  {
    code: "oauth_client_not_configured",
    titleFragment: /OAuth is not configured/i,
    hintFragment: /GITHUB_OAUTH_CLIENT_ID/,
  },
  {
    code: "oauth_callback_host_mismatch",
    titleFragment: /callback host mismatch/i,
    hintFragment: /GITHUB_OAUTH_CALLBACK_URL/,
  },
]

describe("OAuthErrorBanner (P14)", () => {
  for (const { code, titleFragment, hintFragment } of CASES) {
    it(`renders distinct copy for ${code}`, () => {
      render(<OAuthErrorBanner code={code} />)
      const banner = screen.getByTestId("oauth-error-banner")
      expect(banner).toHaveAttribute("data-error-code", code)
      expect(banner.textContent).toMatch(titleFragment)
      expect(banner.textContent).toMatch(hintFragment)
      // Always surfaces the machine-readable code so operators can grep
      // logs / cross-reference with CAS errors.
      expect(banner.textContent).toContain(code)
    })
  }
})
